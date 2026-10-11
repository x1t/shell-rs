//! 云同步: hosts, credentials, port forwards, snippets and settings kept in
//! step across one person's devices through their own shell-rs-server.
//! Everything leaves this machine sealed with a key the server never sees
//! (`crypto`).

mod api;
mod apply;
mod crypto;
mod engine;
mod local;
mod memory;
mod merge;
mod record;
mod state;
mod sync_form;
mod syncer;

pub use api::{ApiError, SyncApi};
pub use crypto::{CryptoError, DerivedKeys, KdfParams, VaultKey, derive};
pub use memory::MemorySyncServer;
pub use sync_form::SyncForm;
pub use syncer::{SyncBadge, SyncServices, SyncStatus, Syncer, SyncerEvent};

/// Bytes as the server's JSON spells them: standard base64.
mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    /// `null` is no bytes: how Go writes an empty slice it read as nil.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = Option::<String>::deserialize(deserializer)?.unwrap_or_default();
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}
