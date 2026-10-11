//! Carrying out what a round brought from the server, on the UI thread:
//! into the store, the keychain, the key files, the settings and the known
//! hosts file.
//!
//! Items go in in the order they refer to each other (credentials and
//! groups, then hosts, then what refers to hosts) and leave in the other.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gpui_kit::{App, Context, Entity};

use crate::host::{
    AuthKind, CredentialDraft, CredentialKind, ForwardDraft, ForwardEndpoint, ForwardKind,
    GroupDraft, HostDraft, HostId, HostStore, ProxyKind, ProxySettings, Route, SnippetDraft,
};
use crate::secrets::SecretRef;
use crate::settings::{AppSettings, SettingsStore};

use super::record::{
    CredentialRecord, ForwardRecord, GroupRecord, HostRecord, Payload, RouteRecord, SecretText,
    SnippetRecord,
};

/// The items a round changes here, the last word for each id.
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub upserts: Vec<(String, Payload)>,
    pub deletions: Vec<String>,
}

impl Changes {
    pub fn new(apply: Vec<(String, Option<Payload>)>) -> Self {
        let mut last: HashMap<String, Option<Payload>> = HashMap::new();
        let mut order = Vec::new();
        for (id, payload) in apply {
            if last.insert(id.clone(), payload).is_none() {
                order.push(id);
            }
        }
        let mut changes = Self::default();
        for id in order {
            match last.remove(&id).flatten() {
                Some(payload) => changes.upserts.push((id, payload)),
                None => changes.deletions.push(id),
            }
        }
        changes
    }

    /// The hosts these changes delete, those in deleted groups included:
    /// their tabs close first.
    pub fn removed_hosts(&self, store: &HostStore) -> Vec<HostId> {
        let mut hosts: HashSet<HostId> = HashSet::new();
        for id in &self.deletions {
            if let Some(host) = store.host_by_sync_id(id) {
                hosts.insert(host);
            }
            if let Some(group) = store.group_by_sync_id(id) {
                hosts.extend(store.hosts_under(group));
            }
        }
        hosts.into_iter().collect()
    }
}

/// Where the round's changes go.
pub struct Targets<'a> {
    pub store: &'a Entity<HostStore>,
    pub settings: &'a Entity<SettingsStore>,
    pub known_hosts: Option<&'a Path>,
}

pub fn apply(changes: Changes, targets: &Targets, cx: &mut App) {
    let of_kind = |kind: fn(&Payload) -> bool| {
        changes
            .upserts
            .iter()
            .filter(move |(_, payload)| kind(payload))
    };
    targets.store.update(cx, |store, cx| {
        for (id, payload) in of_kind(|p| matches!(p, Payload::Credential(_))) {
            if let Payload::Credential(record) = payload {
                upsert_credential(store, id, record, cx);
            }
        }
        upsert_groups(store, of_kind(|p| matches!(p, Payload::Group(_))), cx);
        upsert_hosts(store, of_kind(|p| matches!(p, Payload::Host(_))), cx);
        for (id, payload) in &changes.upserts {
            match payload {
                Payload::Forward(record) => upsert_forward(store, id, record, cx),
                Payload::SnippetCategory(record) => {
                    store.upsert_synced_snippet_category(id, record.name.clone(), cx);
                }
                _ => {}
            }
        }
        for (id, payload) in of_kind(|p| matches!(p, Payload::Snippet(_))) {
            if let Payload::Snippet(record) = payload {
                upsert_snippet(store, id, record, cx);
            }
        }
        delete(store, &changes.deletions, cx);
    });
    for (_, payload) in &changes.upserts {
        match payload {
            Payload::Settings { settings } => apply_settings(targets.settings, settings, cx),
            Payload::KnownHosts { lines } => {
                if let Some(path) = targets.known_hosts {
                    write_known_hosts(targets.store, path, lines, cx);
                }
            }
            _ => {}
        }
    }
}

fn secret_value(text: &Option<SecretText>) -> Option<String> {
    text.as_ref().map(|text| text.as_str().to_owned())
}

fn upsert_credential(
    store: &mut HostStore,
    id: &str,
    record: &CredentialRecord,
    cx: &mut Context<HostStore>,
) {
    let Some(kind) = CredentialKind::from_stored(&record.kind) else {
        return;
    };
    let existing = store.credential_by_sync_id(id);
    let mut draft = CredentialDraft::new(record.name.clone(), kind, record.user.clone());
    if kind == CredentialKind::Key {
        let Some(path) = key_path(store, existing, record) else {
            return;
        };
        draft.key_path = Some(path.clone().into());
        store.save_secret(
            SecretRef::passphrase(&path),
            secret_value(&record.passphrase),
            cx,
        );
    }
    let local = store.upsert_synced_credential(id, draft, record.sort_order, cx);
    if kind == CredentialKind::Password
        && let Some(credential) = store.credential(local)
    {
        let secret = credential.password_secret();
        store.save_secret(secret, secret_value(&record.password), cx);
    }
}

/// Where the credential's key is: the file it uses now when that already
/// holds the key, otherwise a file of ShellRS's own written with it.
fn key_path(
    store: &HostStore,
    existing: Option<crate::host::CredentialId>,
    record: &CredentialRecord,
) -> Option<String> {
    let text = record.private_key.as_ref()?.as_str();
    let current = existing
        .and_then(|id| store.credential(id))
        .and_then(|credential| credential.key_path.as_ref())
        .map(ToString::to_string);
    if let Some(path) = &current
        && std::fs::read_to_string(path).is_ok_and(|held| held == text)
    {
        return current;
    }
    store
        .save_private_key(existing, text)
        .ok()
        .map(|path| path.to_string())
}

/// Parents before their subgroups, whatever order they came in.
fn upsert_groups<'a>(
    store: &mut HostStore,
    groups: impl Iterator<Item = &'a (String, Payload)>,
    cx: &mut Context<HostStore>,
) {
    let mut pending: Vec<(&String, &GroupRecord)> = groups
        .filter_map(|(id, payload)| match payload {
            Payload::Group(record) => Some((id, record)),
            _ => None,
        })
        .collect();
    while !pending.is_empty() {
        let waiting: HashSet<&str> = pending.iter().map(|(id, _)| id.as_str()).collect();
        let (ready, later): (Vec<_>, Vec<_>) = pending.into_iter().partition(|(_, record)| {
            record
                .parent
                .as_deref()
                .is_none_or(|parent| !waiting.contains(parent))
        });
        // A cycle, which no device makes: put them all in.
        let (ready, later) = if ready.is_empty() {
            (later, Vec::new())
        } else {
            (ready, later)
        };
        for (id, record) in ready {
            let parent = record
                .parent
                .as_deref()
                .and_then(|parent| store.group_by_sync_id(parent));
            let draft = GroupDraft::new(record.name.clone(), parent);
            store.upsert_synced_group(id, draft, record.sort_order, cx);
        }
        pending = later;
    }
}

/// Twice: once so every host is there, once more so a jump route can name
/// hosts that came in the same round.
fn upsert_hosts<'a>(
    store: &mut HostStore,
    hosts: impl Iterator<Item = &'a (String, Payload)>,
    cx: &mut Context<HostStore>,
) {
    let hosts: Vec<(&String, &HostRecord)> = hosts
        .filter_map(|(id, payload)| match payload {
            Payload::Host(record) => Some((id, record.as_ref())),
            _ => None,
        })
        .collect();
    for (id, record) in &hosts {
        if store.host_by_sync_id(id).is_none() {
            let mut draft = host_draft(store, record);
            draft.route = Route::Direct;
            store.upsert_synced_host(id, draft, record.sort_order, [vec![], vec![]], cx);
        }
    }
    for (id, record) in hosts {
        let draft = host_draft(store, record);
        let password = draft.credential.is_none() && draft.auth == AuthKind::Password;
        let bookmarks = [
            record.local_bookmarks.clone(),
            record.remote_bookmarks.clone(),
        ];
        let local = store.upsert_synced_host(id, draft, record.sort_order, bookmarks, cx);
        let Some(host) = store.host(local) else {
            continue;
        };
        let endpoint = host.password_secret();
        let proxy = match &host.route {
            Route::Proxy(proxy) => proxy.password_secret(),
            _ => None,
        };
        if password {
            store.save_secret(endpoint, secret_value(&record.password), cx);
        }
        if let Some(proxy) = proxy {
            store.save_secret(proxy, secret_value(&record.proxy_password), cx);
        }
    }
}

fn host_draft(store: &HostStore, record: &HostRecord) -> HostDraft {
    let group = record
        .group
        .as_deref()
        .and_then(|id| store.group_by_sync_id(id));
    let mut draft = HostDraft::new(
        record.name.clone(),
        record.address.clone(),
        record.port,
        record.user.clone(),
        AuthKind::from_stored(&record.auth),
        group,
    )
    .with_notes(record.notes.clone());
    draft.credential = record
        .credential
        .as_deref()
        .and_then(|id| store.credential_by_sync_id(id));
    draft.route = match &record.route {
        RouteRecord::Direct => Route::Direct,
        RouteRecord::Jump { hops } => Route::Jump(
            hops.iter()
                .map(|hop| hop.as_deref().and_then(|id| store.host_by_sync_id(id)))
                .collect(),
        ),
        RouteRecord::Proxy {
            kind,
            host,
            port,
            user,
        } => ProxyKind::from_stored(kind).map_or(Route::Direct, |kind| {
            Route::Proxy(
                ProxySettings::new(kind, host.clone(), *port)
                    .with_user(user.clone().unwrap_or_default()),
            )
        }),
    };
    draft
}

fn upsert_forward(
    store: &mut HostStore,
    id: &str,
    record: &ForwardRecord,
    cx: &mut Context<HostStore>,
) {
    let (Some(kind), Some(host)) = (
        ForwardKind::from_stored(&record.kind),
        store.host_by_sync_id(&record.host),
    ) else {
        return;
    };
    let target = record
        .target_host
        .clone()
        .zip(record.target_port)
        .map(|(host, port)| ForwardEndpoint::new(host, port));
    let draft = ForwardDraft::new(
        kind,
        host,
        ForwardEndpoint::new(record.bind_host.clone(), record.bind_port),
        target,
    )
    .with_name(record.name.clone())
    .with_auto_start(record.auto_start);
    store.upsert_synced_forward(id, draft, record.sort_order, cx);
}

fn upsert_snippet(
    store: &mut HostStore,
    id: &str,
    record: &SnippetRecord,
    cx: &mut Context<HostStore>,
) {
    let category = record
        .category
        .as_deref()
        .and_then(|id| store.snippet_category_by_sync_id(id));
    let draft = SnippetDraft::new(record.name.clone(), record.command.clone(), category)
        .with_run_on_click(record.run_on_click);
    store.upsert_synced_snippet(id, draft, record.sort_order, cx);
}

/// What refers to others first: snippets and forwards, then hosts, then
/// groups and credentials.
fn delete(store: &mut HostStore, ids: &[String], cx: &mut Context<HostStore>) {
    for id in ids {
        if let Some(snippet) = store.snippet_by_sync_id(id) {
            store.remove_snippet(snippet, cx);
        } else if let Some(rule) = store.forward_by_sync_id(id) {
            store.remove_forward(rule, cx);
        }
    }
    for id in ids {
        if let Some(category) = store.snippet_category_by_sync_id(id) {
            store.remove_snippet_category(category, cx);
        } else if let Some(host) = store.host_by_sync_id(id) {
            store.remove(host, cx);
        }
    }
    for id in ids {
        if let Some(group) = store.group_by_sync_id(id) {
            store.remove_group(group, cx);
        } else if let Some(credential) = store.credential_by_sync_id(id) {
            store.remove_credential(credential, cx);
        }
    }
}

/// Settings another device wrote. Ones this build cannot read (a newer
/// version's) are left for the next version.
fn apply_settings(settings: &Entity<SettingsStore>, value: &serde_json::Value, cx: &mut App) {
    let Ok(new) = serde_json::from_value::<AppSettings>(value.clone()) else {
        return;
    };
    settings.update(cx, |settings, cx| {
        if settings.settings() != new {
            settings.update(|current| *current = new, cx);
        }
    });
}

/// Replace the known hosts file whole, through a temporary one.
fn write_known_hosts(store: &Entity<HostStore>, path: &Path, lines: &[String], cx: &mut App) {
    let mut text = lines.join("\n");
    text.push('\n');
    let temporary = path.with_extension("sync.tmp");
    let written = std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, path));
    if let Err(error) = written {
        store.update(cx, |_, cx| {
            cx.emit(crate::host::HostStoreEvent::PersistFailed(crate::i18n::t!(
                "sync.known_hosts_not_saved",
                error = error
            )));
        });
    }
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
