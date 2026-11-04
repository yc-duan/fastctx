//! What a Claude session's receipts say, read once in this run of the node.
//! The mod writes each receipt once and never changes it, and the receipts
//! stay while the session's books are kept, so each look at the session
//! found every one of them again and settled each anew: a write to take back
//! the doubt about it, a write to bind its turn and a read of the whole
//! inbox, however long ago it was settled. Now one settled here, and no
//! longer waiting in the inbox, is passed over, as is the handover of one
//! already cleared: a look costs what the new receipts and those still
//! waiting need. An input once delivered is never kept again
//! (`inbox::keep`), so one passed over has nothing left to settle.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fastctx_hosts::claude::exchange;

#[derive(Default)]
pub(super) struct Receipts(Mutex<HashMap<PathBuf, Known>>);

#[derive(Default)]
struct Known {
    /// Each receipt read, with the turn it names.
    turns: HashMap<String, String>,
    /// Those settled in this run.
    settled: HashSet<String>,
    /// Those whose handover was cleared in this run.
    cleared: HashSet<String>,
}

impl Receipts {
    /// The session's receipts, each with the turn it names; only those not
    /// read before are read.
    pub(super) fn listed(&self, directory: &Path) -> io::Result<Vec<(String, String)>> {
        let names = exchange::receipt_names(directory)?;
        let unread: Vec<String> = {
            let known = self.0.lock().expect("never poisoned");
            let turns = known.get(directory).map(|known| &known.turns);
            names.iter().filter(|name| turns.is_none_or(|turns| !turns.contains_key(*name))).cloned().collect()
        };
        let mut read = Vec::new();
        for name in unread {
            if let Some(turn) = exchange::receipt(directory, &name)? {
                read.push((name, turn));
            }
        }
        let mut known = self.0.lock().expect("never poisoned");
        let known = known.entry(directory.to_path_buf()).or_default();
        known.turns.extend(read);
        // One gone, as when the session's books are retired, is let go.
        let present: HashSet<&String> = names.iter().collect();
        known.turns.retain(|name, _| present.contains(name));
        known.settled.retain(|name| present.contains(name));
        known.cleared.retain(|name| present.contains(name));
        Ok(names.iter().filter_map(|name| known.turns.get(name).map(|turn| (name.clone(), turn.clone()))).collect())
    }

    pub(super) fn settled(&self, directory: &Path, name: &str) -> bool {
        self.0.lock().expect("never poisoned").get(directory).is_some_and(|known| known.settled.contains(name))
    }

    pub(super) fn settle(&self, directory: &Path, name: &str) {
        let mut known = self.0.lock().expect("never poisoned");
        known.entry(directory.to_path_buf()).or_default().settled.insert(name.to_string());
    }

    pub(super) fn cleared(&self, directory: &Path, name: &str) -> bool {
        self.0.lock().expect("never poisoned").get(directory).is_some_and(|known| known.cleared.contains(name))
    }

    pub(super) fn clear(&self, directory: &Path, names: Vec<String>) {
        let mut known = self.0.lock().expect("never poisoned");
        known.entry(directory.to_path_buf()).or_default().cleared.extend(names);
    }

    /// The session is no longer looked at.
    pub(super) fn forget(&self, directory: &Path) {
        self.0.lock().expect("never poisoned").remove(directory);
    }
}
