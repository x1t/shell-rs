//! The passwords Xshell 7 and later keep in its session and proxy files:
//! RC4 under the SHA-256 of the Windows user's name and SID, reversed
//! twice over, with the SHA-256 of the password after it. Only the user
//! who saved them, on the same Windows, can read them back; Xshell's master
//! password is not supported.

use base64::Engine as _;
use sha2::{Digest as _, Sha256};

/// The key that opens the passwords one Windows user saved.
#[derive(Clone)]
pub struct XshellKey([u8; 32]);

impl XshellKey {
    /// The key of `user` with the SID `sid` (`S-1-5-21-…`).
    pub fn new(user: &str, sid: &str) -> Self {
        let reversed: String = user.chars().rev().chain(sid.chars()).collect();
        let text: String = reversed.chars().rev().collect();
        Self(Sha256::digest(text.as_bytes()).into())
    }

    /// The key of the user ShellRS runs as. Xshell only runs on Windows, so
    /// elsewhere there is none.
    #[cfg(windows)]
    pub fn current() -> Option<Self> {
        let user = std::env::var("USERNAME").ok()?;
        let sid = crate::cli::current_user_sid().ok()?;
        Some(Self::new(&user, &sid))
    }

    #[cfg(not(windows))]
    pub fn current() -> Option<Self> {
        None
    }

    /// The password in `field` (a session's `Password=`), or `None` when it
    /// is not one this key opens: another user's, another Windows's, or
    /// behind a master password.
    pub fn decrypt(&self, field: &str) -> Option<String> {
        let data = base64::engine::general_purpose::STANDARD
            .decode(field.trim())
            .ok()?;
        let split = data.len().checked_sub(32)?;
        let (cipher, checksum) = data.split_at(split);
        let plain = rc4(&self.0, cipher);
        if Sha256::digest(&plain).as_slice() != checksum {
            return None;
        }
        String::from_utf8(plain).ok()
    }
}

/// RC4, which encrypts and decrypts alike.
fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut state: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
        state.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|byte| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(state[i as usize]);
            state.swap(i as usize, j as usize);
            let k = state[i as usize].wrapping_add(state[j as usize]);
            byte ^ state[k as usize]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made outside ShellRS, by a separate RC4 and SHA-256, for this user.
    const USER: &str = "alice";
    const SID: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn a_password_the_user_saved_comes_back() {
        let key = XshellKey::new(USER, SID);
        assert_eq!(
            key.decrypt("mFkyXWQc1eLwglvNmClOk5XQQTLBNz2dzvo2OipTJ59amW1zDhzERo3XTbixLw==")
                .as_deref(),
            Some("hunter2 密码")
        );
        assert_eq!(
            key.decrypt(" gF4zUXhDl6NmXtAXfQMOh1qEgz1fuY1d9CQdpx0dMtGIYWa/kKGPajvr ")
                .as_deref(),
            Some("proxy-pass")
        );
    }

    #[test]
    fn another_users_password_does_not_open() {
        let key = XshellKey::new("bob", SID);
        assert_eq!(
            key.decrypt("gF4zUXhDl6NmXtAXfQMOh1qEgz1fuY1d9CQdpx0dMtGIYWa/kKGPajvr"),
            None
        );
        let key = XshellKey::new(USER, "S-1-5-21-1-2-3-1002");
        assert_eq!(
            key.decrypt("gF4zUXhDl6NmXtAXfQMOh1qEgz1fuY1d9CQdpx0dMtGIYWa/kKGPajvr"),
            None
        );
    }

    #[test]
    fn what_is_not_a_password_does_not_open() {
        let key = XshellKey::new(USER, SID);
        assert_eq!(key.decrypt(""), None);
        assert_eq!(key.decrypt("not base64!"), None);
        assert_eq!(key.decrypt("AAAA"), None);
    }

    #[test]
    fn rc4_matches_the_published_test_vector() {
        // RFC 6229 has none this short; this is the classic "Key"/"Plaintext".
        assert_eq!(
            rc4(b"Key", b"Plaintext"),
            [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]
        );
    }
}
