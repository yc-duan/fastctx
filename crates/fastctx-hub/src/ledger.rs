//! The towns' shared ledgers, as the hub keeps them (`world-network.md`,
//! 共享账本).
//!
//! ```text
//! <hub>/ledger/<town>/<seq>.json   one entry each, as its author signed it
//! ```
//!
//! The hub takes an entry from its author, a member now, only when every
//! file it adds is a path the town does not have yet, so nothing written is
//! ever overwritten and of two machines writing the same version, the one
//! that comes first wins. The first entry of a town says which machines it
//! spans, and any later one may change that; the hub hands a town's entries
//! to the machines it spans, in the order it took them.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_net::ledger::{self, Entry};
use fastctx_net::sign::Signed;
use fastctx_net::trust::{Table, WorldId};
use fastctx_net::wire::{self, Down, Told};
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::{MachineName, TownName};

/// What the hub knows of one town's ledger.
#[derive(Default)]
pub struct TownLedger {
    /// The number the next entry gets.
    pub next: u64,
    pub machines: Vec<MachineName>,
    paths: Vec<String>,
}

#[derive(Default)]
pub struct Ledgers {
    dir: PathBuf,
    pub towns: BTreeMap<TownName, TownLedger>,
}

impl Ledgers {
    /// Reads every town's ledger under `hub`.
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
            let Some(name) = path.file_name().and_then(|name| TownName::try_from(name.to_str()?).ok()) else {
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
            towns.insert(name, ledger);
        }
        Ok(Self { dir, towns })
    }

    /// Takes an entry from `from`; returns its number and the machines to
    /// tell.
    pub fn write(
        &mut self,
        world: &WorldId,
        table: &Table,
        from: &MachineName,
        signed: Signed,
    ) -> Result<(u64, Vec<MachineName>), String> {
        let claimed: Entry = signed.claim().map_err(|error| format!("an unreadable entry: {error}"))?;
        if claimed.author != *from {
            return Err(format!("the entry is by {}, sent by {from}", claimed.author));
        }
        // Only a member now writes; the entry is checked as every machine
        // checks it.
        if table.get(from).is_none() {
            return Err(format!("{from} is not a member"));
        }
        let entry = ledger::open(&signed, world, table)?;
        let town = self.towns.get(&entry.town);
        if town.is_none() && entry.machines.is_none() {
            return Err(format!(
                "the town {} has no ledger yet; its first entry says which machines it spans",
                entry.town
            ));
        }
        if let Some(machines) = &entry.machines {
            if machines.is_empty() {
                return Err("a town spans at least one machine".to_string());
            }
            if let Some(stranger) = machines.iter().find(|machine| table.get(machine).is_none()) {
                return Err(format!("{stranger} is not a member of the World"));
            }
        }
        for (index, file) in entry.files.iter().enumerate() {
            if entry.files[..index].iter().any(|other| ledger::conflicts(&other.path, &file.path)) {
                return Err(format!("the entry names {} twice", file.path));
            }
            if let Some(taken) =
                town.and_then(|town| town.paths.iter().find(|path| ledger::conflicts(path, &file.path)))
            {
                return Err(format!("{} has {taken} already; nothing in a ledger is written twice", entry.town));
            }
        }
        // It must fit in the answer that hands it to a machine later.
        let size = ledger::handed_size(&signed);
        if size > wire::MAX_CONTROL_FRAME {
            return Err(format!(
                "the entry is {size} bytes, more than one message holds ({})",
                wire::MAX_CONTROL_FRAME
            ));
        }
        let ledger =
            self.towns.entry(entry.town.clone()).or_insert_with(|| TownLedger { next: 1, ..TownLedger::default() });
        let seq = ledger.next;
        let directory = self.dir.join(entry.town.as_str());
        ensure_private_dir(&directory)
            .and_then(|()| {
                publish_new(&directory.join(format!("{seq:020}.json")), &json(&signed)?, Permissions::Private)
            })
            .map_err(|error| format!("the hub could not keep the entry: {error}"))?;
        ledger.add(&entry);
        ledger.next = seq + 1;
        let mut tell = ledger.machines.clone();
        if !tell.contains(from) {
            tell.push(from.clone());
        }
        Ok((seq, tell))
    }

    /// The towns `name` spans, with each one's latest entry.
    pub fn towns_of(&self, name: &MachineName) -> Vec<(TownName, u64)> {
        self.towns
            .iter()
            .filter(|(_, town)| town.machines.contains(name))
            .map(|(town, ledger)| (town.clone(), ledger.next - 1))
            .collect()
    }

    /// The entries of `town` after `after` that fit in one answer to `name`.
    pub fn read(&self, name: &MachineName, town: &TownName, after: u64) -> Result<(Vec<(u64, Signed)>, bool), String> {
        let ledger = self.towns.get(town).ok_or_else(|| format!("the hub has no ledger for {town}"))?;
        if !ledger.machines.contains(name) {
            return Err(format!("{town} does not span {name}"));
        }
        self.read_world(town, after)
    }

    /// Reads for an authenticated machine's explicit query. The caller
    /// checks membership kind before entering this storage-only method.
    pub fn read_world(&self, town: &TownName, after: u64) -> Result<(Vec<(u64, Signed)>, bool), String> {
        if !self.towns.contains_key(town) {
            return Err(format!("the hub has no ledger for {town}"));
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
        self.paths.extend(entry.files.iter().map(|file| file.path.clone()));
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
