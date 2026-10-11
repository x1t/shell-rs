use super::*;
use crate::sync::record::{CategoryRecord, GroupRecord, SnippetRecord};

fn category(name: &str) -> Payload {
    Payload::SnippetCategory(CategoryRecord { name: name.into() })
}

fn snippet(name: &str, category: Option<&str>) -> Payload {
    Payload::Snippet(SnippetRecord {
        category: category.map(Into::into),
        name: name.into(),
        command: format!("echo {name}"),
        run_on_click: true,
        sort_order: 0,
    })
}

fn group(name: &str) -> Payload {
    Payload::Group(GroupRecord {
        name: name.into(),
        parent: None,
        sort_order: 0,
    })
}

fn known(lines: &[&str]) -> Payload {
    Payload::KnownHosts {
        lines: lines.iter().map(|line| line.to_string()).collect(),
    }
}

/// A fingerprint anyone can read in a test: the JSON itself.
fn print(payload: &Payload) -> String {
    String::from_utf8(payload.to_bytes().to_vec()).unwrap()
}

/// A device: what it holds and what it last agreed on with the server.
#[derive(Default)]
struct Device {
    local: HashMap<String, Payload>,
    base: HashMap<String, Base>,
    unreadable: HashSet<String>,
}

impl Device {
    fn with(items: &[(&str, Payload)]) -> Self {
        Self {
            local: items
                .iter()
                .map(|(id, payload)| (id.to_string(), payload.clone()))
                .collect(),
            ..Self::default()
        }
    }

    /// As if it had synced everything it holds at `revision`.
    fn synced(mut self, revision: i64) -> Self {
        self.base = self
            .local
            .iter()
            .map(|(id, payload)| {
                let base = Base {
                    revision,
                    fingerprint: print(payload),
                };
                (id.clone(), base)
            })
            .collect();
        self
    }

    fn plan(&mut self, remote: &[(&str, i64, Option<Payload>)]) -> Plan {
        let remote: BTreeMap<String, Remote> = remote
            .iter()
            .map(|(id, revision, payload)| {
                let remote = Remote {
                    revision: *revision,
                    payload: payload.clone(),
                };
                (id.to_string(), remote)
            })
            .collect();
        Merge {
            local: &mut self.local,
            unreadable: &self.unreadable,
            base: &mut self.base,
            fingerprint: &print,
        }
        .plan(&remote)
    }
}

fn push(id: &str, base_revision: i64, payload: Option<Payload>) -> Push {
    Push {
        id: id.into(),
        base_revision,
        payload,
    }
}

#[test]
fn nothing_changed_is_nothing_to_do() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(1);
    assert_eq!(device.plan(&[]), Plan::default());
}

#[test]
fn a_new_item_here_goes_up() {
    let mut device = Device::with(&[("a", category("Docker"))]);
    let plan = device.plan(&[]);
    assert_eq!(plan.push, [push("a", 0, Some(category("Docker")))]);
    assert!(plan.apply.is_empty());
}

#[test]
fn an_edit_here_goes_up_against_the_revision_it_was_made_on() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(4);
    device.local.insert("a".into(), category("容器"));
    assert_eq!(
        device.plan(&[]).push,
        [push("a", 4, Some(category("容器")))]
    );
}

#[test]
fn a_deletion_here_goes_up_as_a_tombstone() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(4);
    device.local.clear();
    assert_eq!(device.plan(&[]).push, [push("a", 4, None)]);
}

#[test]
fn an_item_the_server_deleted_that_this_device_never_had_is_noted() {
    let mut device = Device::default();
    let plan = device.plan(&[("a", 3, None)]);
    assert_eq!(plan, Plan::default());
    assert_eq!(device.base["a"].fingerprint, GONE);
}

#[test]
fn a_change_on_the_server_comes_down() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(1);
    let plan = device.plan(&[
        ("a", 5, Some(category("容器"))),
        ("b", 6, Some(group("生产"))),
    ]);
    assert_eq!(
        plan.apply,
        [
            ("a".to_string(), Some(category("容器"))),
            ("b".to_string(), Some(group("生产")))
        ]
    );
    assert!(plan.push.is_empty());
    assert_eq!(device.base["a"].revision, 5);
    assert_eq!(device.local["b"], group("生产"));
}

#[test]
fn a_deletion_on_the_server_comes_down() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(1);
    let plan = device.plan(&[("a", 2, None)]);
    assert_eq!(plan.apply, [("a".to_string(), None)]);
    assert!(device.local.is_empty());
    assert_eq!(device.base["a"].fingerprint, GONE);
}

#[test]
fn changed_on_both_sides_this_device_wins() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(1);
    device.local.insert("a".into(), category("本机改的"));
    let plan = device.plan(&[("a", 7, Some(category("别处改的")))]);
    assert!(plan.apply.is_empty());
    assert_eq!(plan.push, [push("a", 7, Some(category("本机改的")))]);
}

#[test]
fn deleted_here_and_edited_there_the_deletion_wins() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(1);
    device.local.clear();
    let plan = device.plan(&[("a", 7, Some(category("别处改的")))]);
    assert_eq!(plan.push, [push("a", 7, None)]);
}

#[test]
fn the_same_change_on_both_sides_is_simply_agreed() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(1);
    device.local.insert("a".into(), category("容器"));
    let plan = device.plan(&[("a", 7, Some(category("容器")))]);
    assert_eq!(plan, Plan::default());
    assert_eq!(device.base["a"].revision, 7);
}

#[test]
fn an_item_that_could_not_be_read_is_left_alone() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(1);
    device.local.clear();
    device.unreadable.insert("a".into());
    assert_eq!(device.plan(&[]), Plan::default());
}

#[test]
fn an_empty_fingerprint_sends_this_devices_version_again() {
    let mut device = Device::with(&[("a", category("Docker"))]).synced(3);
    device.base.get_mut("a").unwrap().fingerprint.clear();
    assert_eq!(
        device.plan(&[]).push,
        [push("a", 3, Some(category("Docker")))]
    );
}

#[test]
fn a_new_device_takes_the_servers_settings() {
    let defaults = Payload::Settings {
        settings: serde_json::json!({"language": "system"}),
    };
    let chosen = Payload::Settings {
        settings: serde_json::json!({"language": "en"}),
    };
    let mut device = Device::with(&[("settings", defaults)]);
    let plan = device.plan(&[("settings", 2, Some(chosen.clone()))]);
    assert_eq!(plan.apply, [("settings".to_string(), Some(chosen))]);
    assert!(plan.push.is_empty());
}

#[test]
fn items_made_on_both_devices_become_one() {
    // Both imported the same Xshell sessions: the same category and
    // snippet under ids of their own.
    let mut device = Device::with(&[
        ("mine-c", category("Docker")),
        ("mine-s", snippet("ps", Some("mine-c"))),
        ("mine-only", snippet("df", None)),
    ]);
    let plan = device.plan(&[
        ("their-c", 1, Some(category("Docker"))),
        ("their-s", 2, Some(snippet("ps", Some("their-c")))),
    ]);
    assert_eq!(
        plan.rekey,
        [
            ("mine-c".to_string(), "their-c".to_string()),
            ("mine-s".to_string(), "their-s".to_string())
        ]
    );
    assert!(plan.apply.is_empty(), "{:?}", plan.apply);
    assert_eq!(plan.push, [push("mine-only", 0, Some(snippet("df", None)))]);
    assert_eq!(device.local["their-s"], snippet("ps", Some("their-c")));
}

#[test]
fn a_matched_item_takes_the_servers_version() {
    let mut mine = snippet("ps", None);
    if let Payload::Snippet(snippet) = &mut mine {
        snippet.sort_order = 9;
    }
    let mut device = Device::with(&[("mine", mine)]);
    let plan = device.plan(&[("theirs", 1, Some(snippet("ps", None)))]);
    assert_eq!(plan.rekey, [("mine".to_string(), "theirs".to_string())]);
    assert_eq!(
        plan.apply,
        [("theirs".to_string(), Some(snippet("ps", None)))]
    );
    assert!(plan.push.is_empty());
}

#[test]
fn an_item_already_synced_is_never_matched_again() {
    let mut device = Device::with(&[("mine", category("Docker"))]).synced(1);
    let plan = device.plan(&[("theirs", 2, Some(category("Docker")))]);
    assert!(plan.rekey.is_empty());
    // Two categories of one name, as the user made them.
    assert_eq!(
        plan.apply,
        [("theirs".to_string(), Some(category("Docker")))]
    );
}

#[test]
fn known_hosts_are_merged_line_by_line() {
    let mut device = Device::with(&[("known-hosts", known(&["a key1", "b key2"]))]).synced(1);
    device
        .local
        .insert("known-hosts".into(), known(&["a key1", "b key2", "c key3"]));
    let plan = device.plan(&[("known-hosts", 4, Some(known(&["a key1", "d key4"])))]);
    let merged = known(&["a key1", "b key2", "c key3", "d key4"]);
    assert_eq!(
        plan.apply,
        [("known-hosts".to_string(), Some(merged.clone()))]
    );
    assert_eq!(plan.push, [push("known-hosts", 4, Some(merged))]);
}

#[test]
fn known_hosts_the_server_already_covers_are_only_taken() {
    let mut device = Device::with(&[("known-hosts", known(&["a key1"]))]);
    let plan = device.plan(&[("known-hosts", 4, Some(known(&["a key1", "b key2"])))]);
    assert_eq!(
        plan.apply,
        [(
            "known-hosts".to_string(),
            Some(known(&["a key1", "b key2"]))
        )]
    );
    assert!(plan.push.is_empty());
    assert_eq!(device.base["known-hosts"].revision, 4);
}
