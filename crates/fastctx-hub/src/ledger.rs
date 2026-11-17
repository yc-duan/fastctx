//! The projects' shared ledgers, as the hub keeps them (`world-network.md`,
//! 共享账本): each entry as its author signed it, by project and number, in
//! the hub's own asset library (`ledger_entries`, design §28.58). A hub of
//! before kept them as `<hub>/ledger/<town id>/<seq:020>.json`; those are
//! taken in as it opens, checked entry for entry, and the folder is kept
//! aside as `ledger.imported`.
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
//! numbered under the store's lock and kept once that lock is let go
//! (`write`); other projects, and everything else the hub does, do not wait
//! on that disk. A read takes only the committed end under the ledger lock,
//! then reads the entries off all locks and off the member's frame reader
//! (`reading`).

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::sync::Arc;

use fastctx_content::v2::Library;
use fastctx_net::directory::Directory;
use fastctx_net::ledger::Entry;
use fastctx_net::sign::Signed;
use fastctx_net::wire::{self, Down, Told};
use fastctx_types::MachineName;
use fastctx_types::access::TownId;

use crate::Hub;

pub(crate) mod for_reader;
mod history_ranges;
pub(crate) mod history_requests;
mod reading;
pub(crate) mod requests;
pub(crate) use reading::{ask, entry as read_entry, projection};

/// What the hub knows of one project's ledger.
#[derive(Default)]
pub struct TownLedger {
    /// The number the next entry gets.
    pub next: u64,

    /// Every path written, in lower case, as a system that ignores case
    /// would compare them.
    paths: BTreeSet<String>,
    memory: fastctx_net::memory::Index,
    pub(crate) history: fastctx_net::topic_history::Index,
    sequences: BTreeMap<String, u64>,
    history_ranges: Arc<history_ranges::HistoryRanges>,
}

#[derive(Default)]
pub struct Ledgers {
    /// The hub's own library, where the entries are kept.
    library: Option<Library>,
    pub towns: BTreeMap<TownId, TownLedger>,
    /// What lets one entry of a project be written at a time.
    writing: BTreeMap<TownId, Arc<tokio::sync::Mutex<()>>>,
}

/// Where an entry of a project is kept, to be read off every lock.
#[derive(Clone)]
pub(crate) struct EntryAt {
    library: Option<Library>,
    town: TownId,
    seq: u64,
}

impl EntryAt {
    /// The entry, as its author signed it. Call on a thread that may block.
    pub(crate) fn read(&self) -> io::Result<Signed> {
        let library = self.library.as_ref().ok_or_else(|| io::Error::other("the hub's ledgers are not open"))?;
        let bytes = library.ledger_entry(self.town.as_str(), self.seq).map_err(io::Error::other)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("entry {} of {} is not kept", self.seq, self.town))
        })?;
        // A damaged entry must not make a read allocate beyond the multipart limit.
        if bytes.len() > wire::PARTS_AT_MOST {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the entry is larger than the multipart limit"));
        }
        serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

/// An entry checked to be kept: its number, and its file and bytes, written
/// off any lock before it is kept (`Ledgers::commit`).
struct Prepared {
    entry: Entry,
    mutation: Option<fastctx_net::memory::Mutation>,
    history: Option<fastctx_net::topic_history::Revision>,
    account: Option<fastctx_types::AccountName>,
    bytes: Vec<u8>,
}

impl Prepared {
    fn check_author(&self, hub: &Hub, store: &crate::store::Snapshot, from: &MachineName) -> Result<(), String> {
        if let Some(history) = &self.history {
            let account = self.account.as_ref().ok_or("project history writes require an explicit account")?;
            crate::history_author::check(
                hub,
                store,
                from,
                &self.entry.key,
                account,
                &history.revision.author,
                history.submission.is_some(),
            )?;
        }
        Ok(())
    }
}

pub struct Checked {
    seq: u64,
    prepared: Arc<Prepared>,
}

/// Entries read at a time as the hub opens: each may be as large as an entry
/// may be, so a few (codex-357de52a #4).
const OPENING_PAGE: usize = 8;

impl Ledgers {
    /// Where the entry `seq` of `town` is kept.
    pub(crate) fn entry_at(&self, town: &TownId, seq: u64) -> EntryAt {
        EntryAt { library: self.library.clone(), town: town.clone(), seq }
    }

    /// Reads every project's ledger the hub keeps in `library`, its own,
    /// those of a hub of before under `hub` taken in first; says what it
    /// took in. Call on a thread that may block.
    pub fn open(hub: &Path, identity: &fastctx_net::ids::HubId, library: Library) -> io::Result<(Self, Vec<String>)> {
        let said = import(hub, identity, &library)?;
        let mut towns = BTreeMap::new();
        for town in library.ledger_towns().map_err(io::Error::other)? {
            let Ok(id) = TownId::try_from(town.clone()) else { continue };
            let mut ledger = TownLedger { next: 1, ..TownLedger::default() };
            let mut after = 0;
            loop {
                let page = library.ledger_entries(&town, after, OPENING_PAGE).map_err(io::Error::other)?;
                let Some((last, _)) = page.last() else { break };
                after = *last;
                for (seq, bytes) in page {
                    let signed: Signed = serde_json::from_slice(&bytes).map_err(|error| {
                        io::Error::new(io::ErrorKind::InvalidData, format!("entry {seq} of {town}: {error}"))
                    })?;
                    let entry = fastctx_net::ledger::open(&signed, identity, &id)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    ledger.memory.take(&entry).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    ledger.history.take(&entry).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    ledger.add(&entry, seq);
                    ledger.next = seq + 1;
                }
            }
            towns.insert(id, ledger);
        }
        Ok((Self { library: Some(library), towns, writing: BTreeMap::new() }, said))
    }

    /// Checks an entry of `town` from `from`, and numbers it: nothing is
    /// kept yet (`commit`), and only one entry of a project is checked and
    /// kept at a time (`write`).
    fn check(
        &self,
        store: &crate::store::Snapshot,
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
            requests::authorize(
                directory,
                &crate::origin::Origin { name: from.clone(), key: prepared.entry.key },
                account,
                town,
                false,
            )?;
        } else if prepared.mutation.is_some() || prepared.history.is_some() {
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
        if let Some(revision) = &prepared.history {
            ledger
                .map(|ledger| &ledger.history)
                .unwrap_or(&fastctx_net::topic_history::Index::default())
                .check(revision)?;
            if let Some(ledger) = ledger {
                crate::history_limits::project(&ledger.history, &revision.revision.name)?;
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
        let seq = self.towns.get(town).map_or(1, |ledger| ledger.next);
        Ok(Checked { seq, prepared: prepared.clone() })
    }

    /// Keeps an entry checked and written and returns its number.
    pub fn commit(&mut self, town: &TownId, checked: Checked) -> u64 {
        let ledger = self.towns.entry(town.clone()).or_insert_with(|| TownLedger { next: 1, ..TownLedger::default() });
        if let Some(mutation) = &checked.prepared.mutation {
            ledger.memory.append(mutation.clone());
        }
        if let Some(revision) = &checked.prepared.history {
            ledger.history.append(revision.clone());
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
    /// the library its entries are kept in, which the caller lets go of
    /// them in off the hub's locks.
    pub fn forget(&mut self, town: &TownId) -> Option<Library> {
        self.towns.remove(town);
        self.writing.remove(town);
        self.library.clone()
    }

    /// Lets go the ledgers of projects the directory no longer names,
    /// archived ones kept: a hub stopped while it deleted one finishes here.
    /// Returns the projects whose ledgers went. Call on a thread that may
    /// block.
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
            if let Some(library) = self.forget(town) {
                library.ledger_forget(town.as_str()).map_err(io::Error::other)?;
            }
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
        let history = self.towns.get(&town).map(|ledger| ledger.history_ranges.clone()).unwrap_or_default();
        reading::Page { library: self.library.clone(), town, after, last, history }
    }
}

impl TownLedger {
    fn add(&mut self, entry: &Entry, seq: u64) {
        self.paths.extend(entry.files.iter().map(|file| file.path.to_lowercase()));
        self.sequences.extend(entry.files.iter().map(|file| (file.path.clone(), seq)));
        if entry.files.iter().any(|file| file.path.starts_with("history/")) {
            self.history_ranges.append(seq);
        }
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

/// Takes in the entries a hub of before kept as files under `hub`, checked
/// as it took them, each kept as its file's bytes: a stop on the way takes
/// in again what it had, the same. Once every one is kept the same, the
/// folder is put aside as `ledger.imported`; one that differs stops the hub
/// opening, named, its files left where they are. Says what it took in.
fn import(hub: &Path, identity: &fastctx_net::ids::HubId, library: &Library) -> io::Result<Vec<String>> {
    let dir = hub.join("ledger");
    let towns = match fastctx_platform::disk::read_dir(&dir) {
        Ok(towns) => towns,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut taken = Vec::new();
    for town in towns {
        let path = town?.path();
        let Some(id) =
            path.file_name().and_then(|name| name.to_str()).and_then(|name| TownId::try_from(name.to_string()).ok())
        else {
            continue;
        };
        for (seq, file) in numbered(&path)? {
            let bytes = fastctx_platform::disk::read(&file)?;
            let signed: Signed = serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", file.display())))?;
            fastctx_net::ledger::open(&signed, identity, &id)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", file.display())))?;
            library.ledger_keep(id.as_str(), seq, bytes).map_err(io::Error::other)?;
            taken.push((id.clone(), seq, file));
        }
    }
    // Every one kept as its file says before the folder goes aside.
    for (town, seq, file) in &taken {
        let kept = library.ledger_entry(town.as_str(), *seq).map_err(io::Error::other)?;
        if kept != Some(fastctx_platform::disk::read(file)?) {
            return Err(io::Error::other(format!("entry {seq} of {town} was not kept as {} holds it", file.display())));
        }
    }
    let mut aside = hub.join("ledger.imported");
    if fastctx_platform::disk::metadata(&aside).is_ok() {
        aside = hub.join(format!("ledger.imported-{}", jiff::Timestamp::now().as_second()));
    }
    fastctx_platform::durable_file::rename_durably(&dir, &aside)?;
    let projects: BTreeSet<&TownId> = taken.iter().map(|(town, _, _)| town).collect();
    Ok(vec![format!(
        "took {} entries of {} projects' ledgers into the hub's library; the files are kept aside in {}",
        taken.len(),
        projects.len(),
        aside.display()
    )])
}

/// The numbered files in a directory, in order.
fn numbered(directory: &Path) -> io::Result<Vec<(u64, std::path::PathBuf)>> {
    let mut found = Vec::new();
    let entries = match fastctx_platform::disk::read_dir(directory) {
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
    let prepared = fastctx_platform::task::spawn_blocking(move || {
        let size = fastctx_net::ledger::handed_size(&signed);
        if size > wire::PARTS_AT_MOST {
            return Err(format!("the entry is {size} bytes, more than the multipart limit ({})", wire::PARTS_AT_MOST));
        }
        let entry = fastctx_net::ledger::open(&signed, &hub_id, &project)?;
        // Pages — HTML an agent published into a project — are no more
        // (beta.18): an earlier node publishing one is refused.
        if entry.files.iter().any(|file| file.path.starts_with("pages/")) {
            return Err("pages were removed in Fastctx beta.18; nothing is published as a page any more".into());
        }
        let mutation = fastctx_net::memory::decode(&entry.files)?;
        let history = fastctx_net::topic_history::decode(&entry.files)?;
        if let Some(stored) = &history {
            let revision = &stored.revision;
            let text = entry
                .files
                .iter()
                .find(|file| file.path == fastctx_net::topic_history::path(&revision.name, revision.version, "md"))
                .expect("decoded history text");
            fastctx_types::history::markdown::parse_written(
                &revision.name,
                std::str::from_utf8(&text.bytes.0).map_err(|error| error.to_string())?,
            )?;
        }
        let bytes = json(&signed).map_err(|error| error.to_string())?;
        if account.is_some() && mutation.is_none() && history.is_none() {
            return Err("a memory write must contain a memory revision or evidence".into());
        }
        Ok(Arc::new(Prepared { entry, mutation, history, account, bytes }))
    })
    .await
    .map_err(|error| format!("checking the ledger entry: {error}"))??;
    let writing = hub.ledgers.lock().expect("never poisoned").writing.entry(town.clone()).or_default().clone();
    let _one = writing.lock().await;
    let checked = {
        let store = hub.store();
        prepared.check_author(hub, &store, from)?;
        hub.ledgers.lock().expect("never poisoned").check(&store, from, &town, &prepared)?
    };
    let (seq, proposed) = (checked.seq, prepared.clone());
    let library =
        hub.ledgers.lock().expect("never poisoned").library.clone().ok_or("the hub's ledgers are not open")?;
    let (keeping, author, project) = (hub.clone(), from.clone(), town.clone());
    // Checked again under the directory as it is kept, as nothing that
    // changed meanwhile lets an entry in that it would not now; kept with the
    // directory let go, so a slow disk holds up no other edit of it; and
    // checked once more: changed as it was written, it is taken back before
    // anyone reads it — none does before it is committed (codex-357de52a #5).
    fastctx_platform::task::spawn_blocking(move || {
        let recheck = || -> Result<(), String> {
            let store = keeping.edit_store();
            proposed.check_author(&keeping, &store, &author)?;
            keeping.ledgers.lock().expect("never poisoned").check(&store, &author, &project, &proposed)?;
            Ok(())
        };
        recheck()?;
        library.ledger_keep(project.as_str(), seq, proposed.bytes.clone()).map_err(|error| error.to_string())?;
        if let Err(why) = recheck() {
            library
                .ledger_withdraw(project.as_str(), seq)
                .map_err(|error| format!("{why}; taking the entry back: {error}"))?;
            return Err(why);
        }
        Ok::<_, String>(())
    })
    .await
    .map_err(|error| format!("the hub could not keep the entry: {error}"))?
    .map_err(|why| format!("the hub could not keep the entry: {why}"))?;
    let history = checked.prepared.history.as_ref().map(|stored| stored.revision.clone());

    let (seq, tell) = {
        let store = hub.store();
        let directory = &store.book.directory;
        // Deleted while its entry was written: the entry goes with it.
        if directory.project(&town).is_none() && !store.book.archived.iter().any(|project| project.town == town) {
            let library = hub.ledgers.lock().expect("never poisoned").forget(&town);
            drop(store);
            remove_later(hub, town.clone(), library);
            return Err(format!("the project {town} was deleted"));
        }
        let seq = hub.ledgers.lock().expect("never poisoned").commit(&town, checked);
        let tell: Vec<_> =
            store.indexed().project_readers(&town).into_iter().map(|at| directory.members[at].name.clone()).collect();
        (seq, tell)
    };
    hub.log.line(format_args!("{from} wrote entry {seq} of {town}"));
    // Into the asset library too, as each reader of the project may know it
    // (§29, 2.4).
    crate::assets::ledger_views::look_at(hub, &town);
    for machine in tell {
        // A machine of a release before history reads only that the ledger
        // changed, and takes the history append as any entry.
        let reads_history = hub.sessions().get(&machine).is_some_and(|session| session.replicated_records);
        let changed = match &history {
            Some(revision) if reads_history => {
                Down::HistoryChanged { town: town.clone(), name: revision.name.clone(), version: revision.version }
            }
            _ => Down::LedgerChanged { town: town.clone() },
        };
        hub.tell(&machine, changed);
    }
    Ok(Told::Written { seq })
}

/// Lets the deleted project `town`'s ledger go: at once from what the hub
/// holds, so no machine is told of it or reads it again, and its entries
/// off the hub's locks.
pub(crate) fn delete(hub: &Hub, town: TownId) {
    let library = hub.ledgers.lock().expect("never poisoned").forget(&town);
    remove_later(hub, town, library);
}

fn remove_later(hub: &Hub, town: TownId, library: Option<Library>) {
    let Some(library) = library else { return };
    let log = hub.log.clone();
    fastctx_platform::task::spawn_blocking(move || {
        if let Err(error) = library.ledger_forget(town.as_str()) {
            log.line(format_args!("deleting the ledger of {town}: {error}; done when the hub opens next"));
        }
    });
}
