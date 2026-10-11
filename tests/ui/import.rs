//! 从 Xshell 导入 (设置 › 应用 › 导入): the dialog reads an Xshell data
//! folder, made here the way Xshell writes one, and brings in its sessions
//! and quick commands.

use std::fs;
use std::path::Path;

use crate::support::*;

/// Write `text` as Xshell does: UTF-16 with a byte order mark, CRLF.
fn write_utf16(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.replace('\n', "\r\n").encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

fn session(host: &str, auth: &str) -> String {
    format!(
        "[CONNECTION]\nPort=22\nHost={host}\nProtocol=SSH\n\
         [CONNECTION:AUTHENTICATION]\nAuthMethodList={auth}\nUserName=root\n"
    )
}

/// Two sessions, one of them in 生产, and a set of two quick commands.
fn xshell_folder() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let xshell = root.path().join("Xshell");
    write_utf16(
        &xshell.join("Sessions/api.xsh"),
        &session("10.0.0.1", "01,10"),
    );
    write_utf16(
        &xshell.join("Sessions/生产/db.xsh"),
        &session("10.0.0.2", "00,11"),
    );
    write_utf16(
        &xshell.join("QuickButton Files/commands.qbl"),
        "[QuickButton]\n\
         Button_1_Name=tmux\nButton_1_Action=tmux ls\\\\r\nButton_1_Type=1\n\
         Button_2_Name=disk\nButton_2_Action=df -h\nButton_2_Type=1\n",
    );
    root
}

/// 设置 › 应用 › 从 Xshell 导入, starting in `folder`.
fn open_import(
    cx: &mut TestAppContext,
    folder: Option<&Path>,
) -> (WindowHandle<Root>, Entity<Workspace>) {
    let (handle, workspace) = open_workspace_with_store(cx, HostStore::empty());
    workspace.update(cx, |workspace, _| {
        workspace.set_xshell_folder(folder.map(Path::to_path_buf))
    });
    in_frame(cx, handle, |window, cx| window.click("open-settings", cx));
    // 应用 comes right before 关于.
    in_frame(cx, handle, |window, cx| {
        window.within("settings").click("0-5", cx)
    });
    in_frame(cx, handle, |window, cx| window.click("import-xshell", cx));
    (handle, workspace)
}

/// Wait for the folder to be read: its line about it goes.
async fn wait_until_read(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.wait_for(handle.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        window.try_find("xshell-import-status").is_none()
    })
    .await;
}

fn status(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> Option<String> {
    in_frame(cx, handle, |window, _| {
        window
            .find("xshell-import-status")
            .label()
            .map(str::to_owned)
    })
}

fn hosts(workspace: &Entity<Workspace>, cx: &mut TestAppContext) -> Vec<(String, String)> {
    workspace.read_with(cx, |workspace, cx| {
        let store = workspace.store().read(cx);
        store
            .hosts()
            .iter()
            .map(|host| {
                let group = host
                    .group
                    .map(|id| store.group_path(id))
                    .unwrap_or_default();
                (host.name.to_string(), group)
            })
            .collect()
    })
}

fn snippets(workspace: &Entity<Workspace>, cx: &mut TestAppContext) -> Vec<(String, bool)> {
    workspace.read_with(cx, |workspace, cx| {
        workspace
            .store()
            .read(cx)
            .snippets()
            .iter()
            .map(|snippet| (snippet.command.clone(), snippet.run_on_click))
            .collect()
    })
}

/// Import, then close the report.
async fn import(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    in_frame(cx, handle, |window, cx| window.click("xshell-import", cx));
    cx.wait_for(handle.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        window.try_find("ok").is_some()
    })
    .await;
    in_frame(cx, handle, |window, cx| window.click("ok", cx));
}

#[gpui_kit::test]
async fn sessions_and_quick_commands_come_in_from_xshells_folder(cx: &mut TestAppContext) {
    let root = xshell_folder();
    let (handle, workspace) = open_import(cx, Some(&root.path().join("Xshell")));
    wait_until_read(cx, handle).await;
    in_frame(cx, handle, |window, _| {
        assert_eq!(window.find("xshell-import-hosts").checked(), Some(true));
        assert_eq!(window.find("xshell-import-snippets").checked(), Some(true));
    });

    import(cx, handle).await;

    assert_eq!(
        hosts(&workspace, cx),
        [
            ("api".to_owned(), String::new()),
            ("db".to_owned(), "生产".to_owned())
        ]
    );
    let auth: Vec<AuthKind> = workspace.read_with(cx, |workspace, cx| {
        workspace
            .store()
            .read(cx)
            .hosts()
            .iter()
            .map(|host| host.auth)
            .collect()
    });
    assert_eq!(auth, [AuthKind::Password, AuthKind::NoPassword]);
    assert_eq!(
        snippets(&workspace, cx),
        [("tmux ls".to_owned(), true), ("df -h".to_owned(), false)]
    );
    in_frame(cx, handle, |window, _| {
        assert!(
            window.try_find("xshell-import-hosts").is_none(),
            "the dialog closed"
        );
    });
}

#[gpui_kit::test]
async fn unticked_quick_commands_stay_behind_and_a_second_import_adds_nothing(
    cx: &mut TestAppContext,
) {
    let root = xshell_folder();
    let (handle, workspace) = open_import(cx, Some(&root.path().join("Xshell")));
    wait_until_read(cx, handle).await;
    in_frame(cx, handle, |window, cx| {
        window.click("xshell-import-snippets", cx)
    });
    import(cx, handle).await;
    assert_eq!(hosts(&workspace, cx).len(), 2);
    assert!(snippets(&workspace, cx).is_empty());

    in_frame(cx, handle, |window, cx| window.click("import-xshell", cx));
    wait_until_read(cx, handle).await;
    import(cx, handle).await;

    assert_eq!(hosts(&workspace, cx).len(), 2);
    assert_eq!(snippets(&workspace, cx).len(), 2);
    let groups = workspace.read_with(cx, |workspace, cx| {
        workspace.store().read(cx).groups().len()
    });
    assert_eq!(groups, 1);
}

#[gpui_kit::test]
async fn a_folder_that_is_not_xshells_says_so_and_imports_nothing(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (handle, workspace) = open_import(cx, Some(root.path()));
    cx.wait_for(handle.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        window
            .try_find("xshell-import-status")
            .is_some_and(|status| status.label() != Some("正在读取…"))
    })
    .await;
    assert_eq!(
        status(cx, handle).as_deref(),
        Some("这个文件夹里没有 Xshell 的会话和快速命令。")
    );

    in_frame(cx, handle, |window, cx| window.click("xshell-import", cx));

    in_frame(cx, handle, |window, _| {
        assert_eq!(
            window.find("form-error").label(),
            Some("请选择 Xshell 数据文件夹")
        );
        assert!(
            window.try_find("xshell-import-hosts").is_some(),
            "still open"
        );
    });
    assert!(hosts(&workspace, cx).is_empty());
}

#[gpui_kit::test]
fn without_xshell_the_dialog_asks_for_its_folder(cx: &mut TestAppContext) {
    let (handle, _) = open_import(cx, None);
    assert_eq!(
        status(cx, handle).as_deref(),
        Some(
            "没有找到 Xshell 的数据文件夹，请选择（通常是 文档\\NetSarang Computer\\8\\Xshell）。"
        )
    );
    in_frame(cx, handle, |window, _| {
        assert_eq!(window.find("xshell-import-hosts").checked(), Some(true));
        assert!(window.try_find("xshell-import-browse").is_some());
    });
}
