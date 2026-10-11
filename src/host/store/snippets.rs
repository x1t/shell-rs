//! The command snippets' part of `HostStore`: the snippets and their
//! categories, written through like everything else.

use gpui_kit::{Context, SharedString};

use super::{Change, HostStore};
use crate::host::{
    Snippet, SnippetCategory, SnippetCategoryId, SnippetDraft, SnippetDrop, SnippetId, SnippetScope,
};

impl HostStore {
    /// Every snippet, in the order they were made.
    pub fn snippets(&self) -> &[Snippet] {
        &self.snippets
    }

    pub fn snippet(&self, id: SnippetId) -> Option<&Snippet> {
        self.snippets.iter().find(|snippet| snippet.id == id)
    }

    /// Every snippet category, in the order they were made.
    pub fn snippet_categories(&self) -> &[SnippetCategory] {
        &self.snippet_categories
    }

    pub fn snippet_category(&self, id: SnippetCategoryId) -> Option<&SnippetCategory> {
        self.snippet_categories
            .iter()
            .find(|category| category.id == id)
    }

    /// How many snippets are in `category`: what deleting it takes along.
    pub fn snippets_in(&self, category: SnippetCategoryId) -> usize {
        self.snippets
            .iter()
            .filter(|snippet| snippet.category == Some(category))
            .count()
    }

    pub fn insert_snippet(&mut self, draft: SnippetDraft, cx: &mut Context<Self>) -> SnippetId {
        let id = self.insert_snippet_unnotified(draft);
        if let Some(snippet) = self.snippet(id) {
            self.persist(Change::NewSnippet, cx, |db| db.insert_snippet(snippet));
        }
        cx.notify();
        id
    }

    /// A new snippet goes last in its category.
    pub fn insert_snippet_unnotified(&mut self, draft: SnippetDraft) -> SnippetId {
        let id = SnippetId(self.next_snippet_id);
        self.next_snippet_id += 1;
        let draft = self.normalized_snippet(draft);
        let order = self.next_snippet_order(draft.category);
        self.snippets
            .push(Snippet::new(id, draft).with_sort_order(order));
        id
    }

    /// The sort order after every snippet in `category`.
    fn next_snippet_order(&self, category: Option<SnippetCategoryId>) -> i64 {
        self.snippets
            .iter()
            .filter(|snippet| snippet.category == category)
            .map(|snippet| snippet.sort_order + 1)
            .max()
            .unwrap_or(0)
    }

    /// Put a snippet where it was dragged, in that category, and renumber the
    /// category's snippets from 0. What changed is written through.
    pub fn move_snippet(&mut self, id: SnippetId, to: SnippetDrop, cx: &mut Context<Self>) {
        for moved in self.move_snippet_unnotified(id, to) {
            if let Some(snippet) = self.snippet(moved) {
                self.persist(Change::SaveSnippet, cx, |db| db.update_snippet(snippet));
            }
        }
        cx.notify();
    }

    /// [`Self::move_snippet`] in memory; the snippets whose place changed.
    pub fn move_snippet_unnotified(&mut self, id: SnippetId, to: SnippetDrop) -> Vec<SnippetId> {
        let category = match to {
            SnippetDrop::Into(category) => category,
            SnippetDrop::Before(other) | SnippetDrop::After(other) => match self.snippet(other) {
                Some(other) => other.category,
                None => return Vec::new(),
            },
        };
        if self.snippet(id).is_none()
            || to == SnippetDrop::Before(id)
            || to == SnippetDrop::After(id)
        {
            return Vec::new();
        }
        let mut order: Vec<&Snippet> = self
            .snippets
            .iter()
            .filter(|snippet| snippet.category == category && snippet.id != id)
            .collect();
        order.sort_by(|a, b| Snippet::in_order(a, b));
        let mut order: Vec<SnippetId> = order.into_iter().map(|snippet| snippet.id).collect();
        let at = match to {
            SnippetDrop::Before(other) => order.iter().position(|each| *each == other),
            SnippetDrop::After(other) => order
                .iter()
                .position(|each| *each == other)
                .map(|at| at + 1),
            SnippetDrop::Into(_) => None,
        };
        order.insert(at.unwrap_or(order.len()), id);
        let mut changed = Vec::new();
        for (place, each) in order.into_iter().enumerate() {
            let Some(snippet) = self.snippets.iter_mut().find(|snippet| snippet.id == each) else {
                continue;
            };
            let place = place as i64;
            if snippet.sort_order != place || snippet.category != category {
                snippet.sort_order = place;
                snippet.category = category;
                changed.push(each);
            }
        }
        changed
    }

    pub fn update_snippet(
        &mut self,
        id: SnippetId,
        draft: SnippetDraft,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.update_snippet_unnotified(id, draft) {
            return false;
        }
        if let Some(snippet) = self.snippet(id) {
            self.persist(Change::SaveSnippet, cx, |db| db.update_snippet(snippet));
        }
        cx.notify();
        true
    }

    /// An edit keeps the snippet's place, unless it moves it to another
    /// category: there it goes last.
    pub fn update_snippet_unnotified(&mut self, id: SnippetId, draft: SnippetDraft) -> bool {
        let draft = self.normalized_snippet(draft);
        let Some(old) = self.snippet(id) else {
            return false;
        };
        let order = if old.category == draft.category {
            old.sort_order
        } else {
            self.next_snippet_order(draft.category)
        };
        let Some(snippet) = self.snippets.iter_mut().find(|snippet| snippet.id == id) else {
            return false;
        };
        let sync_id = snippet.sync_id.clone();
        *snippet = Snippet::new(id, draft).with_sort_order(order);
        snippet.sync_id = sync_id;
        true
    }

    pub fn remove_snippet(&mut self, id: SnippetId, cx: &mut Context<Self>) -> bool {
        if !self.remove_snippet_unnotified(id) {
            return false;
        }
        self.persist(Change::DeleteSnippet, cx, |db| db.remove_snippet(id));
        cx.notify();
        true
    }

    pub fn remove_snippet_unnotified(&mut self, id: SnippetId) -> bool {
        let before = self.snippets.len();
        self.snippets.retain(|snippet| snippet.id != id);
        self.snippets.len() != before
    }

    pub fn insert_snippet_category(
        &mut self,
        name: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> SnippetCategoryId {
        let id = self.insert_snippet_category_unnotified(name);
        if let Some(category) = self.snippet_category(id) {
            self.persist(Change::NewSnippetCategory, cx, |db| {
                db.insert_snippet_category(category)
            });
        }
        cx.notify();
        id
    }

    pub fn insert_snippet_category_unnotified(
        &mut self,
        name: impl Into<SharedString>,
    ) -> SnippetCategoryId {
        let id = SnippetCategoryId(self.next_snippet_category_id);
        self.next_snippet_category_id += 1;
        self.snippet_categories.push(SnippetCategory::new(id, name));
        id
    }

    pub fn rename_snippet_category(
        &mut self,
        id: SnippetCategoryId,
        name: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.rename_snippet_category_unnotified(id, name) {
            return false;
        }
        if let Some(category) = self.snippet_category(id) {
            self.persist(Change::RenameSnippetCategory, cx, |db| {
                db.update_snippet_category(category)
            });
        }
        cx.notify();
        true
    }

    pub fn rename_snippet_category_unnotified(
        &mut self,
        id: SnippetCategoryId,
        name: impl Into<SharedString>,
    ) -> bool {
        let Some(category) = self
            .snippet_categories
            .iter_mut()
            .find(|category| category.id == id)
        else {
            return false;
        };
        category.name = name.into();
        true
    }

    /// Delete a category with the snippets in it.
    pub fn remove_snippet_category(
        &mut self,
        id: SnippetCategoryId,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.remove_snippet_category_unnotified(id) {
            return false;
        }
        // The database takes the snippets through `ON DELETE CASCADE`.
        self.persist(Change::DeleteSnippetCategory, cx, |db| {
            db.remove_snippet_category(id)
        });
        cx.notify();
        true
    }

    pub fn remove_snippet_category_unnotified(&mut self, id: SnippetCategoryId) -> bool {
        let before = self.snippet_categories.len();
        self.snippet_categories.retain(|category| category.id != id);
        if self.snippet_categories.len() == before {
            return false;
        }
        self.snippets.retain(|snippet| snippet.category != Some(id));
        true
    }

    /// A draft that names a category, a group or a host no longer there
    /// is saved without it: 未分类, for every host.
    fn normalized_snippet(&self, mut draft: SnippetDraft) -> SnippetDraft {
        if draft
            .category
            .is_some_and(|category| self.snippet_category(category).is_none())
        {
            draft.category = None;
        }
        let gone = match draft.scope {
            SnippetScope::All => false,
            SnippetScope::Group(group) => self.group(group).is_none(),
            SnippetScope::Host(host) => self.saved_host(host).is_none(),
        };
        if gone {
            draft.scope = SnippetScope::All;
        }
        draft
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{AuthKind, GroupDraft, Host, HostDatabase, HostDraft, HostId};

    fn draft(name: &str, category: Option<SnippetCategoryId>) -> SnippetDraft {
        SnippetDraft::new(name, format!("echo {name}"), category)
    }

    fn names(store: &HostStore) -> Vec<String> {
        store
            .snippets()
            .iter()
            .map(|snippet| snippet.name.to_string())
            .collect()
    }

    #[test]
    fn a_category_takes_its_snippets_with_it() {
        let mut store = HostStore::empty();
        let docker = store.insert_snippet_category_unnotified("Docker");
        let logs = store.insert_snippet_category_unnotified("日志");
        store.insert_snippet_unnotified(draft("ps", Some(docker)));
        store.insert_snippet_unnotified(draft("tail", Some(logs)));
        store.insert_snippet_unnotified(draft("df", None));
        assert_eq!(store.snippets_in(docker), 1);

        assert!(store.remove_snippet_category_unnotified(docker));
        assert_eq!(names(&store), ["tail", "df"]);
        assert!(!store.remove_snippet_category_unnotified(docker));
        // A draft naming it now goes to 未分类.
        let moved = store.insert_snippet_unnotified(draft("images", Some(docker)));
        assert_eq!(store.snippet(moved).unwrap().category, None);
    }

    #[test]
    fn a_snippet_is_edited_in_place_and_removed() {
        let mut store = HostStore::empty();
        let ps = store.insert_snippet_unnotified(draft("ps", None));
        let df = store.insert_snippet_unnotified(draft("df", None));
        assert!(store.update_snippet_unnotified(ps, SnippetDraft::new("容器", "docker ps", None)));
        assert_eq!(store.snippet(ps).unwrap().command, "docker ps");
        assert_eq!(names(&store), ["容器", "df"]);
        assert!(store.remove_snippet_unnotified(df));
        assert!(!store.update_snippet_unnotified(df, draft("df", None)));
        assert_eq!(names(&store), ["容器"]);
    }

    #[test]
    fn a_snippet_kept_to_a_group_or_a_host_goes_with_it() {
        let mut store = HostStore::empty();
        let production = store.insert_group_unnotified(GroupDraft::new("生产", None));
        let web = store.insert_group_unnotified(GroupDraft::new("web", Some(production)));
        let host = store.insert_unnotified(HostDraft::new(
            "db-01",
            "10.0.0.2",
            22,
            "root",
            AuthKind::Password,
            None,
        ));
        let scoped = |name: &str, scope| SnippetDraft {
            scope,
            ..draft(name, None)
        };
        store.insert_snippet_unnotified(scoped("all", SnippetScope::All));
        store.insert_snippet_unnotified(scoped("web", SnippetScope::Group(web)));
        store.insert_snippet_unnotified(scoped("db", SnippetScope::Host(host)));

        store.remove_unnotified(host);
        assert_eq!(names(&store), ["all", "web"]);
        // A subgroup goes with its group, and its snippets with it.
        store.remove_group_unnotified(production);
        assert_eq!(names(&store), ["all"]);
        // Kept to what is not there, a snippet is for every host.
        let stray = store.insert_snippet_unnotified(scoped("stray", SnippetScope::Host(host)));
        assert_eq!(store.snippet(stray).unwrap().scope, SnippetScope::All);
    }

    /// The snippets of `category` in the order the list shows them.
    fn listed(store: &HostStore, category: Option<SnippetCategoryId>) -> Vec<String> {
        let mut snippets: Vec<&Snippet> = store
            .snippets()
            .iter()
            .filter(|snippet| snippet.category == category)
            .collect();
        snippets.sort_by(|a, b| Snippet::in_order(a, b));
        snippets
            .iter()
            .map(|snippet| snippet.name.to_string())
            .collect()
    }

    #[test]
    fn new_snippets_go_last_and_a_drag_puts_one_anywhere() {
        let mut store = HostStore::empty();
        let docker = store.insert_snippet_category_unnotified("Docker");
        let ps = store.insert_snippet_unnotified(draft("ps", Some(docker)));
        let images = store.insert_snippet_unnotified(draft("images", Some(docker)));
        let logs = store.insert_snippet_unnotified(draft("logs", Some(docker)));
        let df = store.insert_snippet_unnotified(draft("df", None));
        // In the order they were made, not by name.
        assert_eq!(listed(&store, Some(docker)), ["ps", "images", "logs"]);

        store.move_snippet_unnotified(logs, SnippetDrop::Before(ps));
        assert_eq!(listed(&store, Some(docker)), ["logs", "ps", "images"]);
        store.move_snippet_unnotified(logs, SnippetDrop::After(images));
        assert_eq!(listed(&store, Some(docker)), ["ps", "images", "logs"]);

        // Into another category, beside one there or last in it.
        let changed = store.move_snippet_unnotified(images, SnippetDrop::Before(df));
        assert_eq!(listed(&store, None), ["images", "df"]);
        assert_eq!(listed(&store, Some(docker)), ["ps", "logs"]);
        assert!(changed.contains(&images) && changed.contains(&df));
        store.move_snippet_unnotified(df, SnippetDrop::Into(Some(docker)));
        assert_eq!(listed(&store, Some(docker)), ["ps", "logs", "df"]);

        // Onto itself, or beside one that is gone, nothing moves.
        assert!(
            store
                .move_snippet_unnotified(ps, SnippetDrop::Before(ps))
                .is_empty()
        );
        assert!(
            store
                .move_snippet_unnotified(ps, SnippetDrop::After(SnippetId(99)))
                .is_empty()
        );
    }

    #[test]
    fn an_edit_keeps_the_place_unless_it_changes_the_category() {
        let mut store = HostStore::empty();
        let docker = store.insert_snippet_category_unnotified("Docker");
        let ps = store.insert_snippet_unnotified(draft("ps", Some(docker)));
        store.insert_snippet_unnotified(draft("images", Some(docker)));
        let df = store.insert_snippet_unnotified(draft("df", None));
        store.insert_snippet_unnotified(draft("uptime", None));

        store.update_snippet_unnotified(ps, draft("容器", Some(docker)));
        assert_eq!(listed(&store, Some(docker)), ["容器", "images"]);
        store.update_snippet_unnotified(df, draft("df", Some(docker)));
        assert_eq!(listed(&store, Some(docker)), ["容器", "images", "df"]);
    }

    #[test]
    fn a_drag_is_written_through_and_comes_back() {
        let database = HostDatabase::in_memory().unwrap();
        let mut store = HostStore::load(database).unwrap();
        let ps = store.insert_snippet_unnotified(draft("ps", None));
        let df = store.insert_snippet_unnotified(draft("df", None));
        for snippet in store.snippets() {
            store
                .database
                .as_ref()
                .unwrap()
                .insert_snippet(snippet)
                .unwrap();
        }
        for moved in store.move_snippet_unnotified(df, SnippetDrop::Before(ps)) {
            let snippet = store.snippet(moved).unwrap();
            store
                .database
                .as_ref()
                .unwrap()
                .update_snippet(snippet)
                .unwrap();
        }
        let data = store.database.as_ref().unwrap().load().unwrap();
        let mut back: Vec<&Snippet> = data.snippets.iter().collect();
        back.sort_by(|a, b| Snippet::in_order(a, b));
        let back: Vec<&str> = back.iter().map(|snippet| snippet.name.as_ref()).collect();
        assert_eq!(back, ["df", "ps"]);
    }

    #[test]
    fn load_resumes_the_snippet_id_sequences() {
        let database = HostDatabase::in_memory().unwrap();
        database
            .insert_host(&Host::new(
                HostId(1),
                HostDraft::new("web", "10.0.0.1", 22, "root", AuthKind::Password, None),
            ))
            .unwrap();
        database
            .insert_snippet_category(&SnippetCategory::new(SnippetCategoryId(3), "Docker"))
            .unwrap();
        database
            .insert_snippet(&Snippet::new(
                SnippetId(7),
                draft("ps", Some(SnippetCategoryId(3))),
            ))
            .unwrap();

        let mut store = HostStore::load(database).unwrap();
        assert_eq!(names(&store), ["ps"]);
        assert_eq!(
            store.insert_snippet_category_unnotified("日志"),
            SnippetCategoryId(4)
        );
        assert_eq!(
            store.insert_snippet_unnotified(draft("df", None)),
            SnippetId(8)
        );
    }
}
