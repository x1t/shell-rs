//! 云同步: logging in from 设置 › 云同步, and what one device changes
//! reaching the other through a shell-rs-server kept in memory.

use crate::support::*;
use shellrs::sync::{MemorySyncServer, SyncServices, SyncStatus, Syncer};

const SERVER: &str = "https://sync.test";
const PASSWORD: &str = "correct horse battery";

fn server() -> Arc<MemorySyncServer> {
    Arc::new(MemorySyncServer::with_user("me", PASSWORD))
}

fn services(server: &Arc<MemorySyncServer>) -> SyncServices {
    SyncServices::with_api(server.clone(), Arc::new(InMemorySecretStore::default()))
}

/// The seeded workspace, syncing through `server`.
fn open_device(
    cx: &mut TestAppContext,
    server: &Arc<MemorySyncServer>,
) -> (WindowHandle<Root>, Entity<Workspace>) {
    let (handle, workspace) = open_workspace(cx);
    let services = services(server);
    cx.update_window(handle.into(), |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.set_sync_services(services, window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    (handle, workspace)
}

/// Another computer: a store and settings of its own, and no window.
fn other_device(
    cx: &mut TestAppContext,
    server: &Arc<MemorySyncServer>,
) -> (Entity<HostStore>, Entity<Syncer>) {
    let store = cx.new(|_| HostStore::empty());
    let settings = cx.new(|_| settings_store());
    let syncer = cx.new(|cx| Syncer::new(store.clone(), settings, cx));
    let services = services(server);
    syncer.update(cx, |syncer, cx| syncer.set_services(services, cx));
    (store, syncer)
}

/// Until the worker has answered and the syncer is no longer busy, what
/// arrives carried out as the workspace does. The worker is a real thread,
/// so real time passes too.
fn settle(cx: &mut TestAppContext, syncer: &Entity<Syncer>) {
    for _ in 0..500 {
        std::thread::sleep(Duration::from_millis(5));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        syncer.update(cx, |syncer, cx| syncer.apply_arrival(cx));
        cx.run_until_parked();
        let busy = syncer.read_with(cx, |syncer, _| {
            matches!(syncer.status(), SyncStatus::Starting | SyncStatus::Syncing)
        });
        if !busy {
            return;
        }
    }
    let status = syncer.read_with(cx, |syncer, _| syncer.status().clone());
    panic!("the syncer never settled: {status:?}");
}

/// A round on the other computer.
fn sync_other(cx: &mut TestAppContext, syncer: &Entity<Syncer>) {
    syncer.update(cx, |syncer, cx| syncer.sync_now(cx));
    settle(cx, syncer);
}

fn log_in_other(cx: &mut TestAppContext, syncer: &Entity<Syncer>) {
    syncer
        .update(cx, |syncer, cx| syncer.log_in(SERVER, "me", PASSWORD, cx))
        .unwrap();
    settle(cx, syncer);
}

/// Open 设置 › 云同步, the seventh category.
fn open_sync_settings(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    in_frame(cx, handle, |window, cx| window.click("open-settings", cx));
    in_frame(cx, handle, |window, cx| {
        window.within("settings").click("0-6", cx)
    });
}

fn log_in_from_settings(cx: &mut TestAppContext, handle: WindowHandle<Root>, password: &str) {
    open_sync_settings(cx, handle);
    in_frame(cx, handle, |window, cx| {
        window.click("sync-server", cx);
        window.input(SERVER, cx);
        window.click("sync-username", cx);
        window.input("me", cx);
        window.click("sync-password", cx);
        window.input(password, cx);
        window.click("sync-log-in", cx);
    });
}

fn status_text(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> String {
    in_frame(cx, handle, |window, _| {
        window
            .find("sync-status")
            .label()
            .unwrap_or_default()
            .to_string()
    })
}

#[gpui_kit::test]
fn logging_in_sends_this_computers_hosts_to_the_other(cx: &mut TestAppContext) {
    let server = server();
    let (handle, workspace) = open_device(cx, &server);
    log_in_from_settings(cx, handle, PASSWORD);
    let syncer = workspace.read_with(cx, |workspace, _| workspace.syncer().clone());
    settle(cx, &syncer);

    assert!(status_text(cx, handle).starts_with("已同步"));
    // 3 groups, 6 hosts and the settings, sealed.
    assert_eq!(server.live_records("me"), 10);
    in_frame(cx, handle, |window, _| {
        assert!(window.try_find("sync-password").is_none());
        assert!(window.find("sync-now").visible());
    });

    let (store, other) = other_device(cx, &server);
    log_in_other(cx, &other);
    store.read_with(cx, |store, _| {
        let mut names: Vec<_> = store.hosts().iter().map(|h| h.name.to_string()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "db-01",
                "dev-box",
                "qa-runner",
                "staging-api",
                "web-01",
                "web-02"
            ]
        );
        let web = store.hosts().iter().find(|h| h.name == "web-01").unwrap();
        assert_eq!(store.group(web.group.unwrap()).unwrap().name, "生产");
    });
}

#[gpui_kit::test]
fn an_edit_on_the_other_computer_arrives_here(cx: &mut TestAppContext) {
    let server = server();
    let (handle, workspace) = open_device(cx, &server);
    log_in_from_settings(cx, handle, PASSWORD);
    let syncer = workspace.read_with(cx, |workspace, _| workspace.syncer().clone());
    settle(cx, &syncer);
    let (other_store, other) = other_device(cx, &server);
    log_in_other(cx, &other);

    other_store.update(cx, |store, cx| {
        let db = store
            .hosts()
            .iter()
            .find(|h| h.name == "db-01")
            .unwrap()
            .clone();
        let mut draft = db.draft();
        draft.name = "db-主库".into();
        store.update(db.id, draft, cx);
        store.insert_snippet_category("Docker", cx);
    });
    sync_other(cx, &other);

    // 立即同步 in the title bar, beside the credentials.
    in_frame(cx, handle, |window, cx| window.click("sync-now-title", cx));
    settle(cx, &syncer);
    workspace.read_with(cx, |workspace, cx| {
        let store = workspace.store().read(cx);
        assert_eq!(store.host(HostId(DB_01)).unwrap().name, "db-主库");
        assert_eq!(store.snippet_categories()[0].name, "Docker");
    });
}

#[gpui_kit::test]
fn a_host_deleted_on_the_other_computer_closes_its_tabs_here(cx: &mut TestAppContext) {
    let server = server();
    let (handle, workspace) = open_device(cx, &server);
    log_in_from_settings(cx, handle, PASSWORD);
    let syncer = workspace.read_with(cx, |workspace, _| workspace.syncer().clone());
    settle(cx, &syncer);
    let (other_store, other) = other_device(cx, &server);
    log_in_other(cx, &other);

    other_store.update(cx, |store, cx| {
        let web = store
            .hosts()
            .iter()
            .find(|h| h.name == "web-01")
            .unwrap()
            .id;
        store.remove(web, cx);
    });
    sync_other(cx, &other);

    in_frame(cx, handle, |window, cx| window.click("sync-now", cx));
    settle(cx, &syncer);
    in_frame(cx, handle, |window, _| {
        assert!(
            window
                .try_find(("terminal-tab", INITIAL_WEB_TERMINAL))
                .is_none()
        );
    });
    workspace.read_with(cx, |workspace, cx| {
        assert!(workspace.store().read(cx).host(HostId(WEB_01)).is_none());
    });
}

#[gpui_kit::test]
fn a_wrong_password_says_so_and_keeps_the_fields(cx: &mut TestAppContext) {
    let server = server();
    let (handle, workspace) = open_device(cx, &server);
    log_in_from_settings(cx, handle, "not the password");
    let syncer = workspace.read_with(cx, |workspace, _| workspace.syncer().clone());
    settle(cx, &syncer);

    assert_eq!(status_text(cx, handle), "用户名或密码不对");
    in_frame(cx, handle, |window, _| {
        assert!(window.find("sync-password").visible());
        // Not logged in: nothing to sync from the title bar.
        assert!(window.try_find("sync-now-title").is_none());
    });
    assert_eq!(server.live_records("me"), 0);
}

#[gpui_kit::test]
fn an_address_that_is_not_https_is_refused_before_anything_is_sent(cx: &mut TestAppContext) {
    let server = server();
    let (handle, _) = open_device(cx, &server);
    open_sync_settings(cx, handle);
    in_frame(cx, handle, |window, cx| {
        window.click("sync-server", cx);
        window.input("http://sync.test", cx);
        window.click("sync-username", cx);
        window.input("me", cx);
        window.click("sync-password", cx);
        window.input(PASSWORD, cx);
        window.click("sync-log-in", cx);
    });
    in_frame(cx, handle, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1);
    });
    assert_eq!(status_text(cx, handle), "未登录");
}

#[gpui_kit::test]
fn logging_out_brings_the_fields_back_and_keeps_the_hosts(cx: &mut TestAppContext) {
    let server = server();
    let (handle, workspace) = open_device(cx, &server);
    log_in_from_settings(cx, handle, PASSWORD);
    let syncer = workspace.read_with(cx, |workspace, _| workspace.syncer().clone());
    settle(cx, &syncer);

    in_frame(cx, handle, |window, cx| window.click("sync-log-out", cx));
    settle(cx, &syncer);
    assert_eq!(status_text(cx, handle), "未登录");
    in_frame(cx, handle, |window, _| {
        assert!(window.find("sync-password").visible());
    });
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.store().read(cx).hosts().len(), 6);
    });
}

#[gpui_kit::test]
fn a_change_here_goes_up_by_itself(cx: &mut TestAppContext) {
    let server = server();
    let (handle, workspace) = open_device(cx, &server);
    log_in_from_settings(cx, handle, PASSWORD);
    let syncer = workspace.read_with(cx, |workspace, _| workspace.syncer().clone());
    settle(cx, &syncer);
    let (other_store, other) = other_device(cx, &server);
    log_in_other(cx, &other);

    // A host edited and a snippet made here, nothing pressed.
    workspace.update(cx, |workspace, cx| {
        workspace.store().update(cx, |store, cx| {
            let mut draft = store.host(HostId(WEB_02)).unwrap().draft();
            draft.name = "web-备用".into();
            store.update(HostId(WEB_02), draft, cx);
            store.insert_snippet(shellrs::host::SnippetDraft::new("磁盘", "df -h", None), cx);
        })
    });
    cx.executor().advance_clock(Duration::from_secs(3));
    settle(cx, &syncer);

    sync_other(cx, &other);
    other_store.read_with(cx, |store, _| {
        assert!(store.hosts().iter().any(|host| host.name == "web-备用"));
        assert_eq!(store.snippets()[0].command, "df -h");
    });
}
