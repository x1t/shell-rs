//! Copying between two SFTP tabs' servers. One server seldom reaches the
//! other, so the files come down from the tab they are on to a folder on
//! this computer, then go up from it through the other tab, as in meatshell
//! and electerm. Each half is an ordinary batch on its tab's queue, with
//! its progress, questions and 停止 there.

use std::path::PathBuf;

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;

use crate::explorer::{ExplorerId, QueueId, TransferJob};
use crate::host::ConnectionState;
use crate::i18n::t;

use super::Workspace;

/// A copy between two tabs, by the batch it waits on.
pub(super) enum Relay {
    /// Coming down from the source tab; then up through `to`, into `target`
    /// there or the directory it shows by then.
    Download {
        to: ExplorerId,
        target: Option<String>,
        folder: PathBuf,
        names: Vec<String>,
    },
    /// Going up; the folder goes once that is done.
    Upload { folder: PathBuf },
}

impl Relay {
    fn folder(self) -> PathBuf {
        match self {
            Relay::Download { folder, .. } | Relay::Upload { folder } => folder,
        }
    }
}

impl Workspace {
    /// Copy `paths` on `from`'s server to `to`'s: down first.
    pub(super) fn copy_to_tab(
        &mut self,
        from: ExplorerId,
        to: ExplorerId,
        paths: Vec<String>,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.explorers.get(&from).cloned() else {
            return;
        };
        if paths.is_empty() || from == to {
            return;
        }
        if !self.is_connected_explorer(to, cx) {
            window.push_notification(Notification::error(t!("explorer.copy.target_gone")), cx);
            return;
        }
        self.next_relay += 1;
        let folder = std::env::temp_dir().join(format!(
            "shellrs-copy-{}-{}",
            std::process::id(),
            self.next_relay
        ));
        if let Err(error) = std::fs::create_dir_all(&folder) {
            window.push_notification(Notification::error(error.to_string()), cx);
            return;
        }
        let names = paths.iter().map(|path| remote_name(path)).collect();
        let job = TransferJob::Download {
            paths,
            target: folder.display().to_string(),
        };
        match source.update(cx, |source, cx| source.enqueue(job, window, cx)) {
            Some(batch) => {
                let relay = Relay::Download {
                    to,
                    target,
                    folder,
                    names,
                };
                self.relays.insert((from, batch), relay);
            }
            None => remove_folder(folder, cx),
        }
    }

    /// A batch of `explorer`'s went through: if a copy waits on it, the
    /// next half starts, or the folder goes.
    pub(super) fn on_transfer_finished(
        &mut self,
        explorer: ExplorerId,
        batch: QueueId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.relays.remove(&(explorer, batch)) {
            Some(Relay::Download {
                to,
                target,
                folder,
                names,
            }) => self.copy_up(to, target, folder, names, window, cx),
            Some(Relay::Upload { folder }) => remove_folder(folder, cx),
            None => {}
        }
    }

    /// A closed tab's batches will not go through: the copies waiting on
    /// them end, and their folders go.
    pub(super) fn end_relays_of(&mut self, explorer: ExplorerId, cx: &mut Context<Self>) {
        let ended: Vec<_> = self
            .relays
            .keys()
            .filter(|(of, _)| *of == explorer)
            .copied()
            .collect();
        for key in ended {
            if let Some(relay) = self.relays.remove(&key) {
                remove_folder(relay.folder(), cx);
            }
        }
    }

    /// Up through `to` from `folder`: what came down of `names`.
    fn copy_up(
        &mut self,
        to: ExplorerId,
        target: Option<String>,
        folder: PathBuf,
        names: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self
            .explorers
            .get(&to)
            .filter(|_| self.is_connected_explorer(to, cx))
            .cloned()
        else {
            window.push_notification(Notification::error(t!("explorer.copy.target_gone")), cx);
            remove_folder(folder, cx);
            return;
        };
        let target = target.unwrap_or_else(|| panel.read(cx).remote().read(cx).path());
        // Those that failed to come down are not there, and said so already.
        let paths: Vec<PathBuf> = names
            .iter()
            .map(|name| folder.join(name))
            .filter(|path| path.symlink_metadata().is_ok())
            .collect();
        if paths.is_empty() {
            remove_folder(folder, cx);
            return;
        }
        let job = TransferJob::Upload { paths, target };
        match panel.update(cx, |panel, cx| panel.enqueue(job, window, cx)) {
            Some(batch) => {
                self.relays.insert((to, batch), Relay::Upload { folder });
            }
            None => remove_folder(folder, cx),
        }
    }

    fn is_connected_explorer(&self, explorer: ExplorerId, cx: &App) -> bool {
        self.explorers
            .get(&explorer)
            .is_some_and(|panel| panel.read(cx).connection_state() == ConnectionState::Connected)
    }
}

/// The last part of a remote path: the name a download gives it here.
fn remote_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

/// Off the main thread: a tree can be large. One left behind, say with a
/// file another program holds, is in the system's temporary folder, which
/// it clears.
fn remove_folder(folder: PathBuf, cx: &App) {
    cx.background_spawn(async move {
        let _ = std::fs::remove_dir_all(folder);
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::remote_name;

    #[test]
    fn a_remote_path_comes_down_under_its_last_name() {
        assert_eq!(remote_name("/var/log/syslog"), "syslog");
        assert_eq!(remote_name("/srv/site/"), "site");
        assert_eq!(remote_name("notes.txt"), "notes.txt");
    }
}
