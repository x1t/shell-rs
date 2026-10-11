//! The shell-rs-server protocol, over HTTPS. Blocking: it runs on the sync
//! worker's thread, never the UI's.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::KdfParams;
use super::base64_bytes;
use crate::update::{build_info, platform};

const TIMEOUT: Duration = Duration::from_secs(30);

/// Why a request did not get the answer it wanted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// Not reached, or the connection broke: the reason, in a few words.
    Network(String),
    /// The user name or password is wrong, or the session was ended
    /// (logged out elsewhere, or the password was reset on the server).
    Unauthorized,
    /// Too many failed logins: the server waits a while.
    TooManyAttempts,
    /// The server said something else went wrong.
    Server(u16),
    /// The answer was not what a shell-rs-server says: some other site.
    NotSyncServer,
}

/// Where an account lives and the session that is logged in to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub server: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginReply {
    pub token: String,
    #[serde(with = "optional_bytes")]
    pub vault_key: Option<Vec<u8>>,
}

/// One record as the server has it, still sealed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedRecord {
    pub id: String,
    pub revision: i64,
    pub deleted: bool,
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

#[derive(Debug, Deserialize)]
pub struct PullPage {
    pub records: Vec<SealedRecord>,
    pub more: bool,
}

/// A record going up, sealed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SealedPush {
    pub id: String,
    pub base_revision: i64,
    pub deleted: bool,
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

#[derive(Debug, Deserialize)]
pub struct Applied {
    pub id: String,
    pub revision: i64,
}

#[derive(Debug, Deserialize)]
pub struct PushReply {
    pub applied: Vec<Applied>,
    pub conflicts: Vec<SealedRecord>,
}

/// The server, as the sync worker talks to it. Production has
/// [`HttpSyncApi`]; tests give an in-memory one.
pub trait SyncApi: Send + Sync + 'static {
    fn prelogin(&self, server: &str, username: &str) -> Result<KdfParams, ApiError>;
    fn login(
        &self,
        server: &str,
        username: &str,
        auth_key: &[u8],
        device: &str,
    ) -> Result<LoginReply, ApiError>;
    fn logout(&self, session: &Session) -> Result<(), ApiError>;
    /// Keep the first device's sealed vault key. When another device was
    /// first, its key comes back instead.
    fn put_vault_key(&self, session: &Session, wrapped: &[u8])
    -> Result<Option<Vec<u8>>, ApiError>;
    fn pull(&self, session: &Session, since: i64) -> Result<PullPage, ApiError>;
    fn push(&self, session: &Session, records: &[SealedPush]) -> Result<PushReply, ApiError>;
}

/// The address the user typed, as requests use it: no trailing slash, and
/// HTTPS unless the server is on this computer.
pub fn normalized_server(text: &str) -> Option<String> {
    let url = reqwest::Url::parse(text.trim()).ok()?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    let secure = url.scheme() == "https" || url.scheme() == "http" && local;
    if !secure || url.host_str().is_none() || url.query().is_some() {
        return None;
    }
    Some(url.as_str().trim_end_matches('/').to_owned())
}

pub struct HttpSyncApi {
    client: Client,
}

impl HttpSyncApi {
    /// Built on the worker thread: the blocking client runs a runtime of
    /// its own, which must not be made or dropped inside an async context.
    pub fn new() -> Result<Self, ApiError> {
        let client = Client::builder()
            .user_agent(platform::user_agent(build_info::VERSION))
            .connect_timeout(TIMEOUT)
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| ApiError::Network(describe(&error)))?;
        Ok(Self { client })
    }

    fn send<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, ApiError> {
        let response = self.checked(request)?;
        let bytes = response
            .bytes()
            .map_err(|error| ApiError::Network(describe(&error)))?;
        serde_json::from_slice(&bytes).map_err(|_| ApiError::NotSyncServer)
    }

    fn checked(&self, request: RequestBuilder) -> Result<Response, ApiError> {
        let response = request
            .send()
            .map_err(|error| ApiError::Network(describe(&error)))?;
        match response.status() {
            status if status.is_success() => Ok(response),
            StatusCode::UNAUTHORIZED => Err(ApiError::Unauthorized),
            StatusCode::TOO_MANY_REQUESTS => Err(ApiError::TooManyAttempts),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => Err(ApiError::NotSyncServer),
            status => Err(ApiError::Server(status.as_u16())),
        }
    }

    fn authed(&self, request: RequestBuilder, session: &Session) -> RequestBuilder {
        request.bearer_auth(&session.token)
    }
}

impl SyncApi for HttpSyncApi {
    fn prelogin(&self, server: &str, username: &str) -> Result<KdfParams, ApiError> {
        let body = serde_json::json!({ "username": username });
        self.send(json(
            self.client.post(format!("{server}/api/prelogin")),
            &body,
        ))
    }

    fn login(
        &self,
        server: &str,
        username: &str,
        auth_key: &[u8],
        device: &str,
    ) -> Result<LoginReply, ApiError> {
        #[derive(Serialize)]
        struct Login<'a> {
            username: &'a str,
            #[serde(with = "base64_bytes")]
            auth_key: Vec<u8>,
            device: &'a str,
        }
        let body = Login {
            username,
            auth_key: auth_key.to_vec(),
            device,
        };
        self.send(json(self.client.post(format!("{server}/api/login")), &body))
    }

    fn logout(&self, session: &Session) -> Result<(), ApiError> {
        let request = self.client.post(format!("{}/api/logout", session.server));
        self.checked(self.authed(request, session)).map(drop)
    }

    fn put_vault_key(
        &self,
        session: &Session,
        wrapped: &[u8],
    ) -> Result<Option<Vec<u8>>, ApiError> {
        #[derive(Serialize, Deserialize)]
        struct VaultKey {
            #[serde(with = "optional_bytes")]
            vault_key: Option<Vec<u8>>,
        }
        let body = VaultKey {
            vault_key: Some(wrapped.to_vec()),
        };
        let request = json(
            self.client.put(format!("{}/api/vault-key", session.server)),
            &body,
        );
        let response = self
            .authed(request, session)
            .send()
            .map_err(|error| ApiError::Network(describe(&error)))?;
        if response.status() == StatusCode::CONFLICT {
            let bytes = response
                .bytes()
                .map_err(|error| ApiError::Network(describe(&error)))?;
            let kept: VaultKey =
                serde_json::from_slice(&bytes).map_err(|_| ApiError::NotSyncServer)?;
            return kept.vault_key.map(Some).ok_or(ApiError::NotSyncServer);
        }
        match response.status() {
            status if status.is_success() => Ok(None),
            StatusCode::UNAUTHORIZED => Err(ApiError::Unauthorized),
            status => Err(ApiError::Server(status.as_u16())),
        }
    }

    fn pull(&self, session: &Session, since: i64) -> Result<PullPage, ApiError> {
        let request = self
            .client
            .get(format!("{}/api/records?since={since}", session.server));
        self.send(self.authed(request, session))
    }

    fn push(&self, session: &Session, records: &[SealedPush]) -> Result<PushReply, ApiError> {
        let body = serde_json::json!({ "records": records });
        let request = json(
            self.client.post(format!("{}/api/records", session.server)),
            &body,
        );
        self.send(self.authed(request, session))
    }
}

/// A request with `body` as its JSON.
fn json(request: RequestBuilder, body: &impl Serialize) -> RequestBuilder {
    request
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(body).expect("a request body always serializes"))
}

/// A request error in a few words: reqwest's own message only names the
/// URL, the reason is at the end of its chain.
fn describe(error: &reqwest::Error) -> String {
    let mut source: &dyn std::error::Error = error;
    while let Some(next) = source.source() {
        source = next;
    }
    source.to_string()
}

/// `null` or base64.
mod optional_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => serializer.serialize_str(&STANDARD.encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| STANDARD.decode(text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::normalized_server;

    #[test]
    fn a_server_address_must_be_https_unless_it_is_on_this_computer() {
        assert_eq!(
            normalized_server(" https://sync.example.com/ ").as_deref(),
            Some("https://sync.example.com")
        );
        assert_eq!(
            normalized_server("https://example.com/sync/").as_deref(),
            Some("https://example.com/sync")
        );
        assert_eq!(
            normalized_server("http://127.0.0.1:18090").as_deref(),
            Some("http://127.0.0.1:18090")
        );
        assert_eq!(normalized_server("http://sync.example.com"), None);
        assert_eq!(normalized_server("sync.example.com"), None);
        assert_eq!(normalized_server("ftp://example.com"), None);
        assert_eq!(normalized_server("https://example.com/?a=1"), None);
    }
}
