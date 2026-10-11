use std::sync::Arc;

use gpui_kit::{AppContext as _, TestAppContext};

use super::*;
use crate::host::BookmarkSide;
use crate::secrets::{InMemorySecretStore, SecretStore as _};
use crate::sync::record::{CategoryRecord, KNOWN_HOSTS_ID, SETTINGS_ID};

fn host(name: &str, group: Option<&str>, route: RouteRecord) -> Payload {
    Payload::Host(Box::new(HostRecord {
        name: name.into(),
        address: format!("{name}.test"),
        port: 22,
        auth: "password".into(),
        user: "root".into(),
        credential: None,
        route,
        group: group.map(Into::into),
        notes: "备注".into(),
        sort_order: 2,
        password: Some(SecretText::new(format!("{name} password"))),
        proxy_password: None,
        local_bookmarks: vec![],
        remote_bookmarks: vec!["/srv".into()],
    }))
}

fn group(name: &str, parent: Option<&str>) -> Payload {
    Payload::Group(GroupRecord {
        name: name.into(),
        parent: parent.map(Into::into),
        sort_order: 0,
    })
}

struct Fixture {
    store: Entity<HostStore>,
    settings: Entity<SettingsStore>,
    secrets: Arc<InMemorySecretStore>,
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(cx: &mut TestAppContext) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(InMemorySecretStore::default());
        let store = HostStore::empty()
            .with_secrets(secrets.clone())
            .with_key_dir(dir.path().join("keys"));
        Self {
            store: cx.new(|_| store),
            settings: cx.new(|_| SettingsStore::in_memory()),
            secrets,
            dir,
        }
    }

    fn apply(&self, apply: Vec<(String, Option<Payload>)>, cx: &mut TestAppContext) {
        let known_hosts = self.dir.path().join("known_hosts");
        cx.update(|cx| {
            let targets = Targets {
                store: &self.store,
                settings: &self.settings,
                known_hosts: Some(&known_hosts),
            };
            super::apply(Changes::new(apply), &targets, cx)
        });
        cx.run_until_parked();
    }

    fn secret(&self, secret: &SecretRef) -> Option<String> {
        self.secrets
            .get(secret)
            .unwrap()
            .map(|value| value.to_string())
    }
}

fn change(id: &str, payload: Payload) -> (String, Option<Payload>) {
    (id.into(), Some(payload))
}

#[gpui_kit::test]
fn a_round_from_another_device_lands_whatever_its_order(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    fixture.apply(
        vec![
            // A host behind one that comes later, in a subgroup that comes
            // before its parent.
            change(
                "H1",
                host(
                    "behind",
                    Some("G2"),
                    RouteRecord::Jump {
                        hops: vec![Some("H2".into()), None],
                    },
                ),
            ),
            change("G2", group("web", Some("G1"))),
            change("G1", group("生产", None)),
            change("H2", host("bastion", None, RouteRecord::Direct)),
            change(
                "K1",
                Payload::SnippetCategory(CategoryRecord {
                    name: "Docker".into(),
                }),
            ),
            change(
                "S1",
                Payload::Snippet(SnippetRecord {
                    category: Some("K1".into()),
                    name: "ps".into(),
                    command: "docker ps".into(),
                    run_on_click: true,
                    sort_order: 0,
                }),
            ),
            change(
                "F1",
                Payload::Forward(ForwardRecord {
                    host: "H2".into(),
                    name: "socks".into(),
                    kind: "dynamic".into(),
                    bind_host: "127.0.0.1".into(),
                    bind_port: 1080,
                    target_host: None,
                    target_port: None,
                    auto_start: true,
                    sort_order: 0,
                }),
            ),
            change(
                SETTINGS_ID,
                Payload::Settings {
                    settings: serde_json::json!({"compose": {"open": false}}),
                },
            ),
            change(
                KNOWN_HOSTS_ID,
                Payload::KnownHosts {
                    lines: vec!["bastion.test ssh-ed25519 AAAA".into()],
                },
            ),
        ],
        cx,
    );

    fixture.store.read_with(cx, |store, _| {
        let behind = store.host(store.host_by_sync_id("H1").unwrap()).unwrap();
        let bastion = store.host_by_sync_id("H2").unwrap();
        let web = store.group_by_sync_id("G2").unwrap();
        assert_eq!(behind.route, Route::Jump(vec![Some(bastion), None]));
        assert_eq!(behind.group, Some(web));
        assert_eq!(
            store.group(web).unwrap().parent,
            store.group_by_sync_id("G1")
        );
        assert_eq!(behind.notes, "备注");
        assert_eq!(store.bookmarks(behind.id, BookmarkSide::Remote), ["/srv"]);
        assert_eq!(store.forwards()[0].host, bastion);
        assert!(store.forwards()[0].auto_start);
        assert_eq!(
            store.snippets()[0].category,
            store.snippet_category_by_sync_id("K1")
        );
        assert_eq!(
            fixture.secret(&behind.password_secret()).as_deref(),
            Some("behind password")
        );
    });
    fixture
        .settings
        .read_with(cx, |settings, _| assert!(!settings.settings().compose.open));
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("known_hosts")).unwrap(),
        "bastion.test ssh-ed25519 AAAA\n"
    );
}

#[gpui_kit::test]
fn credentials_come_with_their_secrets_and_keys(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let credential = |kind: &str, key: Option<&str>| {
        Payload::Credential(CredentialRecord {
            name: format!("{kind} credential"),
            kind: kind.into(),
            user: "deploy".into(),
            sort_order: 0,
            password: (kind == "password").then(|| SecretText::new("credential password")),
            private_key: key.map(SecretText::new),
            passphrase: key.map(|_| SecretText::new("key passphrase")),
        })
    };
    fixture.apply(
        vec![
            change("C1", credential("password", None)),
            change("C2", credential("key", Some("PRIVATE KEY ONE\n"))),
        ],
        cx,
    );
    let (password, key_path) = fixture.store.read_with(cx, |store, _| {
        let password = store
            .credential(store.credential_by_sync_id("C1").unwrap())
            .unwrap();
        let key = store
            .credential(store.credential_by_sync_id("C2").unwrap())
            .unwrap();
        (
            password.password_secret(),
            key.key_path.clone().unwrap().to_string(),
        )
    });
    assert_eq!(
        fixture.secret(&password).as_deref(),
        Some("credential password")
    );
    assert!(key_path.starts_with(&fixture.dir.path().join("keys").display().to_string()));
    assert_eq!(
        std::fs::read_to_string(&key_path).unwrap(),
        "PRIVATE KEY ONE\n"
    );
    assert_eq!(
        fixture.secret(&SecretRef::passphrase(&key_path)).as_deref(),
        Some("key passphrase")
    );

    // A new key replaces the file ShellRS keeps for it.
    fixture.apply(
        vec![change("C2", credential("key", Some("PRIVATE KEY TWO\n")))],
        cx,
    );
    let new_path = fixture.store.read_with(cx, |store, _| {
        let key = store
            .credential(store.credential_by_sync_id("C2").unwrap())
            .unwrap();
        key.key_path.clone().unwrap().to_string()
    });
    assert_eq!(
        std::fs::read_to_string(&new_path).unwrap(),
        "PRIVATE KEY TWO\n"
    );
}

#[gpui_kit::test]
fn deletions_take_what_refers_to_them_first(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    fixture.apply(
        vec![
            change("G1", group("生产", None)),
            change("H1", host("web", Some("G1"), RouteRecord::Direct)),
            change(
                "K1",
                Payload::SnippetCategory(CategoryRecord {
                    name: "Docker".into(),
                }),
            ),
        ],
        cx,
    );
    let changes = Changes::new(vec![
        ("G1".into(), None),
        ("H1".into(), None),
        ("K1".into(), None),
    ]);
    let removed = fixture
        .store
        .read_with(cx, |store, _| changes.removed_hosts(store));
    assert_eq!(removed.len(), 1);
    fixture.apply(
        vec![
            ("G1".into(), None),
            ("H1".into(), None),
            ("K1".into(), None),
        ],
        cx,
    );
    fixture.store.read_with(cx, |store, _| {
        assert!(store.hosts().is_empty());
        assert!(store.groups().is_empty());
        assert!(store.snippet_categories().is_empty());
    });
}

#[test]
fn the_last_word_on_an_id_is_the_one_kept() {
    let changes = Changes::new(vec![
        change("a", group("one", None)),
        ("b".into(), None),
        change("a", group("two", None)),
        change("b", group("back", None)),
    ]);
    assert_eq!(
        changes.upserts,
        [
            ("a".to_string(), group("two", None)),
            ("b".to_string(), group("back", None))
        ]
    );
    assert!(changes.deletions.is_empty());
}
