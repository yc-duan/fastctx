//! What a machine without a network keeps in memory of each history scope
//! (`world-history.md`): every document's latest revision, every version's
//! revision with the digest of its text and where that text is, and each
//! local copy as it was last written. A call reads from disk only what
//! changed since the last one and the one text it needs, and rewrites only
//! a copy whose version changed or that was changed on disk: what a call
//! costs follows the change, not the history (`world-foundation.md`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fastctx_net::topic_history::{self, Index};
use fastctx_types::TownName;
use fastctx_types::history::{Catalog, Document, HistoryRevision, stored::Stored};
use fastctx_types::layout::WorldDir;

/// The most scopes kept; those used longest ago are let go first.
const KEPT: usize = 4096;

/// How long ago a folder must have changed before what was read after the
/// change is trusted: one made within the same moment could go unseen.
const SETTLED: Duration = Duration::from_secs(2);

/// Every scope's shelf, by the scope's identity.
#[derive(Default)]
pub(crate) struct Shelves {
    kept: Mutex<Kept>,
}

#[derive(Default)]
struct Kept {
    shelves: HashMap<String, (Arc<tokio::sync::Mutex<Shelf>>, u64)>,
    used: u64,
}

impl Shelves {
    /// The shelf of the scope `identity`: its lock is the scope's, held for
    /// the whole of a call, writes included.
    pub(crate) fn of(&self, identity: &str) -> Arc<tokio::sync::Mutex<Shelf>> {
        let mut kept = self.kept.lock().expect("never poisoned");
        kept.used += 1;
        let used = kept.used;
        if let Some((shelf, at)) = kept.shelves.get_mut(identity) {
            *at = used;
            return shelf.clone();
        }
        if kept.shelves.len() >= KEPT {
            // A shelf a call holds stays, so two calls never hold two
            // shelves of one scope.
            let mut idle: Vec<(u64, String)> = kept
                .shelves
                .iter()
                .filter(|(_, (shelf, _))| Arc::strong_count(shelf) == 1)
                .map(|(identity, (_, at))| (*at, identity.clone()))
                .collect();
            idle.sort_unstable();
            let over = kept.shelves.len() + 1 - KEPT;
            for (_, identity) in idle.into_iter().take(over) {
                kept.shelves.remove(&identity);
            }
        }
        let shelf = Arc::new(tokio::sync::Mutex::new(Shelf::default()));
        kept.shelves.insert(identity.to_string(), (shelf.clone(), used));
        shelf
    }
}

/// Where a scope's history is on disk, and its local copies.
pub(crate) struct Place {
    pub of: Of,
    /// The folder of the local copies, one Markdown file a document.
    pub copies: PathBuf,
}

pub(crate) enum Of {
    /// A project's history is in its signed ledger.
    Project { world: WorldDir, town: TownName },
    /// A conversation's is a folder a document, a file a version.
    Conversation { records: PathBuf },
}

/// One scope's history as this machine last read it.
#[derive(Default)]
pub(crate) struct Shelf {
    documents: BTreeMap<String, Shelved>,
    /// A project's: the ledger's history index and the last entry it took.
    ledger: Option<(Index, u64)>,
    /// When the folder that says what changed last changed, as read: the
    /// ledger's entries, or the conversation's records.
    seen: Seen,
    /// A conversation's: when each document's folder last changed, as read.
    folders: BTreeMap<String, Seen>,
    /// Each local copy as last written here.
    copies: BTreeMap<String, Copy>,
}

pub(crate) struct Shelved {
    pub latest: HistoryRevision,
    pub versions: Vec<HistoryRevision>,
    /// Each version's text, by version: its digest, and, in a project, the
    /// ledger entry that holds it.
    texts: BTreeMap<u64, Text>,
}

struct Text {
    digest: [u8; 32],
    entry: u64,
}

/// A local copy as written: the version it shows, and how it stood after.
struct Copy {
    version: u64,
    retired: bool,
    stood: Option<(u64, SystemTime)>,
}

/// When a folder changed, as read; trusted only once that change settled.
#[derive(Clone, Copy, Default, PartialEq)]
struct Seen(Option<SystemTime>);

impl Seen {
    fn after(changed: Option<SystemTime>) -> Self {
        Self(changed.filter(|at| SystemTime::now().duration_since(*at).is_ok_and(|age| age >= SETTLED)))
    }

    fn holds(self, changed: Option<SystemTime>) -> bool {
        self.0.is_some() && self.0 == changed
    }
}

fn changed(path: &Path) -> Result<Option<SystemTime>, String> {
    match fastctx_platform::disk::metadata(path) {
        Ok(metadata) => metadata.modified().map(Some).map_err(|error| error.to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn stood(path: &Path) -> Result<Option<(u64, SystemTime)>, String> {
    match fastctx_platform::disk::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            Ok(Some((metadata.len(), metadata.modified().map_err(|error| error.to_string())?)))
        }
        Ok(_) => Ok(Some((u64::MAX, SystemTime::UNIX_EPOCH))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

impl Shelf {
    /// Reads what changed on disk since this shelf last did. On a failure
    /// the shelf is to be let go, as it may hold part of a change.
    pub(crate) fn refresh(&mut self, place: &Place) -> Result<(), String> {
        match &place.of {
            Of::Project { world, town } => self.refresh_project(world, town),
            Of::Conversation { records } => self.refresh_conversation(records),
        }
    }

    fn refresh_project(&mut self, world: &WorldDir, town: &TownName) -> Result<(), String> {
        let entries = world.town(town).signed();
        let now = changed(&entries)?;
        if self.ledger.is_some() && self.seen.holds(now) {
            return Ok(());
        }
        let (mut index, mut through) = self.ledger.take().unwrap_or_default();
        // A ledger that went back is read again from its start.
        if crate::ledger::replica_seq(world, town).map_err(|error| error.to_string())? < through {
            (index, through) = (Index::default(), 0);
            self.documents.clear();
        }
        for (seq, entry) in crate::ledger::entries_after(world, town, through).map_err(|error| error.to_string())? {
            if let Some(revision) = topic_history::decode(&entry.files)? {
                index.check(&revision)?;
                let latest = &revision.revision;
                let text = entry
                    .files
                    .iter()
                    .find(|file| file.path == topic_history::path(&latest.name, latest.version, "md"))
                    .ok_or("the history revision has no Markdown text")?;
                self.put(latest.clone(), fastctx_net::sign::sha256(&text.bytes.0), seq);
                index.append(revision);
            }
            through = seq;
        }
        self.ledger = Some((index, through));
        self.seen = Seen::after(now);
        Ok(())
    }

    fn refresh_conversation(&mut self, records: &Path) -> Result<(), String> {
        if let Some(paths) = super::import::state::archiving(records)? {
            self.documents.clear();
            self.folders.clear();
            self.seen = Seen::default();
            for path in paths {
                let stored = read_stored(&path)?;
                self.put(stored.revision, fastctx_net::sign::sha256(stored.text.as_bytes()), 0);
            }
            return Ok(());
        }
        let now = changed(records)?;
        if now.is_none() {
            self.documents.clear();
            self.folders.clear();
            self.seen = Seen::default();
            return Ok(());
        }
        // The documents there, when one may have come or gone.
        if !self.seen.holds(now) {
            let mut names = BTreeSet::new();
            for entry in fastctx_platform::disk::read_dir(records).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                if entry.file_type().map_err(|error| error.to_string())?.is_dir() {
                    names.insert(entry.file_name().to_string_lossy().to_string());
                }
            }
            self.documents.retain(|name, _| names.contains(name));
            self.folders.retain(|name, _| names.contains(name));
            for name in names {
                self.folders.entry(name).or_default();
            }
            self.seen = Seen::after(now);
        }
        let names: Vec<String> = self.folders.keys().cloned().collect();
        for name in names {
            let folder = records.join(&name);
            let now = changed(&folder)?;
            if self.folders.get(&name).is_some_and(|seen| seen.holds(now)) {
                continue;
            }
            self.read_versions(&folder, &name)?;
            self.folders.insert(name, Seen::after(now));
        }
        Ok(())
    }

    /// Reads the versions of the document `name` in `folder` not read yet;
    /// every version again when one read before is gone.
    fn read_versions(&mut self, folder: &Path, name: &str) -> Result<(), String> {
        fastctx_types::history::markdown::check_name(name)?;
        let mut listed = BTreeMap::new();
        for file in fastctx_platform::disk::read_dir(folder).map_err(|error| error.to_string())? {
            let path = file.map_err(|error| error.to_string())?.path();
            if path.extension().is_some_and(|extension| extension == "json")
                && let Some(version) =
                    path.file_stem().and_then(|stem| stem.to_str()).and_then(|stem| stem.parse().ok())
            {
                listed.insert(version, path);
            }
        }
        if self.documents.get(name).is_some_and(|kept| kept.texts.keys().any(|version| !listed.contains_key(version))) {
            self.documents.remove(name);
        }
        for (version, path) in listed {
            if self.documents.get(name).is_some_and(|kept| kept.texts.contains_key(&version)) {
                continue;
            }
            let stored = read_stored(&path)?;
            if stored.revision.version != version || stored.revision.name != name {
                return Err(format!("{} holds another revision than its name says", path.display()));
            }
            self.put(stored.revision, fastctx_net::sign::sha256(stored.text.as_bytes()), 0);
        }
        Ok(())
    }

    /// Takes the revision `revision`, whose text has the digest `digest`
    /// and, in a project, is in the ledger entry `entry`.
    pub(crate) fn put(&mut self, revision: HistoryRevision, digest: [u8; 32], entry: u64) {
        let version = revision.version;
        let shelved = self.documents.entry(revision.name.clone()).or_insert_with(|| Shelved {
            latest: revision.clone(),
            versions: Vec::new(),
            texts: BTreeMap::new(),
        });
        if shelved.texts.insert(version, Text { digest, entry }).is_some() {
            return;
        }
        let at = shelved.versions.partition_point(|known| known.version < version);
        shelved.versions.insert(at, revision);
        shelved.latest = shelved.versions.last().expect("a version was put").clone();
    }

    /// How many documents the scope holds, retired ones included.
    pub(crate) fn count(&self) -> usize {
        self.documents.len()
    }

    pub(crate) fn document(&self, name: &str) -> Option<&Shelved> {
        self.documents.get(name)
    }

    pub(crate) fn catalog(&self) -> Catalog {
        let mut catalog = Catalog { documents: self.documents.values().map(|kept| kept.latest.clone()).collect() };
        catalog.sort();
        catalog
    }

    /// The version `version` of the document `name`, read from disk.
    pub(crate) fn stored(&self, place: &Place, name: &str, version: u64) -> Result<Stored, String> {
        let shelved = self.documents.get(name).ok_or("the history revision does not exist")?;
        let text = shelved.texts.get(&version).ok_or("the history revision does not exist")?;
        let revision = shelved
            .versions
            .iter()
            .find(|revision| revision.version == version)
            .ok_or("the history revision does not exist")?
            .clone();
        match &place.of {
            Of::Project { world, town } => {
                let entry = crate::ledger::entry(world, town, text.entry)
                    .map_err(|error| error.to_string())?
                    .ok_or("the ledger entry of the history revision is gone")?;
                let file = entry
                    .files
                    .iter()
                    .find(|file| file.path == topic_history::path(name, version, "md"))
                    .ok_or("the history revision has no Markdown text")?;
                let text = String::from_utf8(file.bytes.0.clone()).map_err(|error| error.to_string())?;
                Ok(Stored { revision, text, submission: None })
            }
            Of::Conversation { records } => {
                let path = records.join(name).join(format!("{version}.json"));
                read_stored(&super::import::state::readable(&path)?)
            }
        }
    }

    /// The document `name` as it stands, its text read from disk.
    pub(crate) fn whole(&self, place: &Place, name: &str) -> Result<Option<Document>, String> {
        let Some(shelved) = self.documents.get(name) else { return Ok(None) };
        let stored = self.stored(place, name, shelved.latest.version)?;
        Ok(Some(Document { revision: stored.revision, text: stored.text, versions: shelved.versions.clone() }))
    }

    /// The text of the newest version of `name` whose digest is `sha256`.
    pub(crate) fn seen(&self, place: &Place, name: &str, sha256: &str) -> Result<Option<String>, String> {
        let Some(shelved) = self.documents.get(name) else { return Ok(None) };
        let found =
            shelved.texts.iter().rev().find(|(_, text)| hex(&text.digest) == sha256).map(|(version, _)| *version);
        found.map(|version| self.stored(place, name, version).map(|stored| stored.text)).transpose()
    }

    /// Writes again every local copy whose document moved on since it was
    /// written here, or that was changed on disk since: a copy of a retired
    /// document is taken away.
    pub(crate) fn keep_copies(&mut self, place: &Place) -> Result<(), String> {
        let names: Vec<String> = self.documents.keys().cloned().collect();
        for name in names {
            let latest = self.documents[&name].latest.clone();
            let path = place.copies.join(format!("{name}.md"));
            let now = stood(&path)?;
            let copy = self.copies.get(&name);
            let current = copy.is_some_and(|copy| copy.version == latest.version && copy.retired == latest.retired);
            if current && copy.is_some_and(|copy| copy.stood == now) {
                continue;
            }
            // A copy untouched since it was written here shows a version
            // read here: what it holds is known, and goes without a trace.
            let untouched = copy.is_some_and(|copy| copy.stood.is_some() && copy.stood == now);
            let text = (!latest.retired).then(|| self.stored(place, &name, latest.version)).transpose()?;
            let previous = match (&text, untouched) {
                (Some(_), true) => fastctx_platform::disk::read_to_string(&path).ok(),
                (Some(text), false) => Some(text.text.clone()),
                (None, _) => None,
            };
            self.write_copy(
                &path,
                &name,
                &latest,
                text.as_ref().map(|stored| stored.text.as_str()),
                previous.as_deref(),
            )?;
        }
        Ok(())
    }

    /// Writes the local copy of `name` to show `latest`, whose text is
    /// `text` (none for a retired document), over what held `previous`.
    pub(crate) fn write_copy(
        &mut self,
        path: &Path,
        name: &str,
        latest: &HistoryRevision,
        text: Option<&str>,
        previous: Option<&str>,
    ) -> Result<(), String> {
        let previous = previous.map(crate::replicas::bodies::Text::of);
        match text {
            None => crate::replicas::files::retire(path, None, previous.as_ref()).map_err(|error| error.to_string())?,
            Some(text) => {
                crate::replicas::files::publish(path, text, None, previous.as_ref())
                    .map_err(|error| error.to_string())?;
            }
        }
        let copy = Copy { version: latest.version, retired: latest.retired, stood: stood(path)? };
        self.copies.insert(name.to_string(), copy);
        Ok(())
    }
}

fn read_stored(path: &Path) -> Result<Stored, String> {
    let bytes = fastctx_platform::disk::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let stored: Stored = serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    stored.validate()?;
    Ok(stored)
}

fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
