//! What one sync round does with what changed here and what changed on the
//! server: pure, so every case is a unit test.
//!
//! Each item is compared three ways, the way a three-way merge does: what
//! this device holds now, what it held when it last agreed with the server
//! (its fingerprint in `base`) and what the server holds now.
//!
//! - Changed on one side only: that side's version goes to the other.
//! - Changed on both: this device's wins. It changed more recently, since a
//!   change is synced seconds after it is made.
//! - New here and already on the server, the same item by its natural key
//!   (both devices imported the same Xshell sessions, say): it takes the
//!   server's id and the server's version, rather than becoming two.
//! - Known hosts are merged line by line, so no device's trust is lost.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::record::{KNOWN_HOSTS_ID, Payload};

/// The fingerprint of an item that is not there: deleted, or never made.
pub const GONE: &str = "-";

/// What this device knew of an item when it last agreed with the server.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Base {
    pub revision: i64,
    /// The keyed hash of the content agreed on, [`GONE`] for a deletion,
    /// empty when this device's version must go up whatever it is.
    pub fingerprint: String,
}

/// An item as the server has it now. `None` is a deletion.
#[derive(Clone, Debug, PartialEq)]
pub struct Remote {
    pub revision: i64,
    pub payload: Option<Payload>,
}

/// An item this device sends.
#[derive(Clone, Debug, PartialEq)]
pub struct Push {
    pub id: String,
    pub base_revision: i64,
    pub payload: Option<Payload>,
}

/// What a round does.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// New ids for items made here that the server already has: `(here,
    /// server's)`.
    pub rekey: Vec<(String, String)>,
    /// Items to change here: `None` deletes.
    pub apply: Vec<(String, Option<Payload>)>,
    pub push: Vec<Push>,
}

/// The round's inputs. `local` holds every item this device has; the ids in
/// `unreadable` are there but could not be read (a keychain that refused, a
/// key file gone), so they are taken as unchanged.
pub struct Merge<'a> {
    pub local: &'a mut HashMap<String, Payload>,
    pub unreadable: &'a HashSet<String>,
    pub base: &'a mut HashMap<String, Base>,
    pub fingerprint: &'a dyn Fn(&Payload) -> String,
}

impl Merge<'_> {
    /// Decide, for what the server has that changed since this device last
    /// looked, and for everything changed here. `local` and `base` come out
    /// as they will be once the plan is carried out, but for the pushes:
    /// their bases follow what the server answers.
    pub fn plan(&mut self, remote: &BTreeMap<String, Remote>) -> Plan {
        let mut plan = Plan {
            rekey: self.match_new_items(remote),
            ..Plan::default()
        };
        let mut ids: Vec<String> = self
            .local
            .keys()
            .chain(remote.keys())
            .chain(self.base.keys())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        ids.sort();
        for id in ids {
            if self.unreadable.contains(&id) {
                continue;
            }
            match remote.get(&id) {
                Some(theirs) => self.meet(&id, theirs, &mut plan),
                None => self.send_if_changed(&id, &mut plan),
            }
        }
        plan
    }

    fn print(&self, payload: Option<&Payload>) -> String {
        payload.map_or_else(|| GONE.to_owned(), |payload| (self.fingerprint)(payload))
    }

    fn changed_here(&self, id: &str) -> bool {
        let ours = self.print(self.local.get(id));
        match self.base.get(id) {
            Some(base) => base.fingerprint != ours,
            None => ours != GONE,
        }
    }

    /// An item the server has news of.
    fn meet(&mut self, id: &str, theirs: &Remote, plan: &mut Plan) {
        let ours = self.print(self.local.get(id));
        let their_print = self.print(theirs.payload.as_ref());
        if ours == their_print {
            self.agree(id, theirs.revision, their_print);
        } else if id == KNOWN_HOSTS_ID {
            self.merge_known_hosts(theirs, plan);
        } else if !self.changed_here(id) || !self.base.contains_key(id) {
            // Never agreed on before, yet the server has it under this id:
            // the settings, or an item matched by its natural key. The
            // server's is the one every other device has.
            self.take(id, theirs, plan);
        } else {
            plan.push.push(Push {
                id: id.to_owned(),
                base_revision: theirs.revision,
                payload: self.local.get(id).cloned(),
            });
        }
    }

    fn send_if_changed(&mut self, id: &str, plan: &mut Plan) {
        if !self.changed_here(id) {
            return;
        }
        plan.push.push(Push {
            id: id.to_owned(),
            base_revision: self.base.get(id).map_or(0, |base| base.revision),
            payload: self.local.get(id).cloned(),
        });
    }

    fn agree(&mut self, id: &str, revision: i64, fingerprint: String) {
        self.base.insert(
            id.to_owned(),
            Base {
                revision,
                fingerprint,
            },
        );
    }

    fn take(&mut self, id: &str, theirs: &Remote, plan: &mut Plan) {
        match &theirs.payload {
            Some(payload) => self.local.insert(id.to_owned(), payload.clone()),
            None => self.local.remove(id),
        };
        plan.apply.push((id.to_owned(), theirs.payload.clone()));
        let print = self.print(theirs.payload.as_ref());
        self.agree(id, theirs.revision, print);
    }

    /// Both sides' lines: taken here when the server had any this device
    /// did not, sent when this device had any the server did not.
    fn merge_known_hosts(&mut self, theirs: &Remote, plan: &mut Plan) {
        let lines = |payload: Option<&Payload>| match payload {
            Some(Payload::KnownHosts { lines }) => lines.clone(),
            _ => Vec::new(),
        };
        let mut merged = lines(self.local.get(KNOWN_HOSTS_ID));
        merged.extend(lines(theirs.payload.as_ref()));
        merged.sort();
        merged.dedup();
        let merged = Payload::KnownHosts { lines: merged };
        if Some(&merged) != self.local.get(KNOWN_HOSTS_ID) {
            self.local.insert(KNOWN_HOSTS_ID.to_owned(), merged.clone());
            plan.apply
                .push((KNOWN_HOSTS_ID.to_owned(), Some(merged.clone())));
        }
        if Some(&merged) == theirs.payload.as_ref() {
            let print = self.print(Some(&merged));
            self.agree(KNOWN_HOSTS_ID, theirs.revision, print);
        } else {
            plan.push.push(Push {
                id: KNOWN_HOSTS_ID.to_owned(),
                base_revision: theirs.revision,
                payload: Some(merged),
            });
        }
    }

    /// Items made here that the server already has under another id, by
    /// natural key, one for one. They take the server's id, and so do the
    /// references to them.
    fn match_new_items(&mut self, remote: &BTreeMap<String, Remote>) -> Vec<(String, String)> {
        let unmatched_remote: Vec<(&String, &Payload)> = remote
            .iter()
            .filter(|(id, _)| !self.local.contains_key(*id) && !self.base.contains_key(*id))
            .filter_map(|(id, theirs)| theirs.payload.as_ref().map(|payload| (id, payload)))
            .collect();
        if unmatched_remote.is_empty() {
            return Vec::new();
        }
        let renamed = self.find_matches(&unmatched_remote, remote);
        if renamed.is_empty() {
            return Vec::new();
        }
        let rename: HashMap<String, String> = renamed.iter().cloned().collect();
        let local = std::mem::take(self.local);
        *self.local = local
            .into_iter()
            .map(|(id, mut payload)| {
                payload.rename_references(&rename);
                (rename.get(&id).cloned().unwrap_or(id), payload)
            })
            .collect();
        renamed
    }

    fn find_matches(
        &self,
        unmatched_remote: &[(&String, &Payload)],
        remote: &BTreeMap<String, Remote>,
    ) -> Vec<(String, String)> {
        let local = &*self.local;
        let lookup = |id: &str| {
            local
                .get(id)
                .or_else(|| remote.get(id).and_then(|theirs| theirs.payload.as_ref()))
        };
        let mut by_key: HashMap<String, &String> = HashMap::new();
        for (id, payload) in unmatched_remote {
            if let Some(key) = payload.natural_key(&lookup) {
                by_key.entry(key).or_insert(id);
            }
        }
        let mut new_here: Vec<(&String, &Payload)> = local
            .iter()
            .filter(|(id, _)| !self.base.contains_key(*id) && !remote.contains_key(*id))
            .collect();
        new_here.sort_by_key(|(id, _)| *id);
        let mut renamed = Vec::new();
        for (id, payload) in new_here {
            if let Some(theirs) = payload
                .natural_key(&lookup)
                .and_then(|key| by_key.remove(&key))
            {
                renamed.push((id.clone(), theirs.clone()));
            }
        }
        renamed
    }
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
