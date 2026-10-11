//! A remote file in an editor of another maker, as WinSCP does it: a copy
//! here, opened there, and each save of the copy written back over the SFTP
//! tab's connection. The copies are watched once a second while there are
//! any; their folder goes with the tab.
//!
//! A write back is checked like the editor's own save: when the server's
//! file changed since it was read, the user is asked before it is replaced.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::{WindowExt as _, dialog::DialogButtonProps, notification::Notification};
use gpui_kit::*;

use super::ExplorerPanel;
use super::external_editor::{ExternalEditor, ExternalEditors};
use super::file_edit::{FileLocation, Opening};
use crate::i18n::t;
use crate::sftp::{FileBytes, FileStamp, RemotePath, SaveFailure};

/// The largest file opened this way: the copy is read whole first.
pub const EXTERNAL_LIMIT: u64 = 50 * 1024 * 1024;

/// How often the copies are looked at for a save.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// A remote file open in an editor through a copy here.
struct ExternalCopy {
    remote: RemotePath,
    local: PathBuf,
    /// The server's file as last read or written. `None` after a write
    /// that broke off: the next one goes without checking, as the editor's
    /// own does.
    server: Option<FileStamp>,
    /// The copy as last written or sent back.
    seen: Option<FileStamp>,
    /// Being written back: the next look waits for it.
    sending: bool,
}

/// One SFTP tab's copies, in a folder of their own that goes with the tab.
#[derive(Default)]
pub(super) struct ExternalCopies {
    dir: Option<PathBuf>,
    copies: Vec<ExternalCopy>,
    /// How many copies there have been: each has a folder of its own, so
    /// two files of one name do not meet.
    made: usize,
    watch: Option<Task<()>>,
}

impl Drop for ExternalCopies {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            // Left behind only when the system will not let it go; the
            // system's temporary folder is cleaned in time either way.
            let _ = fs::remove_dir_all(dir);
        }
    }
}

impl ExternalCopies {
    fn copy_of(&mut self, remote: &RemotePath) -> Option<&mut ExternalCopy> {
        self.copies.iter_mut().find(|copy| &copy.remote == remote)
    }

    /// Where a new copy of `name` goes.
    fn place_for(&mut self, name: &str) -> PathBuf {
        let dir = self.dir.get_or_insert_with(|| {
            std::env::temp_dir()
                .join("shellrs-edit")
                .join(uuid::Uuid::new_v4().simple().to_string())
        });
        self.made += 1;
        dir.join(self.made.to_string()).join(name)
    }
}

/// What a copy is now, from the disk; `None` when it cannot be read.
fn stamp_of(path: &Path) -> Option<FileStamp> {
    fs::metadata(path)
        .ok()
        .map(|metadata| FileStamp::local(&metadata))
}

impl ExplorerPanel {
    /// 用 VS Code / Zed 打开. A local file opens as it is; a remote one is
    /// read first, then opened through a copy, or through the copy it
    /// already has.
    pub(super) fn open_with(
        &mut self,
        location: FileLocation,
        editor: ExternalEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &location {
            FileLocation::Local(path) => self.launch(editor, &location, &path.clone(), window, cx),
            FileLocation::Remote(remote) => {
                match self.external.copy_of(remote).map(|copy| copy.local.clone()) {
                    Some(local) => self.launch(editor, &location, &local, window, cx),
                    None => self.open_file(location, Opening::External(editor), window, cx),
                }
            }
        }
    }

    /// A remote file was read for `editor`: make its copy, open it, and
    /// watch it.
    pub(super) fn open_copy(
        &mut self,
        location: FileLocation,
        editor: ExternalEditor,
        file: FileBytes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let FileLocation::Remote(remote) = &location else {
            return;
        };
        let local = self.external.place_for(&location.name());
        let server = Some(file.stamp());
        let written = local
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&local, file.into_inner()));
        if let Err(error) = written {
            self.say_not_opened(editor, &location, error.to_string(), window, cx);
            return;
        }
        self.external.copies.push(ExternalCopy {
            remote: remote.clone(),
            local: local.clone(),
            server,
            seen: stamp_of(&local),
            sending: false,
        });
        self.watch_copies(window, cx);
        self.launch(editor, &location, &local, window, cx);
    }

    fn launch(
        &mut self,
        editor: ExternalEditor,
        location: &FileLocation,
        file: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = ExternalEditors::open(editor, file, cx) {
            self.say_not_opened(editor, location, error.to_string(), window, cx);
        }
    }

    fn say_not_opened(
        &self,
        editor: ExternalEditor,
        location: &FileLocation,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = t!(
            "explorer.external.open_failed",
            name = location.name(),
            editor = editor.name()
        );
        window.push_notification(Notification::error(error).title(title), cx);
    }

    /// Look at the copies once a second from now on, for as long as the
    /// tab is there.
    fn watch_copies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.external.watch.is_some() {
            return;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_EVERY).await;
                let looked = this.update_in(cx, |this, window, cx| this.look_at_copies(window, cx));
                if looked.is_err() {
                    break;
                }
            }
        });
        self.external.watch = Some(task);
    }

    /// Send back each copy saved since it was last seen.
    fn look_at_copies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut saved = Vec::new();
        for copy in &mut self.external.copies {
            if copy.sending {
                continue;
            }
            let now = stamp_of(&copy.local);
            if now.is_some() && now != copy.seen {
                copy.seen = now;
                saved.push(copy.remote.clone());
            }
        }
        for remote in saved {
            self.send_copy(&remote, true, window, cx);
        }
    }

    /// Write a copy over its remote file; with `check`, only when that is
    /// still what was read or last written.
    fn send_copy(
        &mut self,
        remote: &RemotePath,
        check: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(copy) = self.external.copy_of(remote) else {
            return;
        };
        let (local, expected) = (copy.local.clone(), copy.server.filter(|_| check));
        let name = remote.file_name().to_string();
        let bytes = match fs::read(&local) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.say_not_sent(&name, error.to_string(), window, cx);
                return;
            }
        };
        // Not connected: this tab has said so, and the next save tries again.
        let Some(write) = self.write_remote_file(remote.clone(), bytes, expected, window, cx)
        else {
            return;
        };
        if let Some(copy) = self.external.copy_of(remote) {
            copy.sending = true;
        }
        let remote = remote.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.copy_sent(&remote, result, window, cx)
            });
        })
        .detach();
    }

    fn copy_sent(
        &mut self,
        remote: &RemotePath,
        result: Result<FileStamp, SaveFailure>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(copy) = self.external.copy_of(remote) else {
            return;
        };
        copy.sending = false;
        let name = remote.file_name().to_string();
        match result {
            Ok(stamp) => {
                copy.server = Some(stamp);
                let message = t!("explorer.external.sent", name = name);
                window.push_notification(Notification::success(message), cx);
                self.reload_if_showing(&FileLocation::Remote(remote.clone()), window, cx);
            }
            Err(SaveFailure::Changed) => self.ask_overwrite(remote.clone(), window, cx),
            Err(SaveFailure::Interrupted(message)) => {
                copy.server = None;
                self.say_not_sent(&name, message, window, cx);
            }
            Err(SaveFailure::Failed(message)) => self.say_not_sent(&name, message, window, cx),
        }
    }

    fn say_not_sent(&self, name: &str, error: String, window: &mut Window, cx: &mut App) {
        let title = t!("explorer.external.send_failed", name = name);
        window.push_notification(Notification::error(error).title(title), cx);
    }

    /// The server's file changed since it was read: replace it only when
    /// the user says so. Otherwise it stays, until the copy is saved again.
    fn ask_overwrite(&mut self, remote: RemotePath, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.weak_entity();
        let name = remote.file_name().to_string();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (panel, remote) = (panel.clone(), remote.clone());
            alert
                .title(t!("explorer.external.changed_title", name = name))
                .description(t!("explorer.external.changed"))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("explorer.external.overwrite"))
                        .cancel_text(t!("common.cancel")),
                )
                .show_cancel(true)
                .on_ok(move |_, window, cx| {
                    let _ =
                        panel.update(cx, |panel, cx| panel.send_copy(&remote, false, window, cx));
                    true
                })
        });
    }
}
