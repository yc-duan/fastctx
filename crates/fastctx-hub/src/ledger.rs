//! The projects' shared ledgers, as the hub keeps them (`world-network.md`,
//! 共享账本).
//!
//! ```text
//! <hub>/ledger/<town id>/<seq>.json   one entry each, as its author signed it
//! ```
//!
//! The hub takes an entry from the member that signed it, acting for an
//! account that may read and write the project, only when no path it writes
//! has been written in the project before, nor is a directory another
//! entry's paths are in, nor is in a directory that is a path written: so
//! nothing written is ever overwritten, and of two machines writing the same
//! version, the one that comes first wins. The first entry of a project says
//! which machines it spans, each one its account may work on, and any later
//! one may change that; the hub tells a project's machines of every new
//! entry, in the order it took them.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use fastctx_net::directory::Directory;
use fastctx_net::ledger::Entry;
use fastctx_net::sign::Signed;
use fastctx_net::wire::{self, Down, Told};
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::MachineName;
use fastctx_types::access::TownId;

use crate::store::Store;

/// What the hub knows of one project's ledger.
#[derive(Default)]
pub struct TownLedger {
    /// The number the next entry gets.
    pub next: u64,
    pub machines: Vec<MachineName>,
    /// Every path written, in lower case, as a system that ignores case
    /// would compare them.
    paths: BTreeSet<String>,
}

#[derive(Default)]
pub struct Ledgers {
    dir: PathBuf,
    pub towns: BTreeMap<TownId, TownLedger>,
}

impl Ledgers {
    /// Reads every project's ledger under `hub`.
    pub fn open(hub: &Path) -> io::Result<Self> {
        let dir = hub.join("ledger");
        let mut towns = BTreeMap::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self { dir, towns }),
            Err(error) => return Err(error),
        };
        for town in entries {
            let path = town?.path();
            let Some(id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| TownId::try_from(name.to_string()).ok())
            else {
                continue;
            };
            let mut ledger = TownLedger { next: 1, ..TownLedger::default() };
            for (seq, file) in numbered(&path)? {
                let signed: Signed = read(&file)?;
                let entry: Entry = signed.claim().map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", file.display()))
                })?;
                ledger.add(&entry);
                ledger.next = seq + 1;
            }
            towns.insert(id, ledger);
        }
        Ok(Self { dir, towns })
    }

    /// Takes an entry of `town` from `from`; returns its number and the
    /// members to tell.
    pub fn write(
        &mut self,
        store: &Store,
        from: &MachineName,
        town: &TownId,
        signed: Signed,
    ) -> Result<(u64, Vec<MachineName>), String> {
        let directory = &store.book.directory;
        let member = directory.member(from).ok_or_else(|| format!("{from} is not a member"))?;
        let project = directory.project(town).ok_or_else(|| format!("there is no project {town} on this hub"))?;
        if !directory.member_in_project(from, town) {
            return Err(format!("{from} acts for no account that may write in the project {}", project.name));
        }
        let entry = fastctx_net::ledger::open(&signed, &store.hub.id, town)?;
        // An entry made before its author was renamed is its all the same.
        if !directory.same(&entry.author, from) || entry.key != member.key {
            return Err(format!("the entry is by {}, sent by {from}", entry.author));
        }
        let ledger = self.towns.get(town);
        if ledger.is_none() && entry.machines.is_none() {
            return Err("the project has no ledger yet; its first entry says which machines it spans".to_string());
        }
        if let Some(machines) = &entry.machines {
            if machines.is_empty() {
                return Err("a project spans at least one machine".to_string());
            }
            if let Some(stranger) = machines.iter().find(|machine| !directory.rights(&project.owner, machine).hands) {
                return Err(format!("the account {} may not work on {stranger}", project.owner));
            }
        }
        let mut seen = BTreeSet::new();
        for file in &entry.files {
            let path = file.path.to_lowercase();
            if !seen.insert(path.clone()) {
                return Err(format!("the entry writes {} twice", file.path));
            }
            if seen.iter().any(|other| *other != path && fastctx_net::ledger::conflicts(other, &path)) {
                return Err(format!("the entry writes {} and a path inside it", file.path));
            }
            if let Some(ledger) = ledger
                && ledger.conflicts(&path)
            {
                return Err(format!(
                    "the project has {} already, or a file where it is a directory; nothing in a ledger is written twice",
                    file.path
                ));
            }
        }
        // It must fit in the answer that hands it to a machine later.
        let size = fastctx_net::ledger::handed_size(&signed);
        if size > wire::MAX_CONTROL_FRAME {
            return Err(format!(
                "the entry is {size} bytes, more than one message holds ({})",
                wire::MAX_CONTROL_FRAME
            ));
        }
        let ledger = self.towns.entry(town.clone()).or_insert_with(|| TownLedger { next: 1, ..TownLedger::default() });
        let seq = ledger.next;
        let folder = self.dir.join(town.as_str());
        ensure_private_dir(&folder)
            .and_then(|()| publish_new(&folder.join(format!("{seq:020}.json")), &json(&signed)?, Permissions::Private))
            .map_err(|error| format!("the hub could not keep the entry: {error}"))?;
        ledger.add(&entry);
        ledger.next = seq + 1;
        let mut tell: Vec<MachineName> =
            ledger.machines.iter().map(|machine| directory.canonical(machine).clone()).collect();
        tell.push(directory.canonical(from).clone());
        tell.sort();
        tell.dedup();
        Ok((seq, tell))
    }

    /// The projects `name` spans, with each one's latest entry: a project
    /// says the machines it spans under the names they had when it was
    /// written, which still name them.
    pub fn towns_of(&self, directory: &Directory, name: &MachineName) -> Vec<(TownId, u64)> {
        self.towns
            .iter()
            .filter(|(_, ledger)| ledger.machines.iter().any(|machine| directory.same(machine, name)))
            .map(|(town, ledger)| (town.clone(), ledger.next - 1))
            .collect()
    }

    /// Every project with a ledger, with its latest entry.
    pub fn catalog(&self) -> Vec<(TownId, u64)> {
        self.towns.iter().map(|(town, ledger)| (town.clone(), ledger.next - 1)).collect()
    }

    /// The entries of `town` after `after` that fit in one answer.
    pub fn read(&self, town: &TownId, after: u64) -> Result<(Vec<(u64, Signed)>, bool), String> {
        if !self.towns.contains_key(town) {
            return Ok((Vec::new(), false));
        }
        let files =
            numbered(&self.dir.join(town.as_str())).map_err(|error| format!("reading {town}'s ledger: {error}"))?;
        // An answer is its frame around the entries, each separated by a
        // comma; it holds entries while that stays within one frame.
        let empty =
            wire::encode(&Down::Told { id: u64::MAX, answer: Ok(Told::Entries { entries: Vec::new(), more: true }) });
        let mut size = empty.len();
        let mut entries = Vec::new();
        for (seq, file) in files.into_iter().filter(|(seq, _)| *seq > after) {
            let signed: Signed = read(&file).map_err(|error| format!("reading entry {seq} of {town}: {error}"))?;
            let cost = wire::encode(&(seq, &signed)).len() + 1;
            if !entries.is_empty() && size + cost > wire::MAX_CONTROL_FRAME {
                return Ok((entries, true));
            }
            size += cost;
            entries.push((seq, signed));
        }
        Ok((entries, false))
    }
}

impl TownLedger {
    fn add(&mut self, entry: &Entry) {
        if let Some(machines) = &entry.machines {
            self.machines = machines.clone();
        }
        self.paths.extend(entry.files.iter().map(|file| file.path.to_lowercase()));
    }

    /// Whether `path`, in lower case, cannot join the paths written: it is
    /// one of them, a directory one of them is in, or in a directory that
    /// is one of them.
    fn conflicts(&self, path: &str) -> bool {
        if self.paths.contains(path) {
            return true;
        }
        let mut ancestor = String::new();
        for name in path.split('/') {
            if !ancestor.is_empty() {
                if self.paths.contains(&ancestor) {
                    return true;
                }
                ancestor.push('/');
            }
            ancestor.push_str(name);
        }
        let inside = format!("{path}/");
        self.paths.range(inside.clone()..).next().is_some_and(|written| written.starts_with(&inside))
    }
}

/// The numbered files in a directory, in order.
fn numbered(directory: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if let Some(seq) = path.file_stem().and_then(|stem| stem.to_str()).and_then(|stem| stem.parse::<u64>().ok())
            && path.extension().is_some_and(|extension| extension == "json")
        {
            found.push((seq, path));
        }
    }
    found.sort();
    Ok(found)
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}

fn json(value: &impl serde::Serialize) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(io::Error::other)
}
