//! What the sync worker does with the server: logging in, and one round of
//! pull, merge and push. Blocking; runs on the worker's thread.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::api::{ApiError, SealedPush, SealedRecord, Session, SyncApi};
use super::crypto::{CryptoError, VaultKey, derive};
use super::merge::{Base, Merge, Push, Remote};
use super::record::Payload;
use super::state::StoredSession;

/// Records sent in one request: well under the server's 1000 and 32 MB.
const PUSH_BATCH: usize = 200;
/// Times a round sends again what came back as changed meanwhile.
const PUSH_ATTEMPTS: usize = 3;

/// Why logging in or a round failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncError {
    Api(ApiError),
    /// The server asked for a key derivation this build will not run.
    UnsafeServer,
    /// What the server holds was sealed with another key: the password was
    /// reset on the server, so this device must log in again.
    WrongKey,
    /// The keychain refused the session.
    Keychain(String),
}

impl From<ApiError> for SyncError {
    fn from(error: ApiError) -> Self {
        SyncError::Api(error)
    }
}

impl From<CryptoError> for SyncError {
    fn from(error: CryptoError) -> Self {
        match error {
            CryptoError::UnsafeParams => SyncError::UnsafeServer,
            CryptoError::WrongKey => SyncError::WrongKey,
        }
    }
}

/// Log in and come back with the session and the account's vault key,
/// making the vault key when this is the account's first device.
pub fn log_in(
    api: &dyn SyncApi,
    server: &str,
    username: &str,
    password: &str,
    device: &str,
) -> Result<StoredSession, SyncError> {
    let params = api.prelogin(server, username)?;
    let keys = derive(password, &params)?;
    let reply = api.login(server, username, keys.auth.as_ref(), device)?;
    let session = Session {
        server: server.to_owned(),
        token: reply.token,
    };
    let vault_key = match reply.vault_key {
        Some(wrapped) => VaultKey::unwrap(&keys.master, &wrapped)?,
        None => {
            let fresh = VaultKey::generate();
            match api.put_vault_key(&session, &fresh.wrap(&keys.master))? {
                None => fresh,
                // Another device made one first.
                Some(wrapped) => VaultKey::unwrap(&keys.master, &wrapped)?,
            }
        }
    };
    Ok(StoredSession {
        token: session.token,
        vault_key,
    })
}

/// What a round leaves for the UI thread to carry out.
#[derive(Debug, Default, PartialEq)]
pub struct RoundResult {
    /// `(this device's id, the server's)`.
    pub rekey: Vec<(String, String)>,
    /// Later entries for one id replace earlier ones.
    pub apply: Vec<(String, Option<Payload>)>,
    pub base: HashMap<String, Base>,
    pub cursor: i64,
}

/// One round: what changed on the server comes in, what changed here goes
/// out.
pub struct Round<'a> {
    pub api: &'a dyn SyncApi,
    pub session: &'a Session,
    pub vault: &'a VaultKey,
}

impl Round<'_> {
    pub fn run(
        &self,
        mut local: HashMap<String, Payload>,
        unreadable: &HashSet<String>,
        mut base: HashMap<String, Base>,
        cursor: i64,
    ) -> Result<RoundResult, SyncError> {
        let (remote, cursor) = self.pull(cursor)?;
        let fingerprint = |payload: &Payload| self.vault.fingerprint(&payload.to_bytes());
        let mut merge = Merge {
            local: &mut local,
            unreadable,
            base: &mut base,
            fingerprint: &fingerprint,
        };
        let mut plan = merge.plan(&remote);
        let mut result = RoundResult {
            rekey: std::mem::take(&mut plan.rekey),
            apply: std::mem::take(&mut plan.apply),
            ..RoundResult::default()
        };
        for _ in 0..PUSH_ATTEMPTS {
            if plan.push.is_empty() {
                break;
            }
            let conflicts = self.push(&plan.push, merge.base, &fingerprint)?;
            plan = merge.plan(&conflicts);
            result.apply.append(&mut plan.apply);
        }
        result.base = base;
        result.cursor = cursor;
        Ok(result)
    }

    /// Everything after `since`, page by page; the latest of each id.
    fn pull(&self, since: i64) -> Result<(BTreeMap<String, Remote>, i64), SyncError> {
        let mut remote = BTreeMap::new();
        let mut cursor = since;
        loop {
            let page = self.api.pull(self.session, cursor)?;
            for record in page.records {
                cursor = cursor.max(record.revision);
                if let Some((id, theirs)) = self.open(record)? {
                    remote.insert(id, theirs);
                }
            }
            if !page.more {
                return Ok((remote, cursor));
            }
        }
    }

    /// A record from the server, opened. One of a kind this build does not
    /// know (a newer ShellRS made it) is passed over.
    fn open(&self, record: SealedRecord) -> Result<Option<(String, Remote)>, SyncError> {
        let payload = if record.deleted {
            None
        } else {
            let bytes = self.vault.open_record(&record.id, &record.data)?;
            match Payload::from_bytes(&bytes) {
                Some(payload) => Some(payload),
                None => return Ok(None),
            }
        };
        let theirs = Remote {
            revision: record.revision,
            payload,
        };
        Ok(Some((record.id, theirs)))
    }

    /// Send `pushes`, noting the revisions the server gave them in `base`,
    /// and come back with those another device changed meanwhile.
    fn push(
        &self,
        pushes: &[Push],
        base: &mut HashMap<String, Base>,
        fingerprint: &dyn Fn(&Payload) -> String,
    ) -> Result<BTreeMap<String, Remote>, SyncError> {
        let mut conflicts = BTreeMap::new();
        for batch in pushes.chunks(PUSH_BATCH) {
            let sealed: Vec<SealedPush> = batch.iter().map(|push| self.seal(push)).collect();
            let reply = self.api.push(self.session, &sealed)?;
            for applied in reply.applied {
                let Some(push) = batch.iter().find(|push| push.id == applied.id) else {
                    continue;
                };
                let print = push
                    .payload
                    .as_ref()
                    .map_or_else(|| super::merge::GONE.to_owned(), fingerprint);
                base.insert(
                    applied.id,
                    Base {
                        revision: applied.revision,
                        fingerprint: print,
                    },
                );
            }
            for record in reply.conflicts {
                if let Some((id, theirs)) = self.open(record)? {
                    conflicts.insert(id, theirs);
                }
            }
        }
        Ok(conflicts)
    }

    fn seal(&self, push: &Push) -> SealedPush {
        SealedPush {
            id: push.id.clone(),
            base_revision: push.base_revision,
            deleted: push.payload.is_none(),
            data: push
                .payload
                .as_ref()
                .map(|payload| self.vault.seal_record(&push.id, &payload.to_bytes()))
                .unwrap_or_default(),
        }
    }
}

/// What this computer is called in the server's list of sessions.
pub fn device_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "ShellRS".to_owned())
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
