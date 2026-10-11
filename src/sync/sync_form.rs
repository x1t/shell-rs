//! 设置 › 云同步: the account's fields while logged out, where syncing
//! stands while logged in.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    label::Label,
    notification::Notification,
    setting::{SettingField, SettingGroup, SettingItem},
};
use gpui_kit::*;

use super::syncer::{SyncStatus, Syncer};
use crate::app::{SyncLogOut, SyncNow};
use crate::i18n::t;

/// The fields keep what was typed across the page's renders.
pub struct SyncForm {
    syncer: Entity<Syncer>,
    server: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl SyncForm {
    pub fn new(syncer: Entity<Syncer>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let account = syncer.read(cx).account().cloned();
        let server = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://sync.example.com")
                .default_value(
                    account
                        .as_ref()
                        .map(|a| a.server.clone())
                        .unwrap_or_default(),
                )
        });
        let username = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(account.map(|a| a.username).unwrap_or_default())
        });
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let subscriptions = vec![
            cx.observe(&syncer, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &password,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.log_in(window, cx);
                    }
                },
            ),
        ];
        Self {
            syncer,
            server,
            username,
            password,
            _subscriptions: subscriptions,
        }
    }

    /// Log in with what the fields hold. The password field empties once
    /// the request is on its way.
    pub fn log_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let server = self.server.read(cx).value().to_string();
        let username = self.username.read(cx).value().to_string();
        let password = self.password.read(cx).value().to_string();
        let sent = self.syncer.update(cx, |syncer, cx| {
            syncer.log_in(&server, &username, &password, cx)
        });
        match sent {
            Ok(()) => self
                .password
                .update(cx, |password, cx| password.set_value("", window, cx)),
            Err(message) => window.push_notification(Notification::error(message), cx),
        }
    }

    /// The page's groups. `focus` is the settings tab's, which actions are
    /// dispatched through.
    pub fn groups(form: &Entity<Self>, focus: &FocusHandle, cx: &App) -> Vec<SettingGroup> {
        let this = form.read(cx);
        let syncer = this.syncer.read(cx);
        let status = status_item(syncer);
        let items = match syncer.account().filter(|_| syncer.is_signed_in()) {
            Some(account) => vec![
                SettingItem::new(
                    t!("sync.settings.server"),
                    text_field(account.server.clone().into()),
                ),
                SettingItem::new(
                    t!("sync.settings.username"),
                    text_field(account.username.clone().into()),
                ),
                status,
                SettingItem::render(signed_in_actions(focus.clone())),
            ],
            None => vec![
                input_item(t!("sync.settings.server"), &this.server, "sync-server"),
                input_item(
                    t!("sync.settings.username"),
                    &this.username,
                    "sync-username",
                ),
                input_item(
                    t!("sync.settings.password"),
                    &this.password,
                    "sync-password",
                ),
                status,
                SettingItem::render(log_in_action(form.clone(), syncer)),
            ],
        };
        vec![
            SettingGroup::new()
                .title(t!("sync.settings.account"))
                .description(t!("sync.settings.account_description"))
                .items(items),
        ]
    }
}

fn text_field(text: SharedString) -> SettingField<SharedString> {
    SettingField::render(move |_, _, _| Label::new(text.clone()))
}

fn input_item(title: SharedString, state: &Entity<InputState>, id: &'static str) -> SettingItem {
    let state = state.clone();
    SettingItem::new(
        title,
        SettingField::render(move |options, _, _| {
            Input::new(&state)
                .id(id)
                .with_size(options.size())
                .w(rems(18.))
        }),
    )
}

fn status_item(syncer: &Syncer) -> SettingItem {
    let text = syncer.status_text();
    let error = syncer.has_failed();
    SettingItem::new(
        t!("sync.settings.status"),
        SettingField::render(move |_, _, cx| {
            let color = if error {
                cx.theme().danger
            } else {
                cx.theme().muted_foreground
            };
            div()
                .id("sync-status")
                .test_support()
                .aria_label(text.clone())
                .text_color(color)
                .child(text.clone())
        }),
    )
}

fn log_in_action(
    form: Entity<SyncForm>,
    syncer: &Syncer,
) -> impl Fn(&gpui_kit::component::setting::RenderOptions, &mut Window, &mut App) -> AnyElement + 'static
{
    let busy = matches!(syncer.status(), SyncStatus::Starting);
    move |options, _, _| {
        let form = form.clone();
        h_flex()
            .justify_end()
            .child(
                Button::new("sync-log-in")
                    .primary()
                    .with_size(options.size())
                    .label(t!("sync.settings.log_in"))
                    .loading(busy)
                    .disabled(busy)
                    .on_click(move |_, window, cx| {
                        form.update(cx, |form, cx| form.log_in(window, cx))
                    }),
            )
            .into_any_element()
    }
}

fn signed_in_actions(
    focus: FocusHandle,
) -> impl Fn(&gpui_kit::component::setting::RenderOptions, &mut Window, &mut App) -> AnyElement + 'static
{
    move |options, _, _| {
        let (sync, log_out) = (focus.clone(), focus.clone());
        h_flex()
            .justify_end()
            .gap_2()
            .child(
                Button::new("sync-now")
                    .outline()
                    .with_size(options.size())
                    .label(t!("sync.settings.sync_now"))
                    .on_click(move |_, window, cx| sync.dispatch_action(&SyncNow, window, cx)),
            )
            .child(
                Button::new("sync-log-out")
                    .outline()
                    .with_size(options.size())
                    .label(t!("sync.settings.log_out"))
                    .on_click(move |_, window, cx| {
                        log_out.dispatch_action(&SyncLogOut, window, cx)
                    }),
            )
            .into_any_element()
    }
}
