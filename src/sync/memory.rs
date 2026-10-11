//! A shell-rs-server kept in memory, answering as the real one does: for
//! the tests, which never reach a network.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use super::api::{
    ApiError, Applied, LoginReply, PullPage, PushReply, SealedPush, SealedRecord, Session, SyncApi,
};
use super::crypto::{KdfParams, derive};

/// The page size of the real server.
const PAGE: usize = 500;

#[derive(Default)]
pub struct MemorySyncServer {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    users: HashMap<String, User>,
    /// Token to user name.
    sessions: HashMap<String, String>,
    tokens: u64,
    /// What the next request fails with, once.
    failure: Option<ApiError>,
}

struct User {
    salt: Vec<u8>,
    auth_key: Vec<u8>,
    vault_key: Option<Vec<u8>>,
    revision: i64,
    records: BTreeMap<String, SealedRecord>,
}

impl MemorySyncServer {
    pub fn with_user(username: &str, password: &str) -> Self {
        let server = Self::default();
        server.add_user(username, password);
        server
    }

    /// An account, as `shell-rs-server user add` makes one.
    pub fn add_user(&self, username: &str, password: &str) {
        let salt: Vec<u8> = (0..16)
            .map(|byte| byte * 7 + username.len() as u8)
            .collect();
        let keys = derive(password, &params(&salt)).expect("the server's own parameters");
        self.lock().users.insert(
            username.to_owned(),
            User {
                salt,
                auth_key: keys.auth.to_vec(),
                vault_key: None,
                revision: 0,
                records: BTreeMap::new(),
            },
        );
    }

    /// The ids the server holds for `username`, deleted ones included.
    pub fn record_ids(&self, username: &str) -> Vec<String> {
        self.lock()
            .users
            .get(username)
            .map(|user| user.records.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// How many records `username` has that are not deleted.
    pub fn live_records(&self, username: &str) -> usize {
        self.lock().users.get(username).map_or(0, |user| {
            user.records.values().filter(|r| !r.deleted).count()
        })
    }

    /// Make the next request fail with `error`.
    pub fn fail_next(&self, error: ApiError) {
        self.lock().failure = Some(error);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn with_session<T>(
        &self,
        session: &Session,
        f: impl FnOnce(&mut User) -> T,
    ) -> Result<T, ApiError> {
        let mut inner = self.lock();
        if let Some(error) = inner.failure.take() {
            return Err(error);
        }
        let username = inner
            .sessions
            .get(&session.token)
            .cloned()
            .ok_or(ApiError::Unauthorized)?;
        let user = inner
            .users
            .get_mut(&username)
            .ok_or(ApiError::Unauthorized)?;
        Ok(f(user))
    }
}

fn params(salt: &[u8]) -> KdfParams {
    KdfParams {
        salt: salt.to_vec(),
        time: 3,
        memory: 64 * 1024,
        threads: 4,
    }
}

impl SyncApi for MemorySyncServer {
    fn prelogin(&self, _: &str, username: &str) -> Result<KdfParams, ApiError> {
        let mut inner = self.lock();
        if let Some(error) = inner.failure.take() {
            return Err(error);
        }
        let salt = inner
            .users
            .get(username)
            .map_or_else(|| vec![1; 16], |user| user.salt.clone());
        Ok(params(&salt))
    }

    fn login(
        &self,
        _: &str,
        username: &str,
        auth_key: &[u8],
        _: &str,
    ) -> Result<LoginReply, ApiError> {
        let mut inner = self.lock();
        if let Some(error) = inner.failure.take() {
            return Err(error);
        }
        let vault_key = match inner.users.get(username) {
            Some(user) if user.auth_key == auth_key => user.vault_key.clone(),
            _ => return Err(ApiError::Unauthorized),
        };
        inner.tokens += 1;
        let token = format!("token-{}", inner.tokens);
        inner.sessions.insert(token.clone(), username.to_owned());
        Ok(LoginReply { token, vault_key })
    }

    fn logout(&self, session: &Session) -> Result<(), ApiError> {
        self.lock().sessions.remove(&session.token);
        Ok(())
    }

    fn put_vault_key(
        &self,
        session: &Session,
        wrapped: &[u8],
    ) -> Result<Option<Vec<u8>>, ApiError> {
        self.with_session(session, |user| match &user.vault_key {
            Some(kept) => Some(kept.clone()),
            None => {
                user.vault_key = Some(wrapped.to_vec());
                None
            }
        })
    }

    fn pull(&self, session: &Session, since: i64) -> Result<PullPage, ApiError> {
        self.with_session(session, |user| {
            let mut records: Vec<SealedRecord> = user
                .records
                .values()
                .filter(|record| record.revision > since)
                .cloned()
                .collect();
            records.sort_by_key(|record| record.revision);
            let more = records.len() > PAGE;
            records.truncate(PAGE);
            PullPage { records, more }
        })
    }

    fn push(&self, session: &Session, pushes: &[SealedPush]) -> Result<PushReply, ApiError> {
        self.with_session(session, |user| {
            let mut reply = PushReply {
                applied: Vec::new(),
                conflicts: Vec::new(),
            };
            for push in pushes {
                if let Some(current) = user.records.get(&push.id)
                    && current.revision != push.base_revision
                {
                    reply.conflicts.push(current.clone());
                    continue;
                }
                user.revision += 1;
                user.records.insert(
                    push.id.clone(),
                    SealedRecord {
                        id: push.id.clone(),
                        revision: user.revision,
                        deleted: push.deleted,
                        data: if push.deleted {
                            Vec::new()
                        } else {
                            push.data.clone()
                        },
                    },
                );
                reply.applied.push(Applied {
                    id: push.id.clone(),
                    revision: user.revision,
                });
            }
            reply
        })
    }
}
