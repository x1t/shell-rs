//! 从 Xshell 导入: the dialog that takes Xshell's data folder, says what is
//! in it and brings in what is ticked, then what came of it.

use std::path::{Path, PathBuf};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::Button,
    checkbox::Checkbox,
    dialog::DialogButtonProps,
    form::{Field, Form},
    h_flex, v_flex,
};
use gpui_kit::*;

use super::{ImportReport, XshellData, XshellKey, import, is_xshell_folder, read_folder};
use crate::host::{HostStore, join_sentences};
use crate::i18n::{join_list, t, tn};
use crate::shared::{commit_footer, dismiss_form_error, form_error_notification};

/// Where reading the chosen folder stands.
enum Reading {
    /// No folder: none was found and none chosen.
    NoFolder,
    /// Reading it; dropping the task stops it.
    Busy {
        _task: Task<()>,
    },
    NotXshell,
    Done(XshellData),
}

pub struct XshellImportForm {
    store: Entity<HostStore>,
    folder: Option<PathBuf>,
    reading: Reading,
    hosts: bool,
    snippets: bool,
}

impl XshellImportForm {
    fn new(folder: Option<PathBuf>, store: Entity<HostStore>, cx: &mut Context<Self>) -> Self {
        let mut form = Self {
            store,
            folder: None,
            reading: Reading::NoFolder,
            hosts: true,
            snippets: true,
        };
        if let Some(folder) = folder {
            form.read(folder, cx);
        }
        form
    }

    /// Read `folder` on a background thread, opening the passwords the
    /// user ShellRS runs as saved.
    fn read(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.folder = Some(folder.clone());
        let task = cx.spawn(async move |this, cx| {
            let data = cx
                .background_executor()
                .spawn(async move {
                    is_xshell_folder(&folder)
                        .then(|| read_folder(&folder, XshellKey::current().as_ref()))
                })
                .await;
            this.update(cx, |this, cx| {
                this.reading = data.map_or(Reading::NotXshell, Reading::Done);
                cx.notify();
            })
            .ok();
        });
        self.reading = Reading::Busy { _task: task };
        cx.notify();
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let choice = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("import.xshell.choose")),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = choice.await
                && let Some(folder) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| this.read(folder, cx)).ok();
            }
        })
        .detach();
    }

    /// Import what is ticked. Returns whether the dialog may close.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let error = match &self.reading {
            Reading::NoFolder | Reading::NotXshell => Some(t!("import.xshell.choose_folder")),
            Reading::Busy { .. } => Some(t!("import.xshell.still_reading")),
            Reading::Done(data) if !self.has_chosen(data) => Some(t!("import.xshell.nothing")),
            Reading::Done(_) => None,
        };
        if let Some(error) = error {
            window.push_notification(form_error_notification(error), cx);
            return false;
        }
        let Reading::Done(data) = std::mem::replace(&mut self.reading, Reading::NoFolder) else {
            return false;
        };
        let (hosts, snippets) = (self.hosts, self.snippets);
        let report = self
            .store
            .update(cx, |store, cx| import(store, data, hosts, snippets, cx));
        // After this dialog has closed.
        window.defer(cx, move |window, cx| open_report(report, window, cx));
        true
    }

    fn has_chosen(&self, data: &XshellData) -> bool {
        (self.hosts && !data.sessions.is_empty()) || (self.snippets && command_count(data) > 0)
    }

    fn status(&self) -> Option<SharedString> {
        match self.reading {
            Reading::NoFolder => Some(t!("import.xshell.not_found")),
            Reading::Busy { .. } => Some(t!("import.xshell.reading")),
            Reading::NotXshell => Some(t!("import.xshell.not_xshell")),
            Reading::Done(_) => None,
        }
    }
}

fn command_count(data: &XshellData) -> usize {
    data.command_sets.iter().map(|set| set.commands.len()).sum()
}

impl Render for XshellImportForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (host_count, command_count) = match &self.reading {
            Reading::Done(data) => (data.sessions.len(), command_count(data)),
            _ => (0, 0),
        };
        let folder = self
            .folder
            .as_deref()
            .map(Path::display)
            .map(|path| path.to_string());
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .w_full()
            .child(
                Form::new().child(
                    Field::new().label(t!("import.xshell.folder")).child(
                        h_flex()
                            .gap_2()
                            .w_full()
                            .child(
                                div()
                                    .id("xshell-import-folder")
                                    .test_support()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_x_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .children(folder),
                            )
                            .child(
                                Button::new("xshell-import-browse")
                                    .small()
                                    .label(t!("import.xshell.browse"))
                                    .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
                            ),
                    ),
                ),
            )
            .children(self.status().map(|status| {
                div()
                    .id("xshell-import-status")
                    .test_support()
                    .aria_label(status.clone())
                    .text_sm()
                    .text_color(muted)
                    .child(status)
            }))
            .child(
                Checkbox::new("xshell-import-hosts")
                    .label(t!("import.xshell.hosts", count = host_count))
                    .small()
                    .checked(self.hosts)
                    .disabled(host_count == 0)
                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.hosts = *checked;
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("xshell-import-snippets")
                    .label(t!("import.xshell.snippets", count = command_count))
                    .small()
                    .checked(self.snippets)
                    .disabled(command_count == 0)
                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.snippets = *checked;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("import.xshell.description")),
            )
    }
}

/// Open 从 Xshell 导入, reading `folder` (the newest Xshell's, if found).
pub fn open_xshell_import_dialog(
    folder: Option<PathBuf>,
    store: Entity<HostStore>,
    window: &mut Window,
    cx: &mut App,
) {
    let form = cx.new(|cx| XshellImportForm::new(folder, store, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(t!("import.xshell.title"))
            .overlay_closable(false)
            .child(form.clone())
            .footer(commit_footer("xshell-import", t!("import.xshell.import")))
            .on_ok({
                let form = form.clone();
                move |_, window, cx| form.update(cx, |form, cx| form.commit(window, cx))
            })
            .on_close(|_, window, cx| dismiss_form_error(window, cx))
    });
}

fn open_report(report: ImportReport, window: &mut Window, cx: &mut App) {
    let description = join_sentences(report_sentences(&report)).unwrap_or_default();
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .title(t!("import.report.title"))
            .description(description.clone())
            .button_props(DialogButtonProps::default().ok_text(t!("common.close")))
    });
}

/// What the import brought in, then what of it needs the user, a sentence
/// each.
pub(super) fn report_sentences(report: &ImportReport) -> Vec<SharedString> {
    let mut sentences = Vec::new();
    let mut count = |sentence: SharedString, count: usize| {
        if count > 0 {
            sentences.push(sentence);
        }
    };
    count(tn!("import.report.hosts", report.hosts), report.hosts);
    count(
        tn!("import.report.duplicate_hosts", report.duplicate_hosts),
        report.duplicate_hosts,
    );
    count(
        tn!("import.report.passwords", report.passwords),
        report.passwords,
    );
    count(
        tn!("import.report.other_protocols", report.other_protocols),
        report.other_protocols,
    );
    count(
        tn!("import.report.snippets", report.snippets),
        report.snippets,
    );
    count(
        tn!(
            "import.report.duplicate_snippets",
            report.duplicate_snippets
        ),
        report.duplicate_snippets,
    );
    count(
        tn!("import.report.not_text", report.not_text),
        report.not_text,
    );
    if report.hosts == 0 && report.snippets == 0 {
        sentences.insert(0, t!("import.report.nothing_new"));
    }
    let list = &report.key_logins;
    if !list.is_empty() {
        sentences.push(tn!(
            "import.report.key_logins",
            list.len(),
            names = join_list(list)
        ));
    }
    let list = &report.unreadable_passwords;
    if !list.is_empty() {
        sentences.push(tn!(
            "import.report.unreadable_passwords",
            list.len(),
            names = join_list(list)
        ));
    }
    let list = &report.dropped_proxies;
    if !list.is_empty() {
        sentences.push(tn!(
            "import.report.dropped_proxies",
            list.len(),
            names = join_list(list)
        ));
    }
    let list = &report.session_variables;
    if !list.is_empty() {
        sentences.push(tn!(
            "import.report.session_variables",
            list.len(),
            names = join_list(list)
        ));
    }
    let list = &report.unreadable_files;
    if !list.is_empty() {
        sentences.push(tn!(
            "import.report.unreadable_files",
            list.len(),
            names = join_list(list)
        ));
    }
    sentences
}

#[cfg(test)]
mod tests {
    use super::{ImportReport, report_sentences};

    fn text(report: &ImportReport) -> String {
        crate::host::join_sentences(report_sentences(report))
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn the_report_says_what_came_in_and_what_needs_the_user() {
        let report = ImportReport {
            hosts: 12,
            passwords: 5,
            key_logins: vec!["db".to_owned(), "box".to_owned()],
            snippets: 70,
            duplicate_snippets: 1,
            not_text: 1,
            ..ImportReport::default()
        };
        assert_eq!(
            text(&report),
            "导入了 12 台主机。5 个密码已存入系统钥匙串。导入了 70 个命令片段。\
             1 个片段已存在，已跳过。跳过了 1 个不是发送文本的快速命令。\
             db 和 box 在 Xshell 里用密钥登录，已设成「无密码」：会依次试 SSH Agent 和 \
             ~/.ssh 里的私钥，要用别的私钥请建密钥凭据。"
        );
    }

    #[test]
    fn importing_nothing_new_says_so() {
        let report = ImportReport {
            duplicate_hosts: 3,
            ..ImportReport::default()
        };
        assert_eq!(
            text(&report),
            "没有新的主机或片段。3 台主机已存在，已跳过。"
        );
    }

    #[test]
    fn the_report_is_in_english_too() {
        crate::i18n::isolate_thread();
        crate::i18n::set_locale("en");
        let report = ImportReport {
            hosts: 1,
            unreadable_passwords: vec!["web".to_owned()],
            session_variables: vec!["a".to_owned(), "b".to_owned()],
            ..ImportReport::default()
        };
        assert_eq!(
            text(&report),
            "Imported 1 host. Couldn’t open the saved password of web; \
             it’s asked for when connecting. a and b use Xshell’s session variables \
             ($HOSTNAME, $USERNAME, $PATH), which ShellRS sends to the shell as they are."
        );
    }
}
