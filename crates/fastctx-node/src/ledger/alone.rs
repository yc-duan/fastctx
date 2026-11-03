//! A computer on no hub checks a new entry of a project's ledger as the hub
//! would, against what it keeps in memory of the ledger: every path taken,
//! the memory and history indexes, and the last entry. Each write reads only
//! the entries kept since the one before, so writing costs what the entry
//! holds, not what the ledger does (`world-foundation.md`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use fastctx_net::ledger::{self, Entry, File};
use fastctx_types::layout::WorldDir;
use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;

use super::names::Place;

/// What is kept of each project's ledger. Its writes go one at a time
/// already, under the node's `syncing`.
#[derive(Default)]
pub(crate) struct Ledgers {
    kept: Mutex<HashMap<TownName, Checked>>,
}

#[derive(Default)]
struct Checked {
    /// The ledger this was read from: one moved, or read anew, starts over.
    place: Option<Place>,
    /// The last entry taken.
    through: u64,
    /// Every path taken, by its lowercase spelling: paths that differ only
    /// in case name one file on some systems.
    paths: BTreeMap<String, String>,
    memory: fastctx_net::memory::Index,
    history: fastctx_net::topic_history::Index,
}

impl Checked {
    fn take(&mut self, seq: u64, entry: &Entry) -> Result<(), String> {
        self.memory.take(entry)?;
        self.history.take(entry)?;
        for file in &entry.files {
            self.paths.insert(file.path.to_lowercase(), file.path.clone());
        }
        self.through = seq;
        Ok(())
    }

    /// The path taken already that `path` would write over, as
    /// `ledger::conflicts` says: itself, a folder above it, or one below.
    fn taken(&self, path: &str) -> Option<&str> {
        let lower = path.to_lowercase();
        if let Some(taken) = self.paths.get(&lower) {
            return Some(taken);
        }
        let mut above = lower.as_str();
        while let Some((parent, _)) = above.rsplit_once('/') {
            if let Some(taken) = self.paths.get(parent) {
                return Some(taken);
            }
            above = parent;
        }
        let below = format!("{lower}/");
        self.paths
            .range(below.clone()..)
            .next()
            .filter(|(key, _)| key.starts_with(&below))
            .map(|(_, taken)| taken.as_str())
    }
}

/// Checks `files` as an entry of `town`'s ledger written here, numbers it,
/// and keeps it.
pub(super) fn write(
    ledgers: &Ledgers,
    world: &WorldDir,
    here: &MachineName,
    town: &TownName,
    place: &Place,
    files: Vec<File>,
    machines: Option<Vec<MachineName>>,
) -> Result<(), String> {
    let mut kept = ledgers.kept.lock().expect("never poisoned");
    let checked = kept.entry(town.clone()).or_default();
    let result = write_checked(checked, world, here, town, place, files, machines);
    if result.is_err() {
        // What was read may be part of a change; the next write reads it all.
        *checked = Checked::default();
    }
    result
}

fn write_checked(
    checked: &mut Checked,
    world: &WorldDir,
    here: &MachineName,
    town: &TownName,
    place: &Place,
    files: Vec<File>,
    machines: Option<Vec<MachineName>>,
) -> Result<(), String> {
    let reading = |error: std::io::Error| format!("reading {town}'s ledger: {error}");
    if checked.place.as_ref() != Some(place) || super::replica_seq(world, town).map_err(reading)? < checked.through {
        *checked = Checked { place: Some(place.clone()), ..Checked::default() };
    }
    for (seq, entry) in super::entries_after(world, town, checked.through).map_err(reading)? {
        checked.take(seq, &entry)?;
    }
    if checked.through == 0 && machines.is_none() {
        return Err(format!("the project {town} has no record yet; its first version says which machines it spans"));
    }
    if let Some(machines) = &machines
        && let Some(stranger) = machines.iter().find(|machine| *machine != here)
    {
        return Err(format!(
            "there is no machine named {stranger}; this computer, {here}, is in no network, so a project spans only it"
        ));
    }
    for (index, file) in files.iter().enumerate() {
        if files[..index].iter().any(|other| ledger::conflicts(&other.path, &file.path)) {
            return Err(format!("the entry names {} twice", file.path));
        }
        if let Some(taken) = checked.taken(&file.path) {
            return Err(format!("{town} has {taken} already; nothing in a ledger is written twice"));
        }
    }
    let key = super::own_key(world).map_err(|error| format!("this machine's key: {error}"))?;
    let entry = Entry {
        hub: place.hub.clone(),
        town: place.town.clone(),
        author: here.clone(),
        key: key.public(),
        at: Timestamp::now(),
        machines,
        files,
    };
    checked.memory.check(&entry.files)?;
    if let Some(revision) = fastctx_net::topic_history::decode(&entry.files)? {
        checked.history.check(&revision)?;
    }
    super::check_size(&entry)?;
    let signed = ledger::sign(&key, &entry);
    let seq = checked.through + 1;
    super::apply(world, town, seq, &entry, &signed).map_err(|error| format!("keeping the entry: {error}"))?;
    checked.take(seq, &entry)
}
