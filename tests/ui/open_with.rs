//! 用 VS Code / Zed 打开: a file from the SFTP lists in an editor of
//! another maker. A remote one goes through a copy here whose saves are
//! written back over the tab's connection.

use std::io;
use std::path::{Path, PathBuf};

use crate::support::*;
use shellrs::app::{ExplorerAction, ExplorerCommand};
use shellrs::explorer::{EditorLauncher, ExternalEditor, ExternalEditors};

const REMOTE_FILE: &str = "/home/tester/文件 甲.txt";
const LOCAL_FILE: &str = "/local/tester/文件 甲.txt";

/// An editor that is not started: what was opened in what, and where.
#[derive(Default)]
struct FakeEditors {
    installed: Vec<ExternalEditor>,
    opened: Arc<Mutex<Vec<(ExternalEditor, PathBuf)>>>,
}

impl FakeEditors {
    fn both() -> Self {
        Self {
            installed: ExternalEditor::ALL.to_vec(),
            ..Self::default()
        }
    }

    /// The file the last open was given, and the editor it went to.
    fn last_opened(self: &Arc<Self>) -> Option<(ExternalEditor, PathBuf)> {
        self.opened
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last()
            .cloned()
    }
}

impl EditorLauncher for FakeEditors {
    fn installed(&self) -> Vec<ExternalEditor> {
        self.installed.clone()
    }

    fn open(&self, editor: ExternalEditor, file: &Path) -> io::Result<()> {
        if !self.installed.contains(&editor) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                editor.name().to_string(),
            ));
        }
        self.opened
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((editor, file.to_path_buf()));
        Ok(())
    }
}

/// An SFTP tab on a server holding `文件 甲.txt`, with these editors on the
/// computer.
async fn open_server(
    cx: &mut TestAppContext,
    editors: Arc<FakeEditors>,
) -> (WindowHandle<Root>, Entity<Workspace>, Arc<FakeSftpProvider>) {
    let provider = Arc::new(FakeSftpProvider::with_files(&[(
        REMOTE_FILE,
        "原内容\n".as_bytes(),
    )]));
    let (handle, workspace) = open_workspace_with_sftp(cx, provider.clone());
    cx.update(|cx| cx.set_global(ExternalEditors(editors)));
    open_test_explorer(cx, handle).await;
    (handle, workspace, provider)
}

fn open_with(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    remote: bool,
    path: &str,
    editor: ExternalEditor,
) {
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(
            Box::new(ExplorerAction::new(
                ExplorerId(SFTP_TAB),
                ExplorerCommand::OpenWith {
                    remote,
                    path: path.to_owned(),
                    editor,
                },
            )),
            cx,
        )
    });
}

/// Until the editor was handed a file, and the SFTP tab has caught up.
async fn wait_for_open(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    editors: &Arc<FakeEditors>,
) -> (ExternalEditor, PathBuf) {
    let opened = editors.opened.clone();
    cx.wait_for(handle.into(), Duration::from_secs(5), move |window, cx| {
        window.render_frame(cx);
        !opened
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
    })
    .await;
    editors.last_opened().expect("opened")
}

/// Save the copy from "outside" the editor, then let the watch see it.
fn save_copy(cx: &mut TestAppContext, file: &Path, text: &str) {
    std::fs::write(file, text).unwrap();
    cx.executor().advance_clock(Duration::from_secs(2));
}

async fn wait_for_writes(cx: &mut TestAppContext, provider: &FakeSftpProvider, count: usize) {
    let writes = provider.writes.clone();
    for _ in 0..200 {
        if writes.lock().unwrap().len() >= count {
            return;
        }
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
    }
    panic!("the server never got {count} writes");
}

#[gpui_kit::test]
async fn a_remote_file_opens_through_a_copy_whose_saves_go_back(cx: &mut TestAppContext) {
    let editors = Arc::new(FakeEditors::both());
    let (handle, _, provider) = open_server(cx, editors.clone()).await;

    open_with(cx, handle, true, REMOTE_FILE, ExternalEditor::VsCode);
    let (editor, copy) = wait_for_open(cx, handle, &editors).await;

    assert_eq!(editor, ExternalEditor::VsCode);
    assert_eq!(copy.file_name().unwrap().to_string_lossy(), "文件 甲.txt");
    // The copy holds what is on the server, in a folder of its own.
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), "原内容\n");
    assert!(copy.parent().is_some_and(|dir| dir != Path::new("/tmp")));

    // Saving the copy writes it back, without asking: the server's file is
    // still what was read.
    save_copy(cx, &copy, "改过\n");
    wait_for_writes(cx, &provider, 1).await;
    assert_eq!(provider.file(REMOTE_FILE), "改过\n".as_bytes());
    // And the editor is told it went.
    // And the user is told it went.
    in_frame(cx, handle, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1);
    });
}

#[gpui_kit::test]
async fn opening_the_same_file_again_goes_through_the_copy_it_has(cx: &mut TestAppContext) {
    let editors = Arc::new(FakeEditors::both());
    let (handle, _, _) = open_server(cx, editors.clone()).await;

    open_with(cx, handle, true, REMOTE_FILE, ExternalEditor::VsCode);
    let (_, first) = wait_for_open(cx, handle, &editors).await;
    std::fs::write(&first, "改了一半\n").unwrap();

    // Zed opens the same copy, not a second one of the same file.
    open_with(cx, handle, true, REMOTE_FILE, ExternalEditor::Zed);
    let (editor, second) = wait_for_open(cx, handle, &editors).await;
    assert_eq!(editor, ExternalEditor::Zed);
    assert_eq!(second, first);
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "改了一半\n");
}

#[gpui_kit::test]
async fn a_file_changed_on_the_server_is_overwritten_only_when_asked(cx: &mut TestAppContext) {
    let editors = Arc::new(FakeEditors::both());
    let (handle, _, provider) = open_server(cx, editors.clone()).await;

    open_with(cx, handle, true, REMOTE_FILE, ExternalEditor::VsCode);
    let (_, copy) = wait_for_open(cx, handle, &editors).await;
    // Someone else writes it while it is open here.
    provider.change_file(REMOTE_FILE, "别人改的\n".as_bytes());

    save_copy(cx, &copy, "本地的\n");
    let writes = provider.writes.clone();
    for _ in 0..200 {
        if !writes.lock().unwrap().is_empty() {
            break;
        }
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
    }
    // The write was refused, and the dialog names the file.
    in_frame(cx, handle, |window, _| {
        assert_eq!(
            window.find("ok").label(),
            Some("覆盖"),
            "the overwrite dialog is up"
        );
    });
    assert_eq!(provider.file(REMOTE_FILE), "别人改的\n".as_bytes());

    // 覆盖 writes it, without checking again.
    in_frame(cx, handle, |window, cx| window.click("ok", cx));
    wait_for_writes(cx, &provider, 2).await;
    assert_eq!(provider.file(REMOTE_FILE), "本地的\n".as_bytes());
}

#[gpui_kit::test]
async fn a_local_file_opens_where_it_is(cx: &mut TestAppContext) {
    let editors = Arc::new(FakeEditors::both());
    let (handle, _, _) = open_server(cx, editors.clone()).await;

    open_with(cx, handle, false, LOCAL_FILE, ExternalEditor::Zed);

    let (editor, file) = wait_for_open(cx, handle, &editors).await;
    assert_eq!(editor, ExternalEditor::Zed);
    assert_eq!(file, PathBuf::from(LOCAL_FILE));
}

#[gpui_kit::test]
async fn an_editor_that_is_not_there_says_so(cx: &mut TestAppContext) {
    // Only Zed is on this computer.
    let editors = Arc::new(FakeEditors {
        installed: vec![ExternalEditor::Zed],
        ..FakeEditors::default()
    });
    let (handle, _, _) = open_server(cx, editors.clone()).await;

    open_with(cx, handle, true, REMOTE_FILE, ExternalEditor::VsCode);

    // Once the file is read, it is said, and nothing started.
    cx.wait_for(handle.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        !window.notifications(cx).is_empty()
    })
    .await;
    in_frame(cx, handle, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1);
    });
    assert!(editors.last_opened().is_none());
}
