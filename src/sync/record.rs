//! What one synced item holds, the same on every device.
//!
//! Items refer to each other by sync id, never by the ids this device's
//! database gives out. Each serializes to the same bytes wherever it was
//! made (`Payload::to_bytes`), so two devices holding the same item hold
//! the same fingerprint of it.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The id of the settings record. Items have random sync ids; there is one
/// of these per account.
pub const SETTINGS_ID: &str = "settings";
/// The id of the known hosts record.
pub const KNOWN_HOSTS_ID: &str = "known-hosts";

/// A password, passphrase or private key inside a record: kept out of
/// `Debug` and wiped when dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretText(Zeroizing<String>);

impl Serialize for SecretText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SecretText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}

impl SecretText {
    pub fn new(text: impl Into<String>) -> Self {
        Self(Zeroizing::new(text.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretText(..)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRecord {
    pub name: String,
    pub parent: Option<String>,
    pub sort_order: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRecord {
    pub name: String,
    /// `password`, `key` or `agent`, as the database spells them. Not
    /// `kind` in the data: the payload's own tag is, and a second one makes
    /// the record unreadable everywhere.
    #[serde(rename = "credential_kind")]
    pub kind: String,
    pub user: String,
    pub sort_order: i64,
    /// A password credential's password.
    pub password: Option<SecretText>,
    /// A key credential's key: the text of the file, wherever it is kept
    /// on the device it came from.
    pub private_key: Option<SecretText>,
    pub passphrase: Option<SecretText>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RouteRecord {
    Direct,
    /// `None` is a deleted jump host, kept in its place.
    Jump {
        hops: Vec<Option<String>>,
    },
    Proxy {
        kind: String,
        host: String,
        port: u16,
        user: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    pub name: String,
    pub address: String,
    pub port: u16,
    /// `password`, `no-password` or `credential`.
    pub auth: String,
    /// Empty for a host using a credential: the credential's user is the
    /// one it logs in as, and renaming it must not change every host.
    pub user: String,
    pub credential: Option<String>,
    pub route: RouteRecord,
    pub group: Option<String>,
    pub notes: String,
    pub sort_order: i64,
    pub password: Option<SecretText>,
    pub proxy_password: Option<SecretText>,
    pub local_bookmarks: Vec<String>,
    pub remote_bookmarks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardRecord {
    pub host: String,
    pub name: String,
    /// `local`, `remote` or `dynamic`. Not `kind` in the data, as with a
    /// credential's.
    #[serde(rename = "forward_kind")]
    pub kind: String,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
    pub auto_start: bool,
    pub sort_order: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryRecord {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetRecord {
    pub category: Option<String>,
    pub name: String,
    pub command: String,
    pub run_on_click: bool,
    pub sort_order: i64,
}

/// One synced item. Its kind travels inside the sealed data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    Group(GroupRecord),
    Credential(CredentialRecord),
    // Boxed: by far the largest.
    Host(Box<HostRecord>),
    Forward(ForwardRecord),
    SnippetCategory(CategoryRecord),
    Snippet(SnippetRecord),
    /// `settings.json` as a whole.
    Settings {
        settings: serde_json::Value,
    },
    /// The hosts whose keys were accepted, one line each, sorted.
    KnownHosts {
        lines: Vec<String>,
    },
}

impl Payload {
    pub fn to_bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(serde_json::to_vec(self).expect("a payload always serializes"))
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }

    /// What makes two items made on different devices the same item, for
    /// the first time they meet: a host by where and as whom it logs in, a
    /// group by its path, and so on. `lookup` finds the items this one
    /// refers to. The settings and known hosts have one id everywhere and
    /// need none.
    pub fn natural_key<'a>(&self, lookup: &impl Fn(&str) -> Option<&'a Payload>) -> Option<String> {
        let key_of = |id: &Option<String>| {
            id.as_deref()
                .and_then(lookup)
                .and_then(|payload| payload.natural_key(lookup))
                .unwrap_or_default()
        };
        Some(match self {
            Payload::Group(group) => format!("group\0{}\0{}", key_of(&group.parent), group.name),
            Payload::Credential(credential) => format!(
                "credential\0{}\0{}\0{}",
                credential.kind, credential.name, credential.user
            ),
            Payload::Host(host) => format!(
                "host\0{}\0{}\0{}\0{}",
                host.name, host.address, host.port, host.user
            ),
            Payload::Forward(forward) => format!(
                "forward\0{}\0{}\0{}\0{}",
                key_of(&Some(forward.host.clone())),
                forward.kind,
                forward.bind_host,
                forward.bind_port
            ),
            Payload::SnippetCategory(category) => format!("category\0{}", category.name),
            Payload::Snippet(snippet) => format!(
                "snippet\0{}\0{}\0{}",
                key_of(&snippet.category),
                snippet.name,
                snippet.command
            ),
            Payload::Settings { .. } | Payload::KnownHosts { .. } => return None,
        })
    }

    /// Point this item's references at other ids: those of `renamed` that
    /// it uses.
    pub fn rename_references(&mut self, renamed: &HashMap<String, String>) {
        let rename = |id: &mut String| {
            if let Some(new) = renamed.get(id) {
                *id = new.clone();
            }
        };
        match self {
            Payload::Group(group) => group.parent.iter_mut().for_each(rename),
            Payload::Host(host) => {
                host.group.iter_mut().for_each(rename);
                host.credential.iter_mut().for_each(rename);
                if let RouteRecord::Jump { hops } = &mut host.route {
                    hops.iter_mut().flatten().for_each(rename);
                }
            }
            Payload::Forward(forward) => rename(&mut forward.host),
            Payload::Snippet(snippet) => snippet.category.iter_mut().for_each(rename),
            Payload::Credential(_)
            | Payload::SnippetCategory(_)
            | Payload::Settings { .. }
            | Payload::KnownHosts { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, group: Option<&str>) -> Payload {
        Payload::Host(Box::new(HostRecord {
            name: name.into(),
            address: "10.0.0.1".into(),
            port: 22,
            auth: "password".into(),
            user: "root".into(),
            credential: None,
            route: RouteRecord::Jump {
                hops: vec![Some("jump".into()), None],
            },
            group: group.map(Into::into),
            notes: String::new(),
            sort_order: 0,
            password: Some(SecretText::new("hunter2")),
            proxy_password: None,
            local_bookmarks: vec![],
            remote_bookmarks: vec!["/var/log".into()],
        }))
    }

    fn group(name: &str, parent: Option<&str>) -> Payload {
        Payload::Group(GroupRecord {
            name: name.into(),
            parent: parent.map(Into::into),
            sort_order: 0,
        })
    }

    #[test]
    fn a_payload_round_trips_and_says_its_kind() {
        let payload = host("web", Some("g1"));
        let bytes = payload.to_bytes();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.starts_with(r#"{"kind":"host","#), "{text}");
        assert_eq!(Payload::from_bytes(&bytes), Some(payload));
        assert_eq!(Payload::from_bytes(b"{\"kind\":\"spaceship\"}"), None);
    }

    /// Every kind of record reads back as it was written: a field of its own
    /// named like the payload's tag would make it unreadable.
    #[test]
    fn every_kind_of_record_round_trips() {
        let secret = |text: &str| Some(SecretText::new(text));
        let payloads = [
            host("web", Some("g1")),
            Payload::Group(GroupRecord {
                name: "生产".into(),
                parent: None,
                sort_order: 0,
            }),
            Payload::Credential(CredentialRecord {
                name: "cong".into(),
                kind: "key".into(),
                user: "root".into(),
                sort_order: 0,
                password: None,
                private_key: secret("-----BEGIN RSA PRIVATE KEY-----"),
                passphrase: secret("passphrase"),
            }),
            Payload::Forward(ForwardRecord {
                host: "h1".into(),
                name: "web".into(),
                kind: "local".into(),
                bind_host: "127.0.0.1".into(),
                bind_port: 8080,
                target_host: Some("localhost".into()),
                target_port: Some(80),
                auto_start: false,
                sort_order: 0,
            }),
            Payload::SnippetCategory(CategoryRecord {
                name: "Docker".into(),
            }),
            Payload::Snippet(SnippetRecord {
                category: Some("k1".into()),
                name: "容器".into(),
                command: "docker ps -a".into(),
                run_on_click: true,
                sort_order: 0,
            }),
            Payload::Settings {
                settings: serde_json::json!({ "appearance": "dark" }),
            },
            Payload::KnownHosts {
                lines: vec!["bastion.test ssh-ed25519 AAAA".into()],
            },
        ];
        for payload in payloads {
            assert_eq!(
                Payload::from_bytes(&payload.to_bytes()),
                Some(payload.clone()),
                "{payload:?}"
            );
        }
    }

    #[test]
    fn secrets_stay_out_of_debug_output() {
        let text = format!("{:?}", host("web", None));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("SecretText(..)"));
    }

    #[test]
    fn a_group_is_known_by_its_path() {
        let items: HashMap<&str, Payload> = [
            ("a", group("生产", None)),
            ("b", group("web", Some("a"))),
            ("c", group("测试", None)),
            ("d", group("web", Some("c"))),
        ]
        .into();
        let lookup = |id: &str| items.get(id);
        let key = |id: &str| items[id].natural_key(&lookup);
        assert_ne!(key("b"), key("d"));
        assert_eq!(key("b"), group("web", Some("a")).natural_key(&lookup));
    }

    #[test]
    fn a_host_is_known_by_where_and_as_whom_it_logs_in() {
        let none = |_: &str| None;
        assert_eq!(
            host("web", Some("g1")).natural_key(&none),
            host("web", Some("g2")).natural_key(&none)
        );
        assert_ne!(
            host("web", None).natural_key(&none),
            host("db", None).natural_key(&none)
        );
        let settings = Payload::Settings {
            settings: serde_json::json!({}),
        };
        assert_eq!(settings.natural_key(&none), None);
    }

    #[test]
    fn references_follow_renamed_ids() {
        let mut payload = host("web", Some("g1"));
        payload.rename_references(&[("g1".into(), "G".into()), ("jump".into(), "J".into())].into());
        let Payload::Host(host) = payload else {
            unreachable!()
        };
        assert_eq!(host.group.as_deref(), Some("G"));
        assert_eq!(
            host.route,
            RouteRecord::Jump {
                hops: vec![Some("J".into()), None]
            }
        );
    }
}
