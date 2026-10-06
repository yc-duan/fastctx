//! The towns' shared ledgers, as the hub keeps them (`world-network.md`,
//! 共享账本).
//!
//! ```text
//! <hub>/ledger/<town id>/<seq>.json   one item each, as its signer signed it
//! ```
//!
//! The hub keeps items it cannot open (`fastctx_net::ledger::sealed`): a
//! town is a meaningless id, an entry's paths are tokens. It takes an entry
//! from its author, a member now whose scope covers the town, only when no
//! path it writes has been written in the town before, nor is a directory
//! another entry's paths are in, nor is in a directory that is a path
//! written: so nothing written is ever overwritten, and of two machines
//! writing the same version, the one that comes first wins. The first entry
//! of a town says which machines it spans, and any later one may change
//! that; the hub hands a town's items to the machines it spans, in the
//! order it took them. The World's own ledger is every member's, and its
//! one entry, the naming key, is written by a member with management
//! rights. A sealed copy of an entry's key is taken from a member that may
//! seal one for that member (`Table::may_share`), for a member that may
//! read the town.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use fastctx_net::ledger::sealed::{naming_token, open_item};
use fastctx_net::ledger::{Item, PathToken, TownId};
use fastctx_net::sign::{Bytes, Signed};
use fastctx_net::trust::{Table, WorldId};
use fastctx_net::wire::{self, Down, Told};
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::MachineName;

/// What the hub knows of one town's ledger.
#[derive(Default)]
pub struct TownLedger {
    /// The number the next item gets.
    pub next: u64,
    pub machines: Vec<MachineName>,
    paths: BTreeSet<PathToken>,
    dirs: BTreeSet<PathToken>,
    /// Each entry's random id, by its number.
    entries: BTreeMap<u64, Bytes>,
}

#[derive(Default)]
pub struct Ledgers {
    dir: PathBuf,
    pub towns: BTreeMap<TownId, TownLedger>,
    /// Directories under `ledger/` that are not towns of this version: the
    /// plaintext ledgers of the second, until a migration lets them go.
    pub older: Vec<String>,
}

impl Ledgers {
    /// Reads every town's ledger under `hub`.
    pub fn open(hub: &Path) -> io::Result<Self> {
        let dir = hub.join("ledger");
        let mut towns = BTreeMap::new();
        let mut older = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self { dir, towns, older }),
            Err(error) => return Err(error),
        };
        for town in entries {
            let path = town?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
            let Ok(id) = TownId::try_from(name.clone()) else {
                older.push(name);
                continue;
            };
            let mut ledger = TownLedger { next: 1, ..TownLedger::default() };
            for (seq, file) in numbered(&path)? {
                let signed: Signed = read(&file)?;
                let item: Item = signed.claim().map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", file.display()))
                })?;
                ledger.add(seq, &item);
                ledger.next = seq + 1;
            }
            towns.insert(id, ledger);
        }
        Ok(Self { dir, towns, older })
    }

    /// Takes an item from `from`; returns its town, its number and the
    /// members to tell.
    pub fn write(
        &mut self,
        world: &WorldId,
        table: &Table,
        from: &MachineName,
        signed: Signed,
    ) -> Result<(TownId, u64, Vec<MachineName>), String> {
        let member = table.get(from).ok_or_else(|| format!("{from} is not a member"))?;
        if !member.kind.is_machine() {
            return Err(format!("{from} is a device, and a device writes nothing in the ledger"));
        }
        let item = open_item(&signed, world, table)?;
        if item.signer() != from {
            return Err(format!("the item is by {}, sent by {from}", item.signer()));
        }
        let town = item.town().clone();
        let mut tell: Vec<MachineName> = Vec::new();
        match &item {
            Item::Entry(entry) => {
                if town == TownId::world() {
                    if !member.manage {
                        return Err("only a member with management rights writes the World's own ledger".to_string());
                    }
                    if entry.paths != [naming_token()] || !entry.dirs.is_empty() {
                        return Err("the World's own ledger holds its naming key and nothing else".to_string());
                    }
                } else if !table.may_write(from, &town) {
                    return Err(format!("{from}'s scope does not include writing in that project"));
                }
                let ledger = self.towns.get(&town);
                if ledger.is_none() && entry.machines.is_none() && town != TownId::world() {
                    return Err("the town has no ledger yet; its first entry says which machines it spans".to_string());
                }
                if let Some(machines) = &entry.machines {
                    if machines.is_empty() {
                        return Err("a town spans at least one machine".to_string());
                    }
                    if let Some(stranger) = machines.iter().find(|machine| table.get(machine).is_none()) {
                        return Err(format!("{stranger} is not a member of the World"));
                    }
                }
                let mut seen = BTreeSet::new();
                for path in &entry.paths {
                    if !seen.insert(path) {
                        return Err("the entry names one path twice".to_string());
                    }
                    if entry.dirs.contains(path) {
                        return Err("the entry writes a file where it puts a directory".to_string());
                    }
                    if ledger.is_some_and(|ledger| ledger.paths.contains(path)) {
                        return Err(
                            "the town has one of its paths already; nothing in a ledger is written twice".to_string()
                        );
                    }
                    if ledger.is_some_and(|ledger| ledger.dirs.contains(path)) {
                        return Err("the town has a directory where the entry writes a file".to_string());
                    }
                }
                if entry.dirs.iter().any(|dir| ledger.is_some_and(|ledger| ledger.paths.contains(dir))) {
                    return Err("the town has a file where the entry puts a directory".to_string());
                }
            }
            Item::Share(share) => {
                if !table.may_share(from, &share.to) {
                    return Err(format!(
                        "{from} may seal keys again only for its own branch, unless it holds management rights"
                    ));
                }
                if !table.readers(&town).any(|reader| reader.name == share.to) {
                    return Err(format!("{}'s scope does not include that project", share.to));
                }
                let known = self.towns.get(&town).and_then(|ledger| ledger.entries.get(&share.entry));
                if known != Some(&share.id) {
                    return Err(format!("the town has no entry {} with that id", share.entry));
                }
                tell.push(share.to.clone());
            }
        }
        // It must fit in the answer that hands it to a machine later.
        let size = fastctx_net::ledger::handed_size(&signed);
        if size > wire::MAX_CONTROL_FRAME {
            return Err(format!("the item is {size} bytes, more than one message holds ({})", wire::MAX_CONTROL_FRAME));
        }
        let ledger = self.towns.entry(town.clone()).or_insert_with(|| TownLedger { next: 1, ..TownLedger::default() });
        let seq = ledger.next;
        let directory = self.dir.join(town.as_str());
        ensure_private_dir(&directory)
            .and_then(|()| {
                publish_new(&directory.join(format!("{seq:020}.json")), &json(&signed)?, Permissions::Private)
            })
            .map_err(|error| format!("the hub could not keep the item: {error}"))?;
        ledger.add(seq, &item);
        ledger.next = seq + 1;
        if town == TownId::world() {
            tell.extend(table.members().map(|member| member.name.clone()));
        } else {
            tell.extend(ledger.machines.iter().cloned());
        }
        tell.push(from.clone());
        tell.sort();
        tell.dedup();
        Ok((town, seq, tell))
    }

    /// The towns `name` spans, with each one's latest item; the World's own
    /// ledger is every member's.
    pub fn towns_of(&self, name: &MachineName) -> Vec<(TownId, u64)> {
        self.towns
            .iter()
            .filter(|(town, ledger)| **town == TownId::world() || ledger.machines.contains(name))
            .map(|(town, ledger)| (town.clone(), ledger.next - 1))
            .collect()
    }

    /// The items of `town` after `after` that fit in one answer to `name`.
    pub fn read(&self, name: &MachineName, town: &TownId, after: u64) -> Result<(Vec<(u64, Signed)>, bool), String> {
        let ledger = self.towns.get(town).ok_or_else(|| format!("the hub has no ledger for {town}"))?;
        if *town != TownId::world() && !ledger.machines.contains(name) {
            return Err(format!("{town} does not span {name}"));
        }
        self.read_world(town, after)
    }

    /// Reads for any member's explicit query: what it cannot open is of no
    /// use to it.
    pub fn read_world(&self, town: &TownId, after: u64) -> Result<(Vec<(u64, Signed)>, bool), String> {
        if !self.towns.contains_key(town) {
            return Err(format!("the hub has no ledger for {town}"));
        }
        let files =
            numbered(&self.dir.join(town.as_str())).map_err(|error| format!("reading {town}'s ledger: {error}"))?;
        // An answer is its frame around the items, each separated by a
        // comma; it holds items while that stays within one frame.
        let empty =
            wire::encode(&Down::Told { id: u64::MAX, answer: Ok(Told::Entries { entries: Vec::new(), more: true }) });
        let mut size = empty.len();
        let mut entries = Vec::new();
        for (seq, file) in files.into_iter().filter(|(seq, _)| *seq > after) {
            let signed: Signed = read(&file).map_err(|error| format!("reading item {seq} of {town}: {error}"))?;
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
    fn add(&mut self, seq: u64, item: &Item) {
        if let Item::Entry(entry) = item {
            if let Some(machines) = &entry.machines {
                self.machines = machines.clone();
            }
            self.paths.extend(entry.paths.iter().cloned());
            self.dirs.extend(entry.dirs.iter().cloned());
            self.entries.insert(seq, entry.id.clone());
        }
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
