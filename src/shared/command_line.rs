//! A shell command on a right-sidebar card: 历史命令's line, and the tooltip
//! with the whole of it, 命令片段's too.

use gpui_kit::component::{ActiveTheme as _, tooltip::Tooltip};
use gpui_kit::*;

/// About as many characters as a command line shows in a right-sidebar
/// card at its narrowest; a longer one has the whole of it in a tooltip.
const SHOWN_CHARACTERS: usize = 32;

/// `command` on one line: a command of several lines shows its line breaks
/// as 「↵」.
pub fn one_line(command: &str) -> String {
    command.replace('\n', " ↵ ")
}

/// The tooltip that shows the whole of `command`, when its line is likely
/// cut short; `None` for one that fits, which would only say it again.
pub fn command_tooltip(
    command: &str,
    cx: &App,
) -> Option<impl Fn(&mut Window, &mut App) -> AnyView + 'static> {
    if command.chars().count() <= SHOWN_CHARACTERS && !command.contains('\n') {
        return None;
    }
    Some(whole_command_tooltip(command, cx))
}

/// The tooltip that shows the whole of `command`, for a card that does not
/// show it.
pub fn whole_command_tooltip(
    command: &str,
    cx: &App,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let command = SharedString::from(command.to_owned());
    let mono = cx.theme().mono_font_family.clone();
    move |window: &mut Window, cx: &mut App| {
        let (command, mono) = (command.clone(), mono.clone());
        Tooltip::element(move |_, _| {
            div()
                .max_w(rems(24.))
                .font_family(mono.clone())
                .text_xs()
                .child(command.clone())
        })
        .build(window, cx)
    }
}
