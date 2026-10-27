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
//!
//! The entries of one project are written one at a time, each checked and
//! numbered under the store's lock and written to disk once that lock is
//! let go (`write`); other projects, and everything else the hub does, do
//! not wait on that disk. A read takes only the committed end under the
//! ledger lock, then reads the numbered files off all locks and off the
//! member's frame reader (`reading`).

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_net::directory::Directory;
use fastctx_net::ledger::Entry;
use fastctx_net::sign::Signed;
use fastctx_net::wire::{self, Down, Told};
use fastctx_platform::{Permissions, PublishFailure, ensure_private_dir, publish_checked};
use fastctx_types::MachineName;
use fastctx_types::access::TownId;

use crate::Hub;
use crate::store::Store;

mod page_index;
mod reading;
pub(crate) mod requests;
pub(crate) use reading::ask;

/// What the hub knows of one project's ledger.
#[derive(Default)]
pub struct TownLedger {
    /// The number the next entry gets.
    pub next: u64,

    /// Every path written, in lower case, as a system that ignores case
    /// would compare them.
    paths: BTreeSet<String>,
    memory: fastctx_net::memory::Index,
    sequences: BTreeMap<String, u64>,
}

#[derive(Default)]
pub struct Ledgers {
    dir: PathBuf,
    pub towns: BTreeMap<TownId, TownLedger>,
    /// What lets one entry of a project be written at a time.
    writing: BTreeMap<TownId, Arc<tokio::sync::Mutex<()>>>,
}

/// An entry checked to be kept: its number, and its file and bytes, written
/// off any lock before it is kept (`Ledgers::commit`).
struct Prepared {
    entry: Entry,
    mutation: Option<fastctx_net::memory::Mutation>,
    account: Option<fastctx_types::AccountName>,
    bytes: Vec<u8>,
}

pub struct Checked {
    seq: u64,
    prepared: Arc<Prepared>,
    path: PathBuf,
}

impl Ledgers {
    /// Reads every project's ledger under `hub`.
    pub fn open(hub: &Path, identity: &fastctx_net::ids::HubId) -> io::Result<Self> {
        let dir = hub.join("ledger");
        let mut towns = BTreeMap::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self { dir, towns, writing: BTreeMap::new() });
            }
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
                let entry = fastctx_net::ledger::open(&signed, identity, &id)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                ledger.memory.take(&entry).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                ledger.add(&entry, seq);
                ledger.next = seq + 1;
            }
            towns.insert(id, ledger);
        }
        Ok(Self { dir, towns, writing: BTreeMap::new() })
    }

    /// Checks an entry of `town` from `from`, and numbers it: nothing is
    /// kept yet (`commit`), and only one entry of a project is checked and
    /// kept at a time (`write`).
    fn check(
        &self,
        store: &Store,
        from: &MachineName,
        town: &TownId,
        prepared: &Arc<Prepared>,
    ) -> Result<Checked, String> {
        let directory = &store.book.directory;
        let member = directory.member(from).ok_or_else(|| format!("{from} is not a member"))?;
        let project = directory.project(town).ok_or_else(|| format!("there is no project {town} on this hub"))?;
        if !directory.member_in_project(from, town) {
            return Err(format!("{from} acts for no account that may write in the project {}", project.name));
        }
        if let Some(account) = &prepared.account {
            requests::authorize(directory, from, account, town, false)?;
        } else if prepared.mutation.is_some() {
            return Err("project memory writes require an explicit account and a current client".into());
        }
        let entry = &prepared.entry;
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
        if let Some(mutation) = &prepared.mutation {
            ledger
                .map(|ledger| &ledger.memory)
                .unwrap_or(&fastctx_net::memory::Index::default())
                .check_mutation(mutation)?;
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
        let seq = self.towns.get(town).map_or(1, |ledger| ledger.next);
        let path = self.dir.join(town.as_str()).join(format!("{seq:020}.json"));
        Ok(Checked { seq, prepared: prepared.clone(), path })
    }

    /// Keeps an entry checked and written and returns its number.
    pub fn commit(&mut self, town: &TownId, checked: Checked) -> u64 {
        let ledger = self.towns.entry(town.clone()).or_insert_with(|| TownLedger { next: 1, ..TownLedger::default() });
        if let Some(mutation) = &checked.prepared.mutation {
            ledger.memory.append(mutation.clone());
        }
        ledger.add(&checked.prepared.entry, checked.seq);
        ledger.next = checked.seq + 1;
        checked.seq
    }

    /// The projects `name` spans, with each one's latest entry: a project
    /// says the machines it spans under the names they had when it was
    /// written, which still name them.
    /// A project archived with a deleted account spans nothing until it is
    /// handed on: no machine may read it meanwhile.
    pub fn towns_of(&self, directory: &Directory, name: &MachineName) -> Vec<(TownId, u64)> {
        self.towns
            .iter()
            .filter(|(town, _)| directory.member_in_project(name, town))
            .map(|(town, ledger)| (town.clone(), ledger.next - 1))
            .collect()
    }

    /// Lets a deleted project's ledger go from what the hub holds; returns
    /// the folder of its files, which the caller removes.
    pub fn forget(&mut self, town: &TownId) -> PathBuf {
        self.towns.remove(town);
        self.writing.remove(town);
        self.dir.join(town.as_str())
    }

    /// Lets go the ledgers of projects the directory no longer names,
    /// archived ones kept: a hub stopped while it deleted one finishes here.
    /// Returns the projects whose ledgers went.
    pub fn sweep(&mut self, book: &crate::store::Book) -> io::Result<Vec<TownId>> {
        let gone: Vec<TownId> = self
            .towns
            .keys()
            .filter(|town| {
                book.directory.project(town).is_none() && !book.archived.iter().any(|project| &project.town == *town)
            })
            .cloned()
            .collect();
        for town in &gone {
            remove_folder(&self.forget(town))?;
        }
        Ok(gone)
    }

    /// Every project with a ledger, with its latest entry.
    pub fn catalog(&self) -> Vec<(TownId, u64)> {
        self.towns.iter().map(|(town, ledger)| (town.clone(), ledger.next - 1)).collect()
    }

    /// The committed range to read, without listing or copying the ledger.
    fn page(&self, town: TownId, after: u64) -> reading::Page {
        let last = self.towns.get(&town).map_or(0, |ledger| ledger.next - 1);
        reading::Page { dir: self.dir.join(town.as_str()), town, after, last }
    }
}

impl TownLedger {
    fn add(&mut self, entry: &Entry, seq: u64) {
        self.paths.extend(entry.files.iter().map(|file| file.path.to_lowercase()));
        self.sequences.extend(entry.files.iter().map(|file| (file.path.clone(), seq)));
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

/// Takes an entry of `town` from `from`: checked and numbered under the
/// store's lock, written off it, kept, and every member that may read the
/// project told of it, as one that keeps a copy it does not span keeps it
/// current (`world-network.md`, 隔着网络也要顺手). One entry of a project at
/// a time.
pub(crate) async fn write(hub: &Arc<Hub>, from: &MachineName, town: TownId, signed: Signed) -> Result<Told, String> {
    write_as(hub, from, town, signed, None).await
}

pub(crate) async fn write_as(
    hub: &Arc<Hub>,
    from: &MachineName,
    town: TownId,
    signed: Signed,
    account: Option<fastctx_types::AccountName>,
) -> Result<Told, String> {
    let (hub_id, project) = (hub.store().hub.id.clone(), town.clone());
    let prepared = tokio::task::spawn_blocking(move || {
        let size = fastctx_net::ledger::handed_size(&signed);
        if size > wire::PARTS_AT_MOST {
            return Err(format!("the entry is {size} bytes, more than the multipart limit ({})", wire::PARTS_AT_MOST));
        }
        let entry = fastctx_net::ledger::open(&signed, &hub_id, &project)?;
        let mutation = fastctx_net::memory::decode(&entry.files)?;
        let bytes = json(&signed).map_err(|error| error.to_string())?;
        if account.is_some() && mutation.is_none() {
            return Err("a memory write must contain a memory revision or evidence".into());
        }
        Ok(Arc::new(Prepared { entry, mutation, account, bytes }))
    })
    .await
    .map_err(|error| format!("checking the ledger entry: {error}"))??;
    let writing = hub.ledgers.lock().expect("never poisoned").writing.entry(town.clone()).or_default().clone();
    let _one = writing.lock().await;
    let checked = {
        let store = hub.store();
        hub.ledgers.lock().expect("never poisoned").check(&store, from, &town, &prepared)?
    };
    let (path, proposed) = (checked.path.clone(), prepared.clone());
    let (publishing, author, project) = (hub.clone(), from.clone(), town.clone());
    let published = tokio::task::spawn_blocking(move || {
        let folder = path.parent().expect("an entry has a folder");
        ensure_private_dir(folder).map_err(|error| PublishFailure { error, published: false }).and_then(|()| {
            publish_checked(&path, &proposed.bytes, Permissions::Private, false, || {
                let store = publishing.store();
                publishing
                    .ledgers
                    .lock()
                    .expect("never poisoned")
                    .check(&store, &author, &project, &proposed)
                    .map_err(|why| io::Error::new(io::ErrorKind::PermissionDenied, why))?;
                Ok(store)
            })
        })
    })
    .await
    .map_err(|error| format!("the hub could not keep the entry: {error}"))?;
    match published {
        Ok(()) => {}
        // In place for all to read, though not known to be on disk for good:
        // kept as written, since the next entry of the project follows it.
        Err(failure) if failure.published => hub.log.line(format_args!(
            "an entry of {town} from {from} is in place, but making it last failed: {}",
            failure.error
        )),
        Err(failure) => return Err(format!("the hub could not keep the entry: {}", failure.error)),
    }
    let (seq, tell) = {
        let store = hub.store();
        let directory = &store.book.directory;
        // Deleted while its entry was written: the entry goes with it.
        if directory.project(&town).is_none() && !store.book.archived.iter().any(|project| project.town == town) {
            let folder = hub.ledgers.lock().expect("never poisoned").forget(&town);
            drop(store);
            remove_later(hub, town.clone(), folder);
            return Err(format!("the project {town} was deleted"));
        }
        let seq = hub.ledgers.lock().expect("never poisoned").commit(&town, checked);
        let tell: Vec<_> = directory
            .members
            .iter()
            .filter(|member| directory.member_in_project(&member.name, &town))
            .map(|member| member.name.clone())
            .collect();
        (seq, tell)
    };
    hub.log.line(format_args!("{from} wrote entry {seq} of {town}"));
    for machine in tell {
        hub.tell(&machine, Down::LedgerChanged { town: town.clone() });
    }
    Ok(Told::Written { seq })
}

/// Lets the deleted project `town`'s ledger go: at once from what the hub
/// holds, so no machine is told of it or reads it again, and its files off
/// the hub's locks.
pub(crate) fn delete(hub: &Hub, town: TownId) {
    let folder = hub.ledgers.lock().expect("never poisoned").forget(&town);
    remove_later(hub, town, folder);
}

fn remove_later(hub: &Hub, town: TownId, folder: PathBuf) {
    let log = hub.log.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = remove_folder(&folder) {
            log.line(format_args!("deleting the ledger of {town}: {error}; done when the hub opens next"));
        }
    });
}

fn remove_folder(folder: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(folder) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
