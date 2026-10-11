//! What 撰写 sent before, for ↑ and ↓ to bring back: kept in
//! `compose_history.json` in the data directory, oldest first, as a shell
//! keeps its history. In plain text, like `~/.bash_history`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// How many it keeps; the oldest go first.
const LIMIT: usize = 200;

/// The texts sent, and where ↑ and ↓ have got to among them.
#[derive(Debug, Default)]
pub(super) struct ComposeHistory {
    entries: Vec<String>,
    /// Where it is kept; none in UI tests, which keep it in memory.
    path: Option<PathBuf>,
    /// The entry on show, while going through them.
    recalled: Option<usize>,
    /// What was in the box before ↑ first brought an entry back.
    draft: String,
}

impl ComposeHistory {
    /// The history kept at `path`. One not there yet, or not readable as a
    /// history, starts empty and is written over on the next send.
    pub(super) fn load(path: PathBuf) -> Self {
        let entries = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            entries,
            path: Some(path),
            ..Self::default()
        }
    }

    /// Add what was just sent, unless it is the last one again, and write
    /// the history out. Going through it starts over.
    pub(super) fn push(&mut self, text: &str) -> io::Result<()> {
        self.recalled = None;
        self.draft.clear();
        if self.entries.last().is_some_and(|last| last == text) {
            return Ok(());
        }
        self.entries.push(text.to_owned());
        let excess = self.entries.len().saturating_sub(LIMIT);
        self.entries.drain(..excess);
        match &self.path {
            Some(path) => save(path, &self.entries),
            None => Ok(()),
        }
    }

    /// ↑: the entry before the one on show, or the last one when none is,
    /// keeping `current` to come back to. `None` when there is nothing
    /// older.
    pub(super) fn older(&mut self, current: &str) -> Option<&str> {
        let at = match self.recalled {
            Some(0) => return None,
            Some(at) => at - 1,
            None => {
                let last = self.entries.len().checked_sub(1)?;
                self.draft = current.to_owned();
                last
            }
        };
        self.recalled = Some(at);
        Some(&self.entries[at])
    }

    /// ↓: the entry after the one on show, and after the newest what was
    /// being written. `None` when no entry is on show.
    pub(super) fn newer(&mut self) -> Option<&str> {
        let at = self.recalled?;
        if at + 1 < self.entries.len() {
            self.recalled = Some(at + 1);
            return Some(&self.entries[at + 1]);
        }
        self.recalled = None;
        Some(&self.draft)
    }
}

fn save(path: &Path, entries: &[String]) -> io::Result<()> {
    let text = serde_json::to_string_pretty(entries).map_err(io::Error::other)?;
    fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(entries: &[&str]) -> ComposeHistory {
        ComposeHistory {
            entries: entries.iter().map(ToString::to_string).collect(),
            ..ComposeHistory::default()
        }
    }

    #[test]
    fn up_goes_back_and_down_comes_back_to_what_was_being_written() {
        let mut history = history(&["uptime", "df -h", "ls"]);
        assert_eq!(history.older("pw"), Some("ls"));
        assert_eq!(history.older("ls"), Some("df -h"));
        assert_eq!(history.older("df -h"), Some("uptime"));
        // Nothing older: it stays where it is.
        assert_eq!(history.older("uptime"), None);
        assert_eq!(history.newer(), Some("df -h"));
        assert_eq!(history.newer(), Some("ls"));
        assert_eq!(history.newer(), Some("pw"));
        // Back in the box: ↓ has nothing to do.
        assert_eq!(history.newer(), None);
    }

    #[test]
    fn an_empty_history_has_nothing_to_bring_back() {
        let mut history = history(&[]);
        assert_eq!(history.older("pw"), None);
        assert_eq!(history.newer(), None);
    }

    #[test]
    fn a_send_is_kept_once_and_the_oldest_go_past_the_limit() {
        let mut history = history(&[]);
        history.push("uptime").unwrap();
        history.push("uptime").unwrap();
        history.push("df -h").unwrap();
        assert_eq!(history.entries, ["uptime", "df -h"]);
        // Sending starts going through it over again.
        history.older("");
        history.push("ls").unwrap();
        assert_eq!(history.older(""), Some("ls"));

        for n in 0..LIMIT {
            history.push(&n.to_string()).unwrap();
        }
        assert_eq!(history.entries.len(), LIMIT);
        assert_eq!(history.entries[0], "0");
    }

    #[test]
    fn the_history_is_kept_on_disk_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("compose_history.json");
        // Not there yet, then not a history: both start empty.
        assert!(ComposeHistory::load(path.clone()).entries.is_empty());
        fs::write(&path, "not json").unwrap();
        let mut history = ComposeHistory::load(path.clone());
        assert!(history.entries.is_empty());

        history.push("uptime").unwrap();
        history.push("cd /tmp\nls").unwrap();

        let mut again = ComposeHistory::load(path);
        assert_eq!(again.older(""), Some("cd /tmp\nls"));
        assert_eq!(again.older(""), Some("uptime"));
    }

    #[test]
    fn a_history_that_cannot_be_written_says_why() {
        let dir = tempfile::tempdir().unwrap();
        // A folder where the file should be.
        let path = dir.path().join("compose_history.json");
        fs::create_dir(&path).unwrap();
        let mut history = ComposeHistory::load(path);
        assert!(history.push("uptime").is_err());
        // It is still there to bring back this time.
        assert_eq!(history.older(""), Some("uptime"));
    }
}
