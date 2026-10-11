use super::*;
use crate::host::{
    CredentialDraft, ForwardDraft, ForwardEndpoint, ForwardKind, HostDraft, ProxyKind,
    ProxySettings, SnippetDraft,
};
use crate::secrets::InMemorySecretStore;

/// The seed hosts, with a key credential, a jump route, a proxy, a
/// bookmark, a forward and a snippet.
fn store(key_path: &Path) -> HostStore {
    let mut store = HostStore::seed();
    let mut key = CredentialDraft::new("deploy key", CredentialKind::Key, "deploy");
    key.key_path = Some(key_path.display().to_string().into());
    let key = store.insert_credential_unnotified(key);
    let web = store.hosts()[0].id;
    let db = store.hosts()[2].id;

    let mut behind = HostDraft::new("behind", "10.9.9.9", 22, "", AuthKind::Password, None);
    behind.credential = Some(key);
    behind.route = Route::Jump(vec![Some(web)]);
    store.insert_unnotified(behind);
    let mut db_draft = store.host(db).unwrap().draft();
    db_draft.route =
        Route::Proxy(ProxySettings::new(ProxyKind::Socks5, "127.0.0.1", 7890).with_user("me"));
    store.update_unnotified(db, db_draft);

    store.add_bookmark_unnotified(web, BookmarkSide::Remote, "/var/log");
    store.insert_forward_unnotified(ForwardDraft::new(
        ForwardKind::Dynamic,
        web,
        ForwardEndpoint::new("127.0.0.1", 1080),
        None,
    ));
    let docker = store.insert_snippet_category_unnotified("Docker");
    store.insert_snippet_unnotified(SnippetDraft::new("ps", "docker ps", Some(docker)));
    store
}

fn record<'a>(local: &'a HashMap<String, Payload>, id: &str) -> &'a Payload {
    local.get(id).unwrap_or_else(|| panic!("{id} is not there"))
}

#[test]
fn every_item_is_a_record_referring_to_others_by_sync_id() {
    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("id_ed25519");
    std::fs::write(&key_path, "-----BEGIN OPENSSH PRIVATE KEY-----\n").unwrap();
    let store = store(&key_path);
    let secrets = InMemorySecretStore::default();
    secrets
        .set(&store.hosts()[0].password_secret(), "web password")
        .unwrap();
    secrets
        .set(&SecretRef::proxy("me", "127.0.0.1", 7890), "proxy password")
        .unwrap();
    secrets
        .set(&SecretRef::passphrase(&key_path), "key passphrase")
        .unwrap();

    let snapshot = LocalSnapshot::of(&store, serde_json::json!({"language": "en"}), None);
    let (local, unreadable) = snapshot.read(&secrets);
    assert!(unreadable.is_empty());
    // 3 groups, 1 credential, 7 hosts, 1 forward, 1 category, 1 snippet and
    // the settings.
    assert_eq!(local.len(), 15);

    let web = &store.hosts()[0];
    let Payload::Host(web_record) = record(&local, web.sync_id.as_str()) else {
        panic!("web-01 is a host");
    };
    assert_eq!(
        web_record.password.as_ref().unwrap().as_str(),
        "web password"
    );
    assert_eq!(web_record.remote_bookmarks, ["/var/log"]);
    assert_eq!(
        web_record.group.as_deref(),
        Some(store.groups()[0].sync_id.as_str())
    );

    let behind = store
        .hosts()
        .iter()
        .find(|host| host.name == "behind")
        .unwrap();
    let Payload::Host(behind_record) = record(&local, behind.sync_id.as_str()) else {
        panic!("behind is a host");
    };
    let credential = &store.credentials()[0];
    assert_eq!(behind_record.auth, "credential");
    assert_eq!(behind_record.user, "");
    assert_eq!(
        behind_record.credential.as_deref(),
        Some(credential.sync_id.as_str())
    );
    assert_eq!(
        behind_record.route,
        RouteRecord::Jump {
            hops: vec![Some(web.sync_id.to_string())]
        }
    );
    assert_eq!(behind_record.password, None);

    let Payload::Credential(key) = record(&local, credential.sync_id.as_str()) else {
        panic!("a credential");
    };
    assert_eq!(
        key.private_key.as_ref().unwrap().as_str(),
        "-----BEGIN OPENSSH PRIVATE KEY-----\n"
    );
    assert_eq!(key.passphrase.as_ref().unwrap().as_str(), "key passphrase");

    let db = &store.hosts()[2];
    let Payload::Host(db_record) = record(&local, db.sync_id.as_str()) else {
        panic!("db-01 is a host");
    };
    assert_eq!(
        db_record.proxy_password.as_ref().unwrap().as_str(),
        "proxy password"
    );
    assert!(matches!(&db_record.route, RouteRecord::Proxy { kind, .. } if kind == "socks5"));

    let rule = &store.forwards()[0];
    let Payload::Forward(forward) = record(&local, rule.sync_id.as_str()) else {
        panic!("a forward");
    };
    assert_eq!(forward.host, web.sync_id.as_str());
    let snippet = &store.snippets()[0];
    let Payload::Snippet(snippet_record) = record(&local, snippet.sync_id.as_str()) else {
        panic!("a snippet");
    };
    assert_eq!(
        snippet_record.category.as_deref(),
        Some(store.snippet_categories()[0].sync_id.as_str())
    );
    assert_eq!(
        record(&local, SETTINGS_ID),
        &Payload::Settings {
            settings: serde_json::json!({"language": "en"})
        }
    );
}

#[test]
fn an_item_whose_secret_cannot_be_read_is_named_not_sent() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir.path().join("gone"));
    let (local, unreadable) = LocalSnapshot::of(&store, serde_json::json!({}), None)
        .read(&InMemorySecretStore::default());
    let credential = store.credentials()[0].sync_id.to_string();
    assert_eq!(unreadable, HashSet::from([credential.clone()]));
    assert!(!local.contains_key(&credential));
}

#[test]
fn known_hosts_are_read_as_sorted_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("known_hosts");
    assert_eq!(read_known_hosts(&path).unwrap(), Vec::<String>::new());
    std::fs::write(&path, "b key2\n\n# comment\na key1\r\nb key2\n").unwrap();
    assert_eq!(read_known_hosts(&path).unwrap(), ["a key1", "b key2"]);

    let snapshot = LocalSnapshot::of(&HostStore::empty(), serde_json::json!({}), Some(path));
    let (local, _) = snapshot.read(&InMemorySecretStore::default());
    assert_eq!(
        local[KNOWN_HOSTS_ID],
        Payload::KnownHosts {
            lines: vec!["a key1".into(), "b key2".into()]
        }
    );
}
