//! This device's items as records: taken from the store on the UI thread
//! without their secrets, which the worker then reads from the keychain
//! and the key files (the keychain blocks, and may ask the user).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::host::{
    AuthKind, BookmarkSide, Credential, CredentialKind, ForwardRule, Host, HostGroup, HostStore,
    Route, Snippet, SnippetCategory,
};
use crate::secrets::{SecretRef, SecretStore};

use super::record::{
    CategoryRecord, CredentialRecord, ForwardRecord, GroupRecord, HostRecord, KNOWN_HOSTS_ID,
    Payload, RouteRecord, SETTINGS_ID, SecretText, SnippetRecord,
};

/// A secret an item holds that only the worker may read.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Read {
    HostPassword(SecretRef),
    ProxyPassword(SecretRef),
    CredentialPassword(SecretRef),
    Passphrase(SecretRef),
    PrivateKey(PathBuf),
}

/// An item as the store has it, its secrets still to be read.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalItem {
    pub id: String,
    pub payload: Payload,
    reads: Vec<Read>,
}

/// Every item of this device, as the UI thread sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalSnapshot {
    pub items: Vec<LocalItem>,
    pub known_hosts: Option<PathBuf>,
}

impl LocalSnapshot {
    pub fn of(
        store: &HostStore,
        settings: serde_json::Value,
        known_hosts: Option<PathBuf>,
    ) -> Self {
        let mut items: Vec<LocalItem> = Vec::new();
        items.extend(store.groups().iter().map(|group| group_item(store, group)));
        items.extend(store.credentials().iter().map(credential_item));
        items.extend(store.hosts().iter().map(|host| host_item(store, host)));
        items.extend(
            store
                .forwards()
                .iter()
                .filter_map(|rule| forward_item(store, rule)),
        );
        items.extend(store.snippet_categories().iter().map(category_item));
        items.extend(
            store
                .snippets()
                .iter()
                .map(|snippet| snippet_item(store, snippet)),
        );
        items.push(LocalItem {
            id: SETTINGS_ID.into(),
            payload: Payload::Settings { settings },
            reads: Vec::new(),
        });
        Self { items, known_hosts }
    }

    /// The item with this id, as the store has it now, secrets left out:
    /// for telling whether it changed while the worker ran.
    pub fn item(&self, id: &str) -> Option<&LocalItem> {
        self.items.iter().find(|item| item.id == id)
    }

    /// Read the secrets and the known hosts. An item whose secret could not
    /// be read is left out and named, so the round neither sends it without
    /// its secret nor takes it as deleted.
    pub fn read(self, secrets: &dyn SecretStore) -> (HashMap<String, Payload>, HashSet<String>) {
        let mut local = HashMap::new();
        let mut unreadable = HashSet::new();
        for mut item in self.items {
            if fill(&mut item, secrets) {
                local.insert(item.id, item.payload);
            } else {
                unreadable.insert(item.id);
            }
        }
        match self.known_hosts.as_deref().map(read_known_hosts) {
            Some(Ok(lines)) => {
                local.insert(KNOWN_HOSTS_ID.into(), Payload::KnownHosts { lines });
            }
            Some(Err(_)) => {
                unreadable.insert(KNOWN_HOSTS_ID.into());
            }
            None => {}
        }
        (local, unreadable)
    }
}

/// The lines of a known hosts file, sorted, without blanks and comments.
/// A file not there yet has none.
pub fn read_known_hosts(path: &Path) -> std::io::Result<Vec<String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines.dedup();
    Ok(lines)
}

fn fill(item: &mut LocalItem, secrets: &dyn SecretStore) -> bool {
    for read in &item.reads {
        let value = match read {
            Read::PrivateKey(path) => match std::fs::read_to_string(path) {
                Ok(text) => Some(SecretText::new(text)),
                Err(_) => return false,
            },
            Read::HostPassword(secret)
            | Read::ProxyPassword(secret)
            | Read::CredentialPassword(secret)
            | Read::Passphrase(secret) => match secrets.get(secret) {
                Ok(value) => value.map(|value| SecretText::new(value.as_str())),
                Err(_) => return false,
            },
        };
        match (&mut item.payload, read) {
            (Payload::Host(host), Read::HostPassword(_)) => host.password = value,
            (Payload::Host(host), Read::ProxyPassword(_)) => host.proxy_password = value,
            (Payload::Credential(credential), Read::CredentialPassword(_)) => {
                credential.password = value
            }
            (Payload::Credential(credential), Read::Passphrase(_)) => credential.passphrase = value,
            (Payload::Credential(credential), Read::PrivateKey(_)) => {
                credential.private_key = value
            }
            _ => {}
        }
    }
    true
}

fn group_item(store: &HostStore, group: &HostGroup) -> LocalItem {
    LocalItem {
        id: group.sync_id.to_string(),
        payload: Payload::Group(GroupRecord {
            name: group.name.to_string(),
            parent: group
                .parent
                .and_then(|id| store.group(id))
                .map(|parent| parent.sync_id.to_string()),
            sort_order: group.sort_order,
        }),
        reads: Vec::new(),
    }
}

fn credential_item(credential: &Credential) -> LocalItem {
    let mut reads = Vec::new();
    match credential.kind {
        CredentialKind::Password => reads.push(Read::CredentialPassword(SecretRef::credential(
            credential.keychain_id.as_str(),
        ))),
        CredentialKind::Key => {
            if let Some(path) = &credential.key_path {
                reads.push(Read::PrivateKey(PathBuf::from(path.as_ref())));
                reads.push(Read::Passphrase(SecretRef::passphrase(path.as_ref())));
            }
        }
        CredentialKind::Agent => {}
    }
    LocalItem {
        id: credential.sync_id.to_string(),
        payload: Payload::Credential(CredentialRecord {
            name: credential.name.to_string(),
            kind: credential.kind.as_str().into(),
            user: credential.user.to_string(),
            sort_order: credential.sort_order,
            password: None,
            private_key: None,
            passphrase: None,
        }),
        reads,
    }
}

fn host_item(store: &HostStore, host: &Host) -> LocalItem {
    let credential = host.credential.and_then(|id| store.credential(id));
    let mut reads = Vec::new();
    if credential.is_none() && host.auth == AuthKind::Password {
        reads.push(Read::HostPassword(host.password_secret()));
    }
    let route = match &host.route {
        Route::Direct => RouteRecord::Direct,
        Route::Jump(hops) => RouteRecord::Jump {
            hops: hops
                .iter()
                .map(|hop| {
                    hop.and_then(|id| store.saved_host(id))
                        .map(|jump| jump.sync_id.to_string())
                })
                .collect(),
        },
        Route::Proxy(proxy) => {
            if let Some(secret) = proxy.password_secret() {
                reads.push(Read::ProxyPassword(secret));
            }
            RouteRecord::Proxy {
                kind: proxy.kind.as_str().into(),
                host: proxy.host.to_string(),
                port: proxy.port,
                user: proxy.user.as_ref().map(ToString::to_string),
            }
        }
    };
    let bookmarks = |side| {
        store
            .bookmarks(host.id, side)
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    LocalItem {
        id: host.sync_id.to_string(),
        payload: Payload::Host(Box::new(HostRecord {
            name: host.name.to_string(),
            address: host.address.to_string(),
            port: host.port,
            auth: if credential.is_some() {
                "credential".into()
            } else {
                host.auth.as_str().into()
            },
            user: if credential.is_some() {
                String::new()
            } else {
                host.user.to_string()
            },
            credential: credential.map(|credential| credential.sync_id.to_string()),
            route,
            group: host
                .group
                .and_then(|id| store.group(id))
                .map(|group| group.sync_id.to_string()),
            notes: host.notes.to_string(),
            sort_order: host.sort_order,
            password: None,
            proxy_password: None,
            local_bookmarks: bookmarks(BookmarkSide::Local),
            remote_bookmarks: bookmarks(BookmarkSide::Remote),
        })),
        reads,
    }
}

fn forward_item(store: &HostStore, rule: &ForwardRule) -> Option<LocalItem> {
    let host = store.saved_host(rule.host)?;
    Some(LocalItem {
        id: rule.sync_id.to_string(),
        payload: Payload::Forward(ForwardRecord {
            host: host.sync_id.to_string(),
            name: rule.name.to_string(),
            kind: rule.kind.as_str().into(),
            bind_host: rule.bind.host.to_string(),
            bind_port: rule.bind.port,
            target_host: rule.target.as_ref().map(|target| target.host.to_string()),
            target_port: rule.target.as_ref().map(|target| target.port),
            auto_start: rule.auto_start,
            sort_order: rule.sort_order,
        }),
        reads: Vec::new(),
    })
}

fn category_item(category: &SnippetCategory) -> LocalItem {
    LocalItem {
        id: category.sync_id.to_string(),
        payload: Payload::SnippetCategory(CategoryRecord {
            name: category.name.to_string(),
        }),
        reads: Vec::new(),
    }
}

fn snippet_item(store: &HostStore, snippet: &Snippet) -> LocalItem {
    LocalItem {
        id: snippet.sync_id.to_string(),
        payload: Payload::Snippet(SnippetRecord {
            category: snippet
                .category
                .and_then(|id| store.snippet_category(id))
                .map(|category| category.sync_id.to_string()),
            name: snippet.name.to_string(),
            command: snippet.command.clone(),
            run_on_click: snippet.run_on_click,
            sort_order: snippet.sort_order,
        }),
        reads: Vec::new(),
    }
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
