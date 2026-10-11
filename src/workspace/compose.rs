//! 撰写: a box under the center area whose text is typed into the SSH
//! terminal in front, or into every connected one, as Xshell's compose bar
//! does. One for the whole window, open or closed as the settings remember;
//! it shows only while an SSH terminal is on screen.
//!
//! What it sends is typed, not pasted: each line break is Enter, and one
//! follows the last line. It goes to full-screen programs too and leaves
//! what is already on a terminal's line alone.

use std::io;
use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Enter, MoveDown, MoveToEnd, MoveUp, Textarea, TextareaState},
    notification::Notification,
    v_flex,
};
use gpui_kit::*;

use crate::app::{CenterTab, SendCompose, ToggleComposeBar};
use crate::i18n::{UiLocale, t};
use crate::terminal::RemoteTerminalId;

use super::Workspace;
use super::compose_history::ComposeHistory;

/// How tall 撰写 is: three lines of text with the buttons beside them. The
/// center keeps this much free at its bottom so nothing hides under it.
pub(super) const COMPOSE_HEIGHT: Rems = rems(6.5);

/// The box and its buttons. What they send to is the workspace's to say.
pub struct ComposeBar {
    input: Entity<TextareaState>,
    /// The whole bar's, so a focus on its buttons counts as inside it.
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    /// Where the buttons and Enter dispatch from: the workspace's handlers
    /// are on its focus path even while nothing is focused.
    target: FocusHandle,
    /// What was sent before, for ↑ and ↓.
    history: ComposeHistory,
    _subscriptions: Vec<Subscription>,
}

impl ComposeBar {
    pub(super) fn new(
        workspace: WeakEntity<Workspace>,
        target: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .submit_on_enter(true)
                .placeholder(t!("workspace.compose.placeholder"))
        });
        let subscriptions = vec![
            cx.observe_global_in::<UiLocale>(window, |this, window, cx| {
                this.input.update(cx, |input, cx| {
                    input.set_placeholder(t!("workspace.compose.placeholder"), window, cx)
                });
            }),
        ];
        Self {
            input,
            focus_handle: cx.focus_handle(),
            workspace,
            target,
            history: ComposeHistory::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Keep the history in this file from now on, starting with what is in
    /// it. Without one it lasts until ShellRS quits.
    pub(super) fn keep_history_in(&mut self, path: PathBuf) {
        self.history = ComposeHistory::load(path);
    }

    pub(super) fn text(&self, cx: &App) -> SharedString {
        self.input.read(cx).value()
    }

    /// What was in the box went out: remember it, and empty the box. Says
    /// why when the history could not be written.
    pub(super) fn sent(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.set_text("", window, cx);
        self.history.push(text)
    }

    fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text.to_owned(), window, cx));
    }

    /// ↑ on the first line, ↓ on the last, go through what was sent; the
    /// arrows move the caret anywhere else. Says whether it took the key.
    fn recall(&mut self, older: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (text, caret) = {
            let input = self.input.read(cx);
            (input.value(), input.cursor())
        };
        let (before, after) = text.split_at(caret.min(text.len()));
        let entry = if older && !before.contains('\n') {
            self.history.older(&text).map(str::to_owned)
        } else if !older && !after.contains('\n') {
            self.history.newer().map(str::to_owned)
        } else {
            None
        };
        let Some(entry) = entry else {
            return false;
        };
        self.set_text(&entry, window, cx);
        // At its end, to go on writing, as a shell leaves it.
        window.defer(cx, |window, cx| {
            window.dispatch_action(Box::new(MoveToEnd), cx)
        });
        true
    }

    /// The box's focus, which opening 撰写 moves into it.
    pub(super) fn input_focus(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    pub(super) fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
    }
}

impl Render for ComposeBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (front, all) = self
            .workspace
            .upgrade()
            .map(|workspace| {
                let workspace = workspace.read(cx);
                let all = workspace.compose_targets(true, cx).len();
                (!workspace.compose_targets(false, cx).is_empty(), all)
            })
            .unwrap_or((false, 0));
        let theme = cx.theme();
        let send = |all: bool| {
            let target = self.target.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                target.dispatch_action(&SendCompose { all }, window, cx)
            }
        };
        let close = self.target.clone();
        h_flex()
            .id("compose")
            .test_support()
            .track_focus(&self.focus_handle)
            // The box passes a plain Enter on rather than taking it, and
            // one nobody takes ends up typed into the box as a line break.
            // Shift+Enter it keeps, as a new line.
            // Before the box moves the caret with them.
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if this.recall(true, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if this.recall(false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &Enter, window, cx| {
                // Once this bar is no longer being updated: sending empties
                // its box.
                let target = this.target.clone();
                window.defer(cx, move |window, cx| {
                    target.dispatch_action(&SendCompose { all: false }, window, cx)
                });
            }))
            .size_full()
            .items_stretch()
            .gap_2()
            .p_2()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                div()
                    .id("compose-input")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .child(Textarea::new(&self.input).h_full().text_sm()),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .child(
                        Button::new("compose-send")
                            .primary()
                            .small()
                            .label(t!("workspace.compose.send"))
                            .disabled(!front)
                            .on_click(send(false)),
                    )
                    .child(
                        Button::new("compose-send-all")
                            .outline()
                            .small()
                            .label(t!("workspace.compose.send_all", count = all))
                            .disabled(all == 0)
                            .on_click(send(true)),
                    )
                    .child(
                        Button::new("compose-close")
                            .ghost()
                            .small()
                            .label(t!("workspace.compose.close"))
                            .tooltip_with_action(
                                t!("workspace.compose.close"),
                                &ToggleComposeBar,
                                None,
                            )
                            .on_click(move |_, window, cx| {
                                close.dispatch_action(&ToggleComposeBar, window, cx)
                            }),
                    ),
            )
    }
}

impl Workspace {
    /// The SSH terminals 撰写 types into, by id: the one the right sidebar
    /// works on, or with `all` every one, as long as they take input.
    pub(super) fn compose_targets(&self, all: bool, cx: &App) -> Vec<RemoteTerminalId> {
        let accepts = |id: &RemoteTerminalId| {
            self.terminals
                .get(id)
                .is_some_and(|panel| panel.read(cx).lifecycle(cx).accepts_input())
        };
        if !all {
            return self
                .tool_terminal(cx)
                .map(|terminal| terminal.id)
                .filter(accepts)
                .into_iter()
                .collect();
        }
        let mut ids: Vec<RemoteTerminalId> =
            self.terminals.keys().copied().filter(accepts).collect();
        ids.sort_by_key(|id| id.0);
        ids
    }

    pub(super) fn on_toggle_compose_bar(
        &mut self,
        _: &ToggleComposeBar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = !self.settings().read(cx).settings().compose.open;
        self.settings().update(cx, |settings, cx| {
            settings.update(|settings| settings.compose.open = open, cx)
        });
        let focus_inside = self.compose.read(cx).contains_focus(window, cx);
        if self.sync_compose(cx) {
            let focus = self.compose.read(cx).input_focus(cx);
            window.focus(&focus, cx);
        } else if focus_inside {
            self.focus_center(window, cx);
        }
    }

    /// Put 撰写 on screen under the center while it is open and an SSH
    /// terminal is on screen, as the right sidebar comes and goes; take it
    /// away otherwise. Says whether it shows.
    pub(super) fn sync_compose(&self, cx: &mut App) -> bool {
        let shows = self.settings().read(cx).settings().compose.open
            && self
                .visible_center_tabs(cx)
                .iter()
                .any(|tab| matches!(tab, CenterTab::Terminal(_)));
        self.skin
            .set_compose(shows.then(|| AnyView::from(self.compose.clone())), cx);
        shows
    }

    pub(super) fn on_send_compose(
        &mut self,
        action: &SendCompose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.compose.read(cx).text(cx);
        if text.trim().is_empty() {
            return;
        }
        let mut sent = false;
        let mut failures = Vec::new();
        for id in self.compose_targets(action.all, cx) {
            let Some(panel) = self.terminals.get(&id) else {
                continue;
            };
            let view = panel.read(cx).terminal().clone();
            match view.update(cx, |view, cx| view.type_text(&text, cx)) {
                Ok(()) => sent = true,
                Err(error) => failures.push(error),
            }
        }
        if sent
            && let Err(error) = self
                .compose
                .update(cx, |compose, cx| compose.sent(&text, window, cx))
        {
            let message = t!(
                "workspace.compose.history_unsaved",
                error = error.to_string()
            );
            window.push_notification(Notification::error(message), cx);
        }
        if let Some(error) = failures.first() {
            let message = t!("workspace.compose.send_failed", error = error);
            window.push_notification(Notification::error(message), cx);
        }
    }
}
