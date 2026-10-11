//! 云同步's encryption. The server only ever holds what this seals.
//!
//! The account password goes through Argon2id into 64 bytes: the first 32
//! are the master key, which never leaves this machine, the last 32 the
//! auth key, which is what logging in sends. A random vault key, sealed
//! with the master key, is what the server keeps for the other devices;
//! every record is sealed with the vault key, bound to its id.
//!
//! The KDF must give what the server's `authKey` gives for the same
//! password and salt: `the_kdf_agrees_with_the_server` holds a vector the
//! Go code made.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use rand::RngCore as _;
use serde::Deserialize;
use sha2::Sha256;
use zeroize::Zeroizing;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
/// What a sealed vault key is bound to, so it can never pass for a record.
const VAULT_KEY_AAD: &[u8] = b"shellrs-vault-key";

/// The least a server may ask the KDF for. A server that asked for less
/// would make the password cheaper to guess from what logging in sends.
const MIN_MEMORY_KIB: u32 = 64 * 1024;
const MIN_TIME: u32 = 3;
/// The most: a server must not make a device grind for minutes.
const MAX_MEMORY_KIB: u32 = 1024 * 1024;
const MAX_TIME: u32 = 10;
const MAX_THREADS: u32 = 16;

/// How the server says to run the KDF for an account.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct KdfParams {
    #[serde(with = "super::base64_bytes")]
    pub salt: Vec<u8>,
    pub time: u32,
    pub memory: u32,
    pub threads: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// The server asked for a KDF weaker or heavier than this build runs.
    UnsafeParams,
    /// Sealed with another key: the vault key with another password, or a
    /// record with another vault key.
    WrongKey,
}

/// The two halves of the password's derived key.
pub struct DerivedKeys {
    pub master: Zeroizing<[u8; KEY_LEN]>,
    pub auth: Zeroizing<[u8; KEY_LEN]>,
}

pub fn derive(password: &str, params: &KdfParams) -> Result<DerivedKeys, CryptoError> {
    let safe = (MIN_MEMORY_KIB..=MAX_MEMORY_KIB).contains(&params.memory)
        && (MIN_TIME..=MAX_TIME).contains(&params.time)
        && (1..=MAX_THREADS).contains(&params.threads)
        && params.salt.len() >= argon2::RECOMMENDED_SALT_LEN;
    if !safe {
        return Err(CryptoError::UnsafeParams);
    }
    let argon = Params::new(
        params.memory,
        params.time,
        params.threads,
        Some(2 * KEY_LEN),
    )
    .map_err(|_| CryptoError::UnsafeParams)?;
    let mut out = Zeroizing::new([0_u8; 2 * KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, argon)
        .hash_password_into(password.as_bytes(), &params.salt, out.as_mut())
        .map_err(|_| CryptoError::UnsafeParams)?;
    let mut keys = DerivedKeys {
        master: Zeroizing::new([0; KEY_LEN]),
        auth: Zeroizing::new([0; KEY_LEN]),
    };
    keys.master.copy_from_slice(&out[..KEY_LEN]);
    keys.auth.copy_from_slice(&out[KEY_LEN..]);
    Ok(keys)
}

/// The key every record is sealed with, the same on every device.
#[derive(Clone)]
pub struct VaultKey(Zeroizing<[u8; KEY_LEN]>);

impl VaultKey {
    pub fn generate() -> Self {
        let mut key = Zeroizing::new([0_u8; KEY_LEN]);
        rand::rng().fill_bytes(key.as_mut());
        Self(key)
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let key: [u8; KEY_LEN] = bytes.try_into().ok()?;
        Some(Self(Zeroizing::new(key)))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_ref()
    }

    /// The vault key sealed with the master key, for the server to keep.
    pub fn wrap(&self, master: &[u8; KEY_LEN]) -> Vec<u8> {
        seal(master, VAULT_KEY_AAD, self.as_bytes())
    }

    pub fn unwrap(master: &[u8; KEY_LEN], wrapped: &[u8]) -> Result<Self, CryptoError> {
        let bytes = open(master, VAULT_KEY_AAD, wrapped)?;
        Self::from_bytes(&bytes).ok_or(CryptoError::WrongKey)
    }

    /// A record's content, sealed and bound to its id: a server that swapped
    /// two records' data would have both refused.
    pub fn seal_record(&self, id: &str, plaintext: &[u8]) -> Vec<u8> {
        seal(&self.0, id.as_bytes(), plaintext)
    }

    pub fn open_record(&self, id: &str, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        open(&self.0, id.as_bytes(), sealed)
    }

    /// What a record's content was when last synced, without keeping the
    /// content: a keyed hash, so a password in it cannot be guessed from
    /// the state file.
    pub fn fingerprint(&self, plaintext: &[u8]) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.as_bytes()).expect("HMAC takes any key length");
        mac.update(plaintext);
        let digest = mac.finalize().into_bytes();
        digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// `nonce || ciphertext`, the nonce random: 24 bytes never repeat.
fn seal(key: &[u8; KEY_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0_u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let sealed = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("sealing in memory does not fail");
    let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

fn open(key: &[u8; KEY_LEN], aad: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let (nonce, ciphertext) = sealed
        .split_first_chunk::<NONCE_LEN>()
        .ok_or(CryptoError::WrongKey)?;
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::WrongKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> KdfParams {
        KdfParams {
            salt: (0..16).collect(),
            time: 3,
            memory: 64 * 1024,
            threads: 4,
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn the_kdf_agrees_with_the_server() {
        // argon2.IDKey("correct horse battery", 0x00..0x0f, 3, 64*1024, 4, 64)
        // from golang.org/x/crypto, as shell-rs-server runs it.
        let keys = derive("correct horse battery", &params()).unwrap();
        assert_eq!(
            hex(keys.master.as_ref()),
            "c83bf2083fa9eba5f08e79315c4c4d92048cff2e5415bbc7dfe1afb5f3501a4e"
        );
        assert_eq!(
            hex(keys.auth.as_ref()),
            "680350def4c72168698ab17a55fd87995b19a8cd2b4a47fc98ff7274c8da48b0"
        );
    }

    #[test]
    fn a_server_cannot_ask_for_a_weak_or_endless_kdf() {
        let weak = [
            KdfParams {
                memory: 8 * 1024,
                ..params()
            },
            KdfParams {
                time: 1,
                ..params()
            },
            KdfParams {
                salt: vec![1; 8],
                ..params()
            },
            KdfParams {
                time: 1000,
                ..params()
            },
            KdfParams {
                threads: 0,
                ..params()
            },
        ];
        for params in weak {
            assert_eq!(
                derive("pw", &params).err(),
                Some(CryptoError::UnsafeParams),
                "{params:?}"
            );
        }
    }

    #[test]
    fn the_vault_key_opens_only_with_the_same_password() {
        let right = derive("correct horse battery", &params()).unwrap();
        let wrong = derive("correct horse battery!", &params()).unwrap();
        let vault = VaultKey::generate();
        let wrapped = vault.wrap(&right.master);

        let opened = VaultKey::unwrap(&right.master, &wrapped).unwrap();
        assert_eq!(opened.as_bytes(), vault.as_bytes());
        assert_eq!(
            VaultKey::unwrap(&wrong.master, &wrapped).err(),
            Some(CryptoError::WrongKey)
        );
        assert!(VaultKey::unwrap(&right.master, &wrapped[..10]).is_err());
    }

    #[test]
    fn a_record_opens_only_under_its_own_id_and_key() {
        let vault = VaultKey::generate();
        let sealed = vault.seal_record("abc", b"{\"name\":\"web\"}");
        assert_eq!(
            vault.open_record("abc", &sealed).unwrap().as_slice(),
            b"{\"name\":\"web\"}"
        );
        assert!(vault.open_record("abd", &sealed).is_err());
        assert!(VaultKey::generate().open_record("abc", &sealed).is_err());
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(vault.open_record("abc", &tampered).is_err());
        // Sealing twice never gives the same bytes.
        assert_ne!(
            vault.seal_record("abc", b"x"),
            vault.seal_record("abc", b"x")
        );
    }

    #[test]
    fn a_vault_key_never_passes_for_a_record() {
        let keys = derive("pw-pw-pw-pw", &params()).unwrap();
        let wrapped = VaultKey::generate().wrap(&keys.master);
        let as_vault = VaultKey::from_bytes(keys.master.as_ref()).unwrap();
        assert!(as_vault.open_record("shellrs-vault", &wrapped).is_err());
    }

    #[test]
    fn fingerprints_depend_on_the_content_and_the_key() {
        let vault = VaultKey::generate();
        assert_eq!(vault.fingerprint(b"a"), vault.fingerprint(b"a"));
        assert_ne!(vault.fingerprint(b"a"), vault.fingerprint(b"b"));
        assert_ne!(
            vault.fingerprint(b"a"),
            VaultKey::generate().fingerprint(b"a")
        );
        assert_eq!(vault.fingerprint(b"a").len(), 32);
    }
}
