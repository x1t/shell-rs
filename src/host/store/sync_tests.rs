use gpui_kit::{AppContext as _, TestAppContext};

use super::*;
use crate::host::{AuthKind, CredentialKind, ForwardEndpoint, ForwardKind, HostDatabase};

/// A store over a database file, as production has, and the file.
fn store_on_disk(cx: &mut TestAppContext) -> (gpui_kit::Entity<HostStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let database = HostDatabase::open(&dir.path().join("shellrs.db")).unwrap();
    let store = HostStore::load(database).unwrap();
    (cx.new(|_| store), dir)
}

fn reopened(dir: &tempfile::TempDir) -> HostStore {
    HostStore::load(HostDatabase::open(&dir.path().join("shellrs.db")).unwrap()).unwrap()
}

#[gpui_kit::test]
fn items_from_another_device_keep_its_ids_and_places(cx: &mut TestAppContext) {
    let (store, dir) = store_on_disk(cx);
    store.update(cx, |store, cx| {
        let group = store.upsert_synced_group("G1", GroupDraft::new("生产", None), 5, cx);
        let credential = store.upsert_synced_credential(
            "C1",
            CredentialDraft::new("ops", CredentialKind::Agent, "ops"),
            3,
            cx,
        );
        let mut draft = HostDraft::new("web", "10.0.0.1", 22, "", AuthKind::Password, Some(group));
        draft.credential = Some(credential);
        let host = store.upsert_synced_host(
            "H1",
            draft,
            7,
            [vec![], vec!["/var/log".into(), "/etc".into()]],
            cx,
        );
        store.upsert_synced_forward(
            "F1",
            ForwardDraft::new(
                ForwardKind::Dynamic,
                host,
                ForwardEndpoint::new("127.0.0.1", 1080),
                None,
            ),
            2,
            cx,
        );
        let category = store.upsert_synced_snippet_category("K1", "Docker".into(), cx);
        store.upsert_synced_snippet(
            "S1",
            SnippetDraft::new("ps", "docker ps", Some(category)),
            4,
            cx,
        );
    });

    let loaded = reopened(&dir);
    let host = &loaded.hosts()[0];
    assert_eq!(host.sync_id.as_str(), "H1");
    assert_eq!(host.sort_order, 7);
    assert_eq!(host.user, "ops", "the credential's user");
    assert_eq!(loaded.groups()[0].sync_id.as_str(), "G1");
    assert_eq!(loaded.groups()[0].sort_order, 5);
    assert_eq!(loaded.credentials()[0].sync_id.as_str(), "C1");
    assert_eq!(loaded.credentials()[0].sort_order, 3);
    assert_eq!(loaded.forwards()[0].sync_id.as_str(), "F1");
    assert_eq!(loaded.snippet_categories()[0].sync_id.as_str(), "K1");
    assert_eq!(loaded.snippets()[0].sync_id.as_str(), "S1");
    assert_eq!(loaded.snippets()[0].sort_order, 4);
    assert_eq!(
        loaded.bookmarks(host.id, BookmarkSide::Remote),
        ["/var/log", "/etc"]
    );
}

#[gpui_kit::test]
fn an_item_synced_again_is_updated_in_place(cx: &mut TestAppContext) {
    let (store, dir) = store_on_disk(cx);
    store.update(cx, |store, cx| {
        let draft = HostDraft::new("web", "10.0.0.1", 22, "root", AuthKind::Password, None);
        store.upsert_synced_host("H1", draft, 0, [vec![], vec!["/a".into()]], cx);
        let draft = HostDraft::new("web-01", "10.0.0.1", 22, "root", AuthKind::Password, None);
        store.upsert_synced_host("H1", draft, 3, [vec!["/b".into()], vec![]], cx);
        store.upsert_synced_snippet_category("K1", "Docker".into(), cx);
        store.upsert_synced_snippet_category("K1", "容器".into(), cx);
    });

    let loaded = reopened(&dir);
    assert_eq!(loaded.hosts().len(), 1);
    let host = &loaded.hosts()[0];
    assert_eq!(host.name, "web-01");
    assert_eq!(host.sort_order, 3);
    assert_eq!(loaded.bookmarks(host.id, BookmarkSide::Local), ["/b"]);
    assert!(loaded.bookmarks(host.id, BookmarkSide::Remote).is_empty());
    assert_eq!(loaded.snippet_categories()[0].name, "容器");
}

#[gpui_kit::test]
fn an_item_can_take_the_id_another_device_gave_it(cx: &mut TestAppContext) {
    let (store, dir) = store_on_disk(cx);
    let (old, renamed) = store.update(cx, |store, cx| {
        let id = store.insert_snippet_category("Docker", cx);
        let old = store.snippet_category(id).unwrap().sync_id.to_string();
        let renamed = store.rename_sync_id(&old, "THEIRS", cx);
        (old, renamed)
    });
    assert!(renamed);
    store.update(cx, |store, cx| {
        assert!(
            !store.rename_sync_id(&old, "AGAIN", cx),
            "the old id is gone"
        );
        assert!(store.snippet_category_by_sync_id("THEIRS").is_some());
    });
    assert_eq!(
        reopened(&dir).snippet_categories()[0].sync_id.as_str(),
        "THEIRS"
    );
}
