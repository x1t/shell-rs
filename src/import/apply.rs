//! 从 Xshell 导入 into the store: sessions become hosts in groups named
//! after their folders, quick command sets become snippet categories. What
//! is already there (the same host, the same snippet in the set's category)
//! is passed over, so importing twice adds nothing.

use gpui_kit::Context;

use super::xshell::{CommandSet, XshellData, XshellSession};
use crate::host::{AuthKind, GroupDraft, GroupId, HostDraft, HostStore, Route, SnippetDraft};

/// What an import brought in, and what of it needs the user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub hosts: usize,
    /// Hosts with this name, address, port and user were there already.
    pub duplicate_hosts: usize,
    /// Passwords written to the keychain, proxies' included.
    pub passwords: usize,
    /// Hosts that logged in with an Xshell key: they want a key credential.
    pub key_logins: Vec<String>,
    /// Hosts whose saved password this Windows user could not open.
    pub unreadable_passwords: Vec<String>,
    /// Hosts that went through a proxy ShellRS cannot take.
    pub dropped_proxies: Vec<String>,
    /// Sessions of another protocol than SSH.
    pub other_protocols: usize,
    /// Files that could not be read.
    pub unreadable_files: Vec<String>,
    pub snippets: usize,
    pub duplicate_snippets: usize,
    /// Quick commands that run a script rather than send text.
    pub not_text: usize,
    /// Snippets naming a session variable Xshell filled in.
    pub session_variables: Vec<String>,
}

/// Add what `data` holds to the store: the sessions when `hosts`, the quick
/// commands when `snippets`.
pub fn import(
    store: &mut HostStore,
    data: XshellData,
    hosts: bool,
    snippets: bool,
    cx: &mut Context<HostStore>,
) -> ImportReport {
    let mut report = ImportReport {
        unreadable_files: data.unreadable,
        ..ImportReport::default()
    };
    if hosts {
        report.other_protocols = data.other_protocols.len();
        for session in data.sessions {
            import_session(store, session, &mut report, cx);
        }
    }
    if snippets {
        for set in data.command_sets {
            import_command_set(store, set, &mut report, cx);
        }
    }
    report
}

fn import_session(
    store: &mut HostStore,
    session: XshellSession,
    report: &mut ImportReport,
    cx: &mut Context<HostStore>,
) {
    let duplicate = store.hosts().iter().any(|host| {
        host.name == session.name.as_str()
            && host.address == session.address.as_str()
            && host.port == session.port
            && host.user == session.user.as_str()
    });
    if duplicate {
        report.duplicate_hosts += 1;
        return;
    }
    let group = session.group.iter().fold(None, |parent, name| {
        Some(group_named(store, parent, name, cx))
    });
    let auth = if session.key_login {
        AuthKind::NoPassword
    } else {
        AuthKind::Password
    };
    let mut draft = HostDraft::new(
        session.name.clone(),
        session.address,
        session.port,
        session.user,
        auth,
        group,
    );
    let mut proxy_password = None;
    if let Some((settings, password)) = session.proxy {
        draft = draft.with_route(Route::Proxy(settings));
        proxy_password = password;
    }
    let id = store.insert(draft, cx);
    report.hosts += 1;
    if let Some(login) = store.login(id) {
        let secrets = [
            (Some(login.password.clone()), session.password),
            (login.route.proxy_password(), proxy_password),
        ];
        for (secret, value) in secrets {
            if let (Some(secret), Some(value)) = (secret, value) {
                store.save_secret(secret, Some(value), cx);
                report.passwords += 1;
            }
        }
    }
    let name = session.name;
    if session.key_login {
        report.key_logins.push(name.clone());
    }
    if session.password_unreadable {
        report.unreadable_passwords.push(name.clone());
    }
    if session.proxy_dropped {
        report.dropped_proxies.push(name);
    }
}

/// The group called `name` under `parent`, made when there is none.
fn group_named(
    store: &mut HostStore,
    parent: Option<GroupId>,
    name: &str,
    cx: &mut Context<HostStore>,
) -> GroupId {
    let existing = store
        .groups()
        .iter()
        .find(|group| group.parent == parent && group.name == name)
        .map(|group| group.id);
    existing.unwrap_or_else(|| store.insert_group(GroupDraft::new(name.to_owned(), parent), cx))
}

fn import_command_set(
    store: &mut HostStore,
    set: CommandSet,
    report: &mut ImportReport,
    cx: &mut Context<HostStore>,
) {
    report.not_text += set.not_text;
    let mut category = store
        .snippet_categories()
        .iter()
        .find(|category| category.name == set.name.as_str())
        .map(|category| category.id);
    for command in set.commands {
        let duplicate = category.is_some()
            && store.snippets().iter().any(|snippet| {
                snippet.category == category
                    && snippet.name == command.name.as_str()
                    && snippet.command == command.command
            });
        if duplicate {
            report.duplicate_snippets += 1;
            continue;
        }
        let category =
            *category.get_or_insert_with(|| store.insert_snippet_category(set.name.clone(), cx));
        let Ok(draft) = SnippetDraft::validated(&command.name, &command.command, Some(category))
        else {
            continue;
        };
        store.insert_snippet(draft.with_run_on_click(command.run_on_click), cx);
        report.snippets += 1;
        if command.session_variables {
            report.session_variables.push(command.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, TestAppContext};

    use super::*;
    use crate::host::{ProxyKind, ProxySettings};
    use crate::import::xshell::QuickCommand;
    use crate::secrets::{InMemorySecretStore, SecretRef, SecretStore as _};

    fn session(name: &str, group: &[&str]) -> XshellSession {
        XshellSession {
            group: group.iter().map(ToString::to_string).collect(),
            name: name.to_owned(),
            address: format!("{name}.test"),
            port: 22,
            user: "root".to_owned(),
            key_login: false,
            password: None,
            password_unreadable: false,
            proxy: None,
            proxy_dropped: false,
        }
    }

    fn data() -> XshellData {
        let mut web = session("web", &["生产", "前端"]);
        web.password = Some("hunter2".to_owned());
        let mut api = session("api", &["生产"]);
        api.proxy = Some((
            ProxySettings::new(ProxyKind::Socks5, "proxy.test", 1080).with_user("me"),
            Some("proxy-pass".to_owned()),
        ));
        api.password_unreadable = true;
        let mut box_ = session("box", &[]);
        box_.key_login = true;
        box_.proxy_dropped = true;
        let command = |name: &str, command: &str, run: bool| QuickCommand {
            name: name.to_owned(),
            command: command.to_owned(),
            run_on_click: run,
            session_variables: name == "where",
        };
        XshellData {
            sessions: vec![web, api, box_],
            other_protocols: vec!["rdp".to_owned()],
            command_sets: vec![CommandSet {
                name: "commands".to_owned(),
                commands: vec![
                    command("tmux", "tmux ls", true),
                    command("where", "echo $HOSTNAME", false),
                ],
                not_text: 2,
            }],
            unreadable: vec!["broken.xsh".to_owned()],
        }
    }

    #[gpui_kit::test]
    fn sessions_become_hosts_in_their_groups_with_their_passwords(cx: &mut TestAppContext) {
        let secrets = Arc::new(InMemorySecretStore::default());
        let store = cx.new(|_| HostStore::empty().with_secrets(secrets.clone()));

        let report = store.update(cx, |store, cx| import(store, data(), true, true, cx));
        cx.run_until_parked();

        assert_eq!(
            report,
            ImportReport {
                hosts: 3,
                passwords: 2,
                key_logins: vec!["box".to_owned()],
                unreadable_passwords: vec!["api".to_owned()],
                dropped_proxies: vec!["box".to_owned()],
                other_protocols: 1,
                unreadable_files: vec!["broken.xsh".to_owned()],
                snippets: 2,
                not_text: 2,
                session_variables: vec!["where".to_owned()],
                ..ImportReport::default()
            }
        );
        store.read_with(cx, |store, _| {
            let paths: Vec<(String, String)> = store
                .hosts()
                .iter()
                .map(|host| {
                    let group = host
                        .group
                        .map(|id| store.group_path(id))
                        .unwrap_or_default();
                    (host.name.to_string(), group)
                })
                .collect();
            assert_eq!(
                paths,
                [
                    ("web".to_owned(), "生产 / 前端".to_owned()),
                    ("api".to_owned(), "生产".to_owned()),
                    ("box".to_owned(), String::new()),
                ]
            );
            assert_eq!(store.groups().len(), 2);
            assert_eq!(store.hosts()[2].auth, AuthKind::NoPassword);
            assert!(matches!(store.hosts()[1].route, Route::Proxy(_)));

            assert_eq!(store.snippet_categories().len(), 1);
            assert_eq!(store.snippet_categories()[0].name.as_ref(), "commands");
            let snippets: Vec<(&str, bool)> = store
                .snippets()
                .iter()
                .map(|snippet| (snippet.command.as_str(), snippet.run_on_click))
                .collect();
            assert_eq!(snippets, [("tmux ls", true), ("echo $HOSTNAME", false)]);
        });
        let password = secrets
            .get(&SecretRef::password("root", "web.test", 22))
            .unwrap();
        assert_eq!(password.as_deref().map(String::as_str), Some("hunter2"));
        let proxy = secrets
            .get(&SecretRef::proxy("me", "proxy.test", 1080))
            .unwrap();
        assert_eq!(proxy.as_deref().map(String::as_str), Some("proxy-pass"));
    }

    #[gpui_kit::test]
    fn importing_again_adds_nothing(cx: &mut TestAppContext) {
        let store = cx.new(|_| HostStore::empty());
        store.update(cx, |store, cx| import(store, data(), true, true, cx));

        let again = store.update(cx, |store, cx| import(store, data(), true, true, cx));

        assert_eq!((again.hosts, again.duplicate_hosts), (0, 3));
        assert_eq!((again.snippets, again.duplicate_snippets), (0, 2));
        store.read_with(cx, |store, _| {
            assert_eq!(store.hosts().len(), 3);
            assert_eq!(store.groups().len(), 2);
            assert_eq!(store.snippets().len(), 2);
            assert_eq!(store.snippet_categories().len(), 1);
        });
    }

    #[gpui_kit::test]
    fn each_command_set_comes_in_whole_into_its_own_category(cx: &mut TestAppContext) {
        let store = cx.new(|_| HostStore::empty());
        let mut data = data();
        let mut copy = data.command_sets[0].clone();
        copy.name = "commands - 复制".to_owned();
        data.command_sets.insert(0, copy);

        let report = store.update(cx, |store, cx| import(store, data, false, true, cx));

        assert_eq!((report.snippets, report.duplicate_snippets), (4, 0));
        store.read_with(cx, |store, _| {
            let categories: Vec<&str> = store
                .snippet_categories()
                .iter()
                .map(|category| category.name.as_ref())
                .collect();
            assert_eq!(categories, ["commands - 复制", "commands"]);
            for category in store.snippet_categories() {
                assert_eq!(store.snippets_in(category.id), 2, "{}", category.name);
            }
        });
    }

    #[gpui_kit::test]
    fn only_what_was_chosen_comes_in(cx: &mut TestAppContext) {
        let store = cx.new(|_| HostStore::empty());

        let report = store.update(cx, |store, cx| import(store, data(), false, true, cx));

        assert_eq!(
            (report.hosts, report.other_protocols, report.snippets),
            (0, 0, 2)
        );
        store.read_with(cx, |store, _| {
            assert!(store.hosts().is_empty() && store.groups().is_empty());
        });
        let report = store.update(cx, |store, cx| import(store, data(), true, false, cx));
        assert_eq!((report.hosts, report.snippets, report.not_text), (3, 0, 0));
    }
}
