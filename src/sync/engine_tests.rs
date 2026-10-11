use std::sync::Arc;

use super::*;
use crate::sync::memory::MemorySyncServer;
use crate::sync::record::{CategoryRecord, SETTINGS_ID};

const SERVER: &str = "https://sync.test";

fn category(name: &str) -> Payload {
    Payload::SnippetCategory(CategoryRecord { name: name.into() })
}

/// One device: its items and what it last agreed on.
struct Device {
    api: Arc<dyn SyncApi>,
    session: Session,
    vault: VaultKey,
    local: HashMap<String, Payload>,
    base: HashMap<String, Base>,
    cursor: i64,
}

impl Device {
    fn log_in(api: Arc<dyn SyncApi>, password: &str) -> Result<Self, SyncError> {
        let stored = log_in(api.as_ref(), SERVER, "me", password, "test")?;
        Ok(Self {
            session: stored.session(&crate::sync::state::Account {
                server: SERVER.into(),
                username: "me".into(),
            }),
            vault: stored.vault_key,
            api,
            local: HashMap::new(),
            base: HashMap::new(),
            cursor: 0,
        })
    }

    /// A round, carried out here the way the UI thread does it.
    fn sync(&mut self) -> RoundResult {
        let round = Round {
            api: self.api.as_ref(),
            session: &self.session,
            vault: &self.vault,
        };
        let result = round
            .run(
                self.local.clone(),
                &HashSet::new(),
                self.base.clone(),
                self.cursor,
            )
            .unwrap();
        for (from, to) in &result.rekey {
            if let Some(payload) = self.local.remove(from) {
                self.local.insert(to.clone(), payload);
            }
        }
        for (id, payload) in &result.apply {
            match payload {
                Some(payload) => self.local.insert(id.clone(), payload.clone()),
                None => self.local.remove(id),
            };
        }
        self.base = result.base.clone();
        self.cursor = result.cursor;
        result
    }
}

fn server() -> Arc<MemorySyncServer> {
    Arc::new(MemorySyncServer::with_user("me", "correct horse battery"))
}

#[test]
fn every_device_of_an_account_gets_the_same_vault_key() {
    let server = server();
    let first = Device::log_in(server.clone(), "correct horse battery").unwrap();
    let second = Device::log_in(server.clone(), "correct horse battery").unwrap();
    assert_eq!(first.vault.as_bytes(), second.vault.as_bytes());
    assert_eq!(
        Device::log_in(server, "wrong password").err(),
        Some(SyncError::Api(ApiError::Unauthorized))
    );
}

#[test]
fn what_one_device_makes_the_other_gets() {
    let server = server();
    let mut laptop = Device::log_in(server.clone(), "correct horse battery").unwrap();
    let mut desktop = Device::log_in(server.clone(), "correct horse battery").unwrap();
    laptop.local.insert("c1".into(), category("Docker"));
    laptop.sync();
    assert_eq!(server.live_records("me"), 1);

    let result = desktop.sync();
    assert_eq!(result.apply, [("c1".to_string(), Some(category("Docker")))]);

    // An edit on the desktop, then a deletion on the laptop.
    desktop.local.insert("c1".into(), category("容器"));
    desktop.sync();
    laptop.sync();
    assert_eq!(laptop.local["c1"], category("容器"));
    laptop.local.remove("c1");
    laptop.sync();
    desktop.sync();
    assert!(desktop.local.is_empty());
    assert_eq!(server.live_records("me"), 0);
    assert_eq!(server.record_ids("me"), ["c1"]);
}

#[test]
fn the_server_holds_only_sealed_records() {
    let server = server();
    let mut device = Device::log_in(server.clone(), "correct horse battery").unwrap();
    device.local.insert("c1".into(), category("机密分类"));
    device.sync();
    let page = server.pull(&device.session, 0).unwrap();
    let data = &page.records[0].data;
    assert!(
        !data
            .windows("机密".len())
            .any(|window| window == "机密".as_bytes())
    );
    assert!(!String::from_utf8_lossy(data).contains("snippet_category"));
}

#[test]
fn a_change_racing_another_device_still_goes_up() {
    let server = server();
    let mut laptop = Device::log_in(server.clone(), "correct horse battery").unwrap();
    let mut desktop = Device::log_in(server.clone(), "correct horse battery").unwrap();
    laptop.local.insert("c1".into(), category("Docker"));
    laptop.sync();
    desktop.sync();

    // Both edit; the laptop gets there first.
    laptop.local.insert("c1".into(), category("笔记本改的"));
    desktop.local.insert("c1".into(), category("台式机改的"));
    laptop.sync();
    desktop.sync();
    laptop.sync();
    // The last to change wins everywhere.
    assert_eq!(laptop.local["c1"], category("台式机改的"));
    assert_eq!(desktop.local["c1"], category("台式机改的"));
}

#[test]
fn a_new_device_takes_the_accounts_settings_and_keeps_nothing_twice() {
    let server = server();
    let mut laptop = Device::log_in(server.clone(), "correct horse battery").unwrap();
    let chosen = Payload::Settings {
        settings: serde_json::json!({"language": "en"}),
    };
    laptop.local.insert(SETTINGS_ID.into(), chosen.clone());
    laptop.local.insert("laptop-c".into(), category("Docker"));
    laptop.sync();

    let mut fresh = Device::log_in(server.clone(), "correct horse battery").unwrap();
    fresh.local.insert(
        SETTINGS_ID.into(),
        Payload::Settings {
            settings: serde_json::json!({}),
        },
    );
    fresh.local.insert("fresh-c".into(), category("Docker"));
    let result = fresh.sync();
    assert_eq!(
        result.rekey,
        [("fresh-c".to_string(), "laptop-c".to_string())]
    );
    assert_eq!(fresh.local[SETTINGS_ID], chosen);
    assert_eq!(server.live_records("me"), 2);
}

#[test]
fn a_long_history_comes_down_in_pages() {
    let server = server();
    let mut laptop = Device::log_in(server.clone(), "correct horse battery").unwrap();
    for ix in 0..620 {
        laptop
            .local
            .insert(format!("c{ix}"), category(&format!("分类 {ix}")));
    }
    laptop.sync();
    let mut desktop = Device::log_in(server.clone(), "correct horse battery").unwrap();
    let result = desktop.sync();
    assert_eq!(result.apply.len(), 620);
    assert_eq!(result.cursor, 620);
}

#[test]
fn a_record_sealed_with_another_key_stops_the_round() {
    let server = server();
    let mut device = Device::log_in(server.clone(), "correct horse battery").unwrap();
    device.local.insert("c1".into(), category("Docker"));
    device.sync();
    // As after the password was reset elsewhere and this device kept the
    // old key.
    device.vault = VaultKey::generate();
    device.cursor = 0;
    let round = Round {
        api: device.api.as_ref(),
        session: &device.session,
        vault: &device.vault,
    };
    let error = round
        .run(HashMap::new(), &HashSet::new(), HashMap::new(), 0)
        .unwrap_err();
    assert_eq!(error, SyncError::WrongKey);
}

/// Against a real shell-rs-server, e.g. one started with
/// `shell-rs-server serve -listen 127.0.0.1:18091 -db test.db` and an account
/// made with `user add`: set `SHELLRS_SYNC_TEST_SERVER`,
/// `SHELLRS_SYNC_TEST_USER` and `SHELLRS_SYNC_TEST_PASSWORD`.
#[test]
#[ignore = "needs a running shell-rs-server"]
fn two_devices_sync_through_a_real_server() {
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    let server = crate::sync::api::normalized_server(&var("SHELLRS_SYNC_TEST_SERVER")).unwrap();
    let (user, password) = (
        var("SHELLRS_SYNC_TEST_USER"),
        var("SHELLRS_SYNC_TEST_PASSWORD"),
    );
    let api: Arc<dyn SyncApi> = Arc::new(crate::sync::api::HttpSyncApi::new().unwrap());
    let device = |api: Arc<dyn SyncApi>| {
        let stored = log_in(api.as_ref(), &server, &user, &password, "e2e").unwrap();
        Device {
            session: Session {
                server: server.clone(),
                token: stored.token.clone(),
            },
            vault: stored.vault_key,
            api,
            local: HashMap::new(),
            base: HashMap::new(),
            cursor: 0,
        }
    };
    let mut laptop = device(api.clone());
    let mut desktop = device(api.clone());
    laptop.sync();
    desktop.sync();

    let id = crate::host::PublicId::generate().to_string();
    laptop.local.insert(id.clone(), category("端到端"));
    laptop.sync();
    desktop.sync();
    assert_eq!(desktop.local[&id], category("端到端"));

    desktop.local.insert(id.clone(), category("端到端 改"));
    desktop.sync();
    laptop.sync();
    assert_eq!(laptop.local[&id], category("端到端 改"));

    laptop.local.remove(&id);
    laptop.sync();
    desktop.sync();
    assert!(!desktop.local.contains_key(&id));
    api.logout(&laptop.session).unwrap();
    assert_eq!(
        api.pull(&laptop.session, 0).err(),
        Some(ApiError::Unauthorized)
    );
}
