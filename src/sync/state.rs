//! What 云同步 remembers between runs: the account, in `sync.json`, and the
//! session, in the keychain.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::api::Session;
use super::crypto::VaultKey;
use super::merge::Base;
use crate::secrets::{SecretRef, SecretStore};

/// The account this device syncs with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub server: String,
    pub username: String,
}

impl Account {
    pub fn secret(&self) -> SecretRef {
        SecretRef::sync(&self.server, &self.username)
    }
}

/// `sync.json`. Nothing secret: the fingerprints are keyed hashes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncState {
    pub account: Option<Account>,
    /// The server revision this device has pulled up to.
    pub cursor: i64,
    /// What this device last agreed on with the server, per item.
    pub base: HashMap<String, Base>,
    /// When the last round finished, in seconds since the Unix epoch.
    pub last_synced: Option<u64>,
}

impl SyncState {
    /// What `path` holds; a missing or unreadable file is a device that
    /// never synced, which the next round sorts out by matching items.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Replace the file whole, through a temporary one.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, path)
    }

    /// Logged in to `account`: what was agreed with another account means
    /// nothing to this one.
    pub fn signed_in(&mut self, account: Account) {
        if self.account.as_ref() != Some(&account) {
            *self = Self {
                account: Some(account),
                ..Self::default()
            };
        }
    }
}

/// The session kept in the keychain: the token and the vault key, so the
/// password is asked for once per device.
pub struct StoredSession {
    pub token: String,
    pub vault_key: VaultKey,
}

#[derive(Serialize, Deserialize)]
struct SessionJson {
    token: String,
    vault_key: String,
}

impl StoredSession {
    pub fn session(&self, account: &Account) -> Session {
        Session {
            server: account.server.clone(),
            token: self.token.clone(),
        }
    }

    pub fn load(secrets: &dyn SecretStore, account: &Account) -> anyhow::Result<Option<Self>> {
        let Some(text) = secrets.get(&account.secret())? else {
            return Ok(None);
        };
        let Ok(json) = serde_json::from_str::<SessionJson>(&text) else {
            return Ok(None);
        };
        let key = Zeroizing::new(STANDARD.decode(&json.vault_key).unwrap_or_default());
        Ok(VaultKey::from_bytes(&key).map(|vault_key| Self {
            token: json.token,
            vault_key,
        }))
    }

    pub fn save(&self, secrets: &dyn SecretStore, account: &Account) -> anyhow::Result<()> {
        let json = Zeroizing::new(serde_json::to_string(&SessionJson {
            token: self.token.clone(),
            vault_key: STANDARD.encode(self.vault_key.as_bytes()),
        })?);
        secrets.set(&account.secret(), &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::InMemorySecretStore;

    fn account(user: &str) -> Account {
        Account {
            server: "https://sync.example.com".into(),
            username: user.into(),
        }
    }

    #[test]
    fn the_state_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sync.json");
        assert_eq!(SyncState::load(&path), SyncState::default());

        let mut state = SyncState::default();
        state.signed_in(account("me"));
        state.cursor = 7;
        state.base.insert(
            "a".into(),
            Base {
                revision: 7,
                fingerprint: "ab".into(),
            },
        );
        state.save(&path).unwrap();
        assert_eq!(SyncState::load(&path), state);

        std::fs::write(&path, "not json").unwrap();
        assert_eq!(SyncState::load(&path), SyncState::default());
    }

    #[test]
    fn another_account_starts_over_and_the_same_one_keeps_its_place() {
        let mut state = SyncState::default();
        state.signed_in(account("me"));
        state.cursor = 7;
        state.signed_in(account("me"));
        assert_eq!(state.cursor, 7);
        state.signed_in(account("you"));
        assert_eq!(state.cursor, 0);
        assert_eq!(state.account, Some(account("you")));
    }

    #[test]
    fn the_session_is_kept_in_the_keychain() {
        let secrets = InMemorySecretStore::default();
        let me = account("me");
        assert!(StoredSession::load(&secrets, &me).unwrap().is_none());

        let vault_key = VaultKey::generate();
        let session = StoredSession {
            token: "t0ken".into(),
            vault_key: vault_key.clone(),
        };
        session.save(&secrets, &me).unwrap();
        let loaded = StoredSession::load(&secrets, &me).unwrap().unwrap();
        assert_eq!(loaded.token, "t0ken");
        assert_eq!(loaded.vault_key.as_bytes(), vault_key.as_bytes());
        assert!(
            StoredSession::load(&secrets, &account("you"))
                .unwrap()
                .is_none()
        );
    }
}
