//! 云同步's way into the store: items found and written by the id they have
//! on every device. Edits go through the same mutators the dialogs use, so
//! reconnecting, restarting forwards and cleaning up the keychain happen as
//! they would for a change made here.

use gpui_kit::Context;

use super::{Change, HostStore};
use crate::host::{
    BookmarkSide, CredentialDraft, CredentialId, ForwardDraft, ForwardId, GroupDraft, GroupId,
    HostDraft, HostId, PublicId, SnippetCategoryId, SnippetDraft, SnippetId,
};

impl HostStore {
    pub fn group_by_sync_id(&self, sync_id: &str) -> Option<GroupId> {
        self.groups
            .iter()
            .find(|group| group.sync_id.as_str() == sync_id)
            .map(|group| group.id)
    }

    pub fn host_by_sync_id(&self, sync_id: &str) -> Option<HostId> {
        self.hosts
            .iter()
            .find(|host| host.sync_id.as_str() == sync_id)
            .map(|host| host.id)
    }

    pub fn credential_by_sync_id(&self, sync_id: &str) -> Option<CredentialId> {
        self.credentials
            .iter()
            .find(|credential| credential.sync_id.as_str() == sync_id)
            .map(|credential| credential.id)
    }

    pub fn forward_by_sync_id(&self, sync_id: &str) -> Option<ForwardId> {
        self.forwards
            .iter()
            .find(|rule| rule.sync_id.as_str() == sync_id)
            .map(|rule| rule.id)
    }

    pub fn snippet_category_by_sync_id(&self, sync_id: &str) -> Option<SnippetCategoryId> {
        self.snippet_categories
            .iter()
            .find(|category| category.sync_id.as_str() == sync_id)
            .map(|category| category.id)
    }

    pub fn snippet_by_sync_id(&self, sync_id: &str) -> Option<SnippetId> {
        self.snippets
            .iter()
            .find(|snippet| snippet.sync_id.as_str() == sync_id)
            .map(|snippet| snippet.id)
    }

    /// Give the item known as `from` the id `to`: the server already has it
    /// from another device. Says whether there was such an item.
    pub fn rename_sync_id(&mut self, from: &str, to: &str, cx: &mut Context<Self>) -> bool {
        let to = PublicId::from_stored(to.to_owned());
        let found = self.find_synced(from);
        let Some((table, id)) = found else {
            return false;
        };
        match table {
            "groups" => set(
                self.groups.iter_mut().map(|g| (g.id.0, &mut g.sync_id)),
                id,
                &to,
            ),
            "credentials" => set(
                self.credentials
                    .iter_mut()
                    .map(|c| (c.id.0, &mut c.sync_id)),
                id,
                &to,
            ),
            "hosts" => set(
                self.hosts.iter_mut().map(|h| (h.id.0, &mut h.sync_id)),
                id,
                &to,
            ),
            "forwards" => set(
                self.forwards.iter_mut().map(|r| (r.id.0, &mut r.sync_id)),
                id,
                &to,
            ),
            "snippet_categories" => set(
                self.snippet_categories
                    .iter_mut()
                    .map(|c| (c.id.0, &mut c.sync_id)),
                id,
                &to,
            ),
            _ => set(
                self.snippets.iter_mut().map(|s| (s.id.0, &mut s.sync_id)),
                id,
                &to,
            ),
        }
        self.persist(Change::Synced, cx, |db| db.set_sync_id(table, id, &to));
        cx.notify();
        true
    }

    /// The table and row of the item with this sync id.
    fn find_synced(&self, sync_id: &str) -> Option<(&'static str, u64)> {
        self.group_by_sync_id(sync_id)
            .map(|id| ("groups", id.0))
            .or_else(|| {
                self.credential_by_sync_id(sync_id)
                    .map(|id| ("credentials", id.0))
            })
            .or_else(|| self.host_by_sync_id(sync_id).map(|id| ("hosts", id.0)))
            .or_else(|| {
                self.forward_by_sync_id(sync_id)
                    .map(|id| ("forwards", id.0))
            })
            .or_else(|| {
                self.snippet_category_by_sync_id(sync_id)
                    .map(|id| ("snippet_categories", id.0))
            })
            .or_else(|| {
                self.snippet_by_sync_id(sync_id)
                    .map(|id| ("snippets", id.0))
            })
    }

    pub fn upsert_synced_group(
        &mut self,
        sync_id: &str,
        draft: GroupDraft,
        sort_order: i64,
        cx: &mut Context<Self>,
    ) -> GroupId {
        let id = match self.group_by_sync_id(sync_id) {
            Some(id) => {
                self.update_group(id, draft, cx);
                id
            }
            None => {
                let id = self.insert_group_unnotified(draft);
                let group = self.groups.last_mut().expect("just added");
                group.sync_id = synced_id(sync_id);
                group.sort_order = sort_order;
                if let Some(group) = self.group(id) {
                    self.persist(Change::Synced, cx, |db| db.insert_group(group));
                }
                id
            }
        };
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == id)
            && group.sort_order != sort_order
        {
            group.sort_order = sort_order;
            if let Some(group) = self.group(id) {
                self.persist(Change::Synced, cx, |db| db.update_group(group));
            }
        }
        cx.notify();
        id
    }

    pub fn upsert_synced_credential(
        &mut self,
        sync_id: &str,
        draft: CredentialDraft,
        sort_order: i64,
        cx: &mut Context<Self>,
    ) -> CredentialId {
        let id = match self.credential_by_sync_id(sync_id) {
            Some(id) => {
                self.update_credential(id, draft, cx);
                id
            }
            None => {
                let id = self.insert_credential_unnotified(draft);
                let credential = self.credentials.last_mut().expect("just added");
                credential.sync_id = synced_id(sync_id);
                credential.sort_order = sort_order;
                self.credentials
                    .sort_by_key(|credential| credential.sort_order);
                if let Some(credential) = self.credential(id) {
                    self.persist(Change::Synced, cx, |db| db.insert_credential(credential));
                }
                id
            }
        };
        self.set_credential_order(id, sort_order, cx);
        cx.notify();
        id
    }

    fn set_credential_order(&mut self, id: CredentialId, sort_order: i64, cx: &mut Context<Self>) {
        let Some(credential) = self.credentials.iter_mut().find(|c| c.id == id) else {
            return;
        };
        if credential.sort_order == sort_order {
            return;
        }
        credential.sort_order = sort_order;
        self.credentials
            .sort_by_key(|credential| credential.sort_order);
        if let Some(credential) = self.credential(id) {
            self.persist(Change::Synced, cx, |db| db.update_credential(credential));
        }
    }

    /// A host from another device, with its place among its group's hosts
    /// and its bookmarks.
    pub fn upsert_synced_host(
        &mut self,
        sync_id: &str,
        draft: HostDraft,
        sort_order: i64,
        bookmarks: [Vec<String>; 2],
        cx: &mut Context<Self>,
    ) -> HostId {
        let id = match self.host_by_sync_id(sync_id) {
            Some(id) => {
                self.update(id, draft, cx);
                id
            }
            None => {
                let id = self.insert_unnotified(draft);
                let host = self.hosts.last_mut().expect("just added");
                host.sync_id = synced_id(sync_id);
                host.sort_order = sort_order;
                if let Some(host) = self.host(id) {
                    self.persist(Change::Synced, cx, |db| db.insert_host(host));
                }
                id
            }
        };
        if let Some(host) = self.hosts.iter_mut().find(|host| host.id == id)
            && host.sort_order != sort_order
        {
            host.sort_order = sort_order;
            if let Some(host) = self.host(id) {
                self.persist(Change::Synced, cx, |db| db.update_host(host));
            }
        }
        let [local, remote] = bookmarks;
        self.set_bookmarks(id, BookmarkSide::Local, local, cx);
        self.set_bookmarks(id, BookmarkSide::Remote, remote, cx);
        cx.notify();
        id
    }

    fn set_bookmarks(
        &mut self,
        host: HostId,
        side: BookmarkSide,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.bookmarks(host, side) == paths.as_slice() {
            return;
        }
        let old: Vec<String> = self.bookmarks(host, side).to_vec();
        for path in old.iter().filter(|path| !paths.contains(path)) {
            self.persist(Change::Synced, cx, |db| {
                db.remove_bookmark(host, side, path)
            });
        }
        for path in paths.iter().filter(|path| !old.contains(path)) {
            self.persist(Change::Synced, cx, |db| {
                db.insert_bookmark(host, side, path)
            });
        }
        self.persist(Change::Synced, cx, |db| {
            db.set_bookmark_order(host, side, &paths)
        });
        if paths.is_empty() {
            self.bookmarks.remove(&(host, side));
        } else {
            self.bookmarks.insert((host, side), paths);
        }
    }

    pub fn upsert_synced_forward(
        &mut self,
        sync_id: &str,
        draft: ForwardDraft,
        sort_order: i64,
        cx: &mut Context<Self>,
    ) -> Option<ForwardId> {
        let id = match self.forward_by_sync_id(sync_id) {
            Some(id) => {
                self.update_forward(id, draft, cx);
                id
            }
            None => {
                let id = self.insert_forward_unnotified(draft)?;
                let rule = self.forwards.last_mut().expect("just added");
                rule.sync_id = synced_id(sync_id);
                rule.sort_order = sort_order;
                self.forwards.sort_by_key(|rule| rule.sort_order);
                if let Some(rule) = self.forward(id) {
                    self.persist(Change::Synced, cx, |db| db.insert_forward(rule));
                }
                id
            }
        };
        if let Some(rule) = self.forwards.iter_mut().find(|rule| rule.id == id)
            && rule.sort_order != sort_order
        {
            rule.sort_order = sort_order;
            self.forwards.sort_by_key(|rule| rule.sort_order);
            if let Some(rule) = self.forward(id) {
                self.persist(Change::Synced, cx, |db| db.update_forward(rule));
            }
        }
        cx.notify();
        Some(id)
    }

    pub fn upsert_synced_snippet_category(
        &mut self,
        sync_id: &str,
        name: String,
        cx: &mut Context<Self>,
    ) -> SnippetCategoryId {
        if let Some(id) = self.snippet_category_by_sync_id(sync_id) {
            self.rename_snippet_category(id, name, cx);
            return id;
        }
        let id = self.insert_snippet_category_unnotified(name);
        self.snippet_categories
            .last_mut()
            .expect("just added")
            .sync_id = synced_id(sync_id);
        if let Some(category) = self.snippet_category(id) {
            self.persist(Change::Synced, cx, |db| {
                db.insert_snippet_category(category)
            });
        }
        cx.notify();
        id
    }

    pub fn upsert_synced_snippet(
        &mut self,
        sync_id: &str,
        draft: SnippetDraft,
        sort_order: i64,
        cx: &mut Context<Self>,
    ) -> SnippetId {
        let id = match self.snippet_by_sync_id(sync_id) {
            Some(id) => {
                self.update_snippet(id, draft, cx);
                id
            }
            None => {
                let id = self.insert_snippet_unnotified(draft);
                let snippet = self.snippets.last_mut().expect("just added");
                snippet.sync_id = synced_id(sync_id);
                snippet.sort_order = sort_order;
                if let Some(snippet) = self.snippet(id) {
                    self.persist(Change::Synced, cx, |db| db.insert_snippet(snippet));
                }
                id
            }
        };
        if let Some(snippet) = self.snippets.iter_mut().find(|snippet| snippet.id == id)
            && snippet.sort_order != sort_order
        {
            snippet.sort_order = sort_order;
            if let Some(snippet) = self.snippet(id) {
                self.persist(Change::Synced, cx, |db| db.update_snippet(snippet));
            }
        }
        cx.notify();
        id
    }
}

fn synced_id(sync_id: &str) -> PublicId {
    PublicId::from_stored(sync_id.to_owned())
}

/// Set the sync id of the item `id` among `items`.
fn set<'a>(items: impl Iterator<Item = (u64, &'a mut PublicId)>, id: u64, to: &PublicId) {
    for (item, sync_id) in items {
        if item == id {
            *sync_id = to.clone();
        }
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
