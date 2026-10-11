//! 撰写: the box under the center whose text is typed into the SSH terminal
//! in front, or into every connected one.

use crate::support::*;

/// The seeded workspace with web-01's terminal in front, both SSH
/// terminals' shells running, and the keyboard in 撰写, which is open
/// from the start.
async fn open_compose(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Root>,
    Entity<Workspace>,
    Arc<FakeTerminalFactory>,
) {
    let factory = Arc::new(FakeTerminalFactory::default());
    let (handle, workspace) =
        open_workspace_with_remote_factory(cx, HostStore::seed(), factory.clone());
    in_frame(cx, handle, |window, cx| {
        window.activate_window();
        window.click(("terminal-tab", INITIAL_WEB_TERMINAL), cx)
    });
    for terminal in [INITIAL_WEB_TERMINAL, INITIAL_STAGING_TERMINAL] {
        let workspace = workspace.clone();
        cx.wait_for(handle.into(), Duration::from_secs(5), move |_, cx| {
            remote_lifecycle(&workspace, RemoteTerminalId(terminal), cx).accepts_input()
        })
        .await;
    }
    in_frame(cx, handle, |window, cx| window.click("compose-input", cx));
    (handle, workspace, factory)
}

fn compose_open(workspace: &Entity<Workspace>, cx: &mut TestAppContext) -> bool {
    cx.update(|cx| {
        workspace
            .read(cx)
            .settings()
            .read(cx)
            .settings()
            .compose
            .open
    })
}

fn sent(factory: &FakeTerminalFactory, before: usize) -> String {
    factory.written_text()[before..].to_owned()
}

#[gpui_kit::test]
async fn enter_types_the_text_into_the_terminal_in_front(cx: &mut TestAppContext) {
    let (handle, workspace, factory) = open_compose(cx).await;
    assert!(compose_open(&workspace, cx));
    in_frame(cx, handle, |window, _| {
        assert!(window.find("compose").visible());
        assert!(enabled(&window.find("compose-send")));
        assert!(enabled(&window.find("compose-send-all")));
        assert_eq!(
            window.find("compose-send-all").label(),
            Some("发送到全部（2）")
        );
    });

    let before = factory.written_text().len();
    in_frame(cx, handle, |window, cx| {
        window.input("uptime", cx);
        window.press("enter", cx);
    });
    // Typed, not pasted: no bracketed paste, nothing cleared first.
    assert_eq!(sent(&factory, before), "uptime\r");

    // The box is empty again and keeps the keyboard, so the next line can
    // follow; Shift+Enter starts a new line instead of sending.
    let before = factory.written_text().len();
    in_frame(cx, handle, |window, cx| {
        window.input("cd /tmp", cx);
        window.press("shift-enter", cx);
        window.input("ls", cx);
    });
    assert_eq!(sent(&factory, before), "");
    in_frame(cx, handle, |window, cx| window.press("enter", cx));
    assert_eq!(sent(&factory, before), "cd /tmp\rls\r");
}

#[gpui_kit::test]
async fn send_to_all_types_into_every_connected_terminal(cx: &mut TestAppContext) {
    let (handle, _, factory) = open_compose(cx).await;
    let before = factory.written_text().len();
    in_frame(cx, handle, |window, cx| {
        window.input("df -h", cx);
        window.click("compose-send-all", cx);
    });
    // web-01's and staging-api's terminals, one after the other.
    assert_eq!(sent(&factory, before), "df -h\rdf -h\r");

    // Nothing to send sends nothing.
    let before = factory.written_text().len();
    in_frame(cx, handle, |window, cx| {
        window.click("compose-send-all", cx)
    });
    assert_eq!(sent(&factory, before), "");
}

#[gpui_kit::test]
async fn compose_goes_with_the_ssh_terminals_on_screen(cx: &mut TestAppContext) {
    let (handle, workspace, _) = open_compose(cx).await;
    // The settings take the center: no SSH terminal shows, nor 撰写.
    in_frame(cx, handle, |window, cx| window.click("open-settings", cx));
    in_frame(cx, handle, |window, _| {
        assert!(window.try_find("compose").is_none());
    });
    // Still open: back with a terminal, back on screen.
    assert!(compose_open(&workspace, cx));
    in_frame(cx, handle, |window, cx| {
        window.click(("terminal-tab", INITIAL_WEB_TERMINAL), cx)
    });
    in_frame(cx, handle, |window, _| {
        assert!(window.find("compose").visible());
    });
}

#[gpui_kit::test]
async fn hiding_compose_hands_the_keyboard_back_and_is_remembered(cx: &mut TestAppContext) {
    let (handle, workspace, factory) = open_compose(cx).await;
    in_frame(cx, handle, |window, cx| window.click("compose-close", cx));

    assert!(!compose_open(&workspace, cx));
    in_frame(cx, handle, |window, _| {
        assert!(window.try_find("compose").is_none());
    });
    // What is typed next goes to the terminal in front.
    let before = factory.written_text().len();
    in_frame(cx, handle, |window, cx| window.input("q", cx));
    assert_eq!(sent(&factory, before), "q");

    // The switch under the tools opens it again, with the keyboard in it.
    in_frame(cx, handle, |window, cx| window.click("toggle-compose", cx));
    assert!(compose_open(&workspace, cx));
    in_frame(cx, handle, |window, _| {
        assert!(window.find("compose").visible());
        assert_eq!(window.find("toggle-compose").checked(), Some(true));
    });
}

#[gpui_kit::test]
async fn up_and_down_bring_back_what_was_sent(cx: &mut TestAppContext) {
    let (handle, _, factory) = open_compose(cx).await;
    for text in ["uptime", "df -h"] {
        in_frame(cx, handle, |window, cx| {
            window.input(text, cx);
            window.press("enter", cx);
        });
    }

    // ↑ brings back the last one, ↑ again the one before.
    let before = factory.written_text().len();
    in_frame(cx, handle, |window, cx| window.press("up", cx));
    in_frame(cx, handle, |window, cx| window.press("enter", cx));
    assert_eq!(sent(&factory, before), "df -h\r");
    let before = factory.written_text().len();
    in_frame(cx, handle, |window, cx| window.press("up", cx));
    in_frame(cx, handle, |window, cx| window.press("up", cx));
    in_frame(cx, handle, |window, cx| window.press("enter", cx));
    assert_eq!(sent(&factory, before), "uptime\r");

    // ↓ past the newest comes back to what was being written.
    let before = factory.written_text().len();
    in_frame(cx, handle, |window, cx| {
        window.input("ls", cx);
        window.press("up", cx);
    });
    in_frame(cx, handle, |window, cx| window.press("down", cx));
    in_frame(cx, handle, |window, cx| window.press("enter", cx));
    assert_eq!(sent(&factory, before), "ls\r");

    // What a recalled entry is left at can be added to.
    let before = factory.written_text().len();
    in_frame(cx, handle, |window, cx| window.press("up", cx));
    in_frame(cx, handle, |window, cx| {
        window.input(" -a", cx);
        window.press("enter", cx);
    });
    assert_eq!(sent(&factory, before), "ls -a\r");
}
