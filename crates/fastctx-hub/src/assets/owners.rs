//! The hub's owner libraries (design §6.2): one per account, one per body
//! that several accounts own together, and one for the hub itself, each a
//! directory under `assets/`. As many are open at once as the machine's
//! memory allows (`open_at_once`), the least recently used that nobody
//! holds closed first; one left idle ten minutes is closed. A library is
//! opened under its own slot, so a slow opening holds up no other library.
//!
//! The first time this process opens a library it settles what a stop left
//! half done and checks it against the hub's record of it; opened again
//! after it was closed, it settles only what a failed write may have left,
//! for nothing else can change it while it is shut — every write goes
//! through an open library. Shut, where it stood is kept, so a device
//! asking what changed since is answered without opening it (`latest`).

use fastctx_content::v2::{Latest, Library};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a deleted library's directory is moved aside under, before it goes.
const DISCARDED: &str = "discarded-";

/// How long an idle library stays open.
pub(crate) const IDLE: Duration = Duration::from_secs(600);

/// How many libraries are kept open at once: one for each 8 MiB of the
/// machine's memory, which an open library's caches stay well within, from
/// 256 to 1024. Each machine shared whole has a library of its own, and the
/// passes of devices touch every library they follow: kept too few, every
/// pass opened one again (64 on a hub of 128 opened six to twelve a second).
/// The files they hold are well within what the hub raises its own limit to
/// as it starts (`fastctx_platform::limits::raise_open_files`).
pub(crate) fn open_at_once() -> usize {
    let physical = fastctx_platform::memory::physical().unwrap_or(2 << 30);
    usize::try_from(physical / (8 << 20)).unwrap_or(usize::MAX).clamp(256, 1024)
}

/// Whose library.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Owner {
    /// An account by its name and when it was opened: one deleted and made
    /// again under the same name owns another library.
    Account(String, i64),
    Body(String),
    Hub,
}

/// A name as a directory: as it is when it is plain, hex otherwise.
fn directory_name(name: &str) -> String {
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && !name.starts_with("x-")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if plain { name.to_string() } else { format!("x-{}", super::witness::hex(name.as_bytes())) }
}

impl Owner {
    /// How the witness and the checkpoints name it.
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Account(name, opened) => format!("account:{name}:{opened}"),
            Self::Body(id) => format!("body:{id}"),
            Self::Hub => "hub".into(),
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key.split_once(':') {
            Some(("account", rest)) => {
                let (name, opened) = rest.rsplit_once(':')?;
                Some(Self::Account(name.into(), opened.parse().ok()?))
            }
            Some(("body", id)) => Some(Self::Body(id.into())),
            None if key == "hub" => Some(Self::Hub),
            _ => None,
        }
    }

    /// The owner whose library a directory named `name` under `kind`
    /// (`account` or `body`) is, read back as `directory` names it.
    fn of_directory(kind: &str, name: &str) -> Option<Self> {
        let plain = match name.strip_prefix("x-") {
            Some(hex) => String::from_utf8(unhex(hex)?).ok()?,
            None => name.to_string(),
        };
        let owner = match kind {
            "account" => {
                let (account, opened) = plain.rsplit_once('.')?;
                Self::Account(account.into(), opened.parse().ok()?)
            }
            "body" => Self::Body(plain),
            _ => return None,
        };
        (owner.directory() == PathBuf::from(kind).join(name)).then_some(owner)
    }

    fn directory(&self) -> PathBuf {
        match self {
            Self::Account(name, opened) => PathBuf::from("account").join(directory_name(&format!("{name}.{opened}"))),
            Self::Body(id) => PathBuf::from("body").join(directory_name(id)),
            Self::Hub => PathBuf::from("hub"),
        }
    }
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}

/// One library's place among the open ones.
struct Slot {
    library: Mutex<Option<Library>>,
    used: Mutex<Instant>,
}

pub(crate) struct Owners {
    root: PathBuf,
    /// The hub's incarnation, stamped on every library it makes.
    incarnation: Vec<u8>,
    slots: Mutex<HashMap<Owner, Arc<Slot>>>,
    /// Owners whose library is being deleted: none is opened, under the same
    /// lock a deletion checks it is held by no one.
    fenced: Mutex<std::collections::HashSet<Owner>>,
    /// Owners whose library is still coming from where the hub was before
    /// it moved (`moving::libraries`): none is opened, nor made, before it
    /// came whole — one made meanwhile would be older than the hub's record
    /// of it, and what came after it would have no place to go.
    coming: Mutex<std::collections::HashSet<Owner>>,
    /// Owners whose library this process opened and settled whole: closed
    /// since, it is opened again as `Library::reopen_hub` does.
    settled: Mutex<HashSet<Owner>>,
    /// Where each library closed while nobody held it stood as it closed.
    stood: Mutex<HashMap<Owner, Latest>>,
    open_at_once: usize,
}

/// What a library keeps its hub's incarnation under.
const STAMP: &str = "hub";

/// How a library that has not come yet is refused.
pub(crate) const COMING: &str = "is still coming from where the hub was before it moved";

impl Owners {
    pub(crate) fn new(root: PathBuf, incarnation: Vec<u8>) -> Self {
        Self {
            root,
            incarnation,
            slots: Mutex::new(HashMap::new()),
            fenced: Mutex::new(std::collections::HashSet::new()),
            coming: Mutex::new(std::collections::HashSet::new()),
            settled: Mutex::new(HashSet::new()),
            stood: Mutex::new(HashMap::new()),
            open_at_once: open_at_once(),
        }
    }

    /// Where `owner`'s library stood as it was closed, while it is shut:
    /// nothing has changed in it since.
    pub(crate) fn latest(&self, owner: &Owner) -> Option<Latest> {
        self.stood.lock().expect("stood libraries").get(owner).cloned()
    }

    /// `owner`'s library is to be opened or made anew: what this process
    /// knew of it is forgotten.
    fn forget(&self, owner: &Owner) {
        self.settled.lock().expect("settled libraries").remove(owner);
        self.stood.lock().expect("stood libraries").remove(owner);
    }

    /// Closes the library of the slot `owner` held, nobody holding it:
    /// where it stood is kept first, under the lock every opening takes.
    fn close(&self, owner: &Owner, slot: &Slot) {
        let library = slot.library.lock().expect("owner library");
        match library.as_ref().map(Library::latest) {
            Some(Ok(Some(latest))) => {
                self.stood.lock().expect("stood libraries").insert(owner.clone(), latest);
            }
            _ => {
                self.stood.lock().expect("stood libraries").remove(owner);
            }
        }
    }

    /// The owner whose library is at `path` under the hub's directory
    /// (`assets/account/<name>`, `assets/body/<id>`).
    pub(crate) fn of_path(path: &str) -> Option<Owner> {
        let mut names = path.strip_prefix("assets/")?.split('/');
        let (kind, name) = (names.next()?, names.next()?);
        if names.next().is_some() {
            return None;
        }
        Owner::of_directory(kind, name)
    }

    /// The libraries at `paths` are still coming: none is opened until
    /// `came` says it came.
    pub(crate) fn hold_back<'a>(&self, paths: impl IntoIterator<Item = &'a String>) {
        let owners: Vec<Owner> = paths.into_iter().filter_map(|path| Self::of_path(path)).collect();
        for owner in &owners {
            self.forget(owner);
        }
        self.coming.lock().expect("coming owners").extend(owners);
    }

    /// The library at `path` came whole: it is opened from now on.
    pub(crate) fn came(&self, path: &str) -> Option<Owner> {
        let owner = Self::of_path(path)?;
        self.coming.lock().expect("coming owners").remove(&owner);
        Some(owner)
    }

    /// Whether `owner`'s library is still coming.
    pub(crate) fn is_coming(&self, owner: &Owner) -> bool {
        self.coming.lock().expect("coming owners").contains(owner)
    }

    /// Whether any library is still coming.
    pub(crate) fn any_coming(&self) -> bool {
        !self.coming.lock().expect("coming owners").is_empty()
    }

    /// The accounts and bodies whose libraries are on this hub's disk, not
    /// one being thrown away.
    pub(crate) fn on_disk(&self) -> io::Result<Vec<Owner>> {
        let mut found = Vec::new();
        for kind in ["account", "body"] {
            let entries = match fastctx_platform::disk::read_dir(self.root.join(kind)) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                // What is thrown away is set aside in the root, not here: a
                // name here is an owner's, whatever it begins with.
                let name = entry.file_name().to_string_lossy().into_owned();
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                found.extend(Owner::of_directory(kind, &name));
            }
        }
        Ok(found)
    }

    /// Whether `owner` has a library on this hub's disk: one asked after by
    /// a device is opened only then, never made.
    pub(crate) fn exists(&self, owner: &Owner) -> bool {
        // One still coming has a library: a device is not told there is
        // none before its directory came.
        self.is_coming(owner) || fastctx_platform::disk::is_dir(self.root.join(owner.directory()))
    }

    /// The library of `owner`, opened or made at need, or why it may not be
    /// served: it bears another hub's incarnation, or `check` finds it older
    /// than the hub's record of it. A library is checked once, as it opens.
    /// Call on a thread that may block.
    pub(crate) fn library(
        &self,
        owner: &Owner,
        check: impl FnOnce(&Library) -> io::Result<Option<String>>,
    ) -> io::Result<Result<Library, String>> {
        let slot = {
            let mut slots = self.slots.lock().expect("owner libraries");
            if self.fenced.lock().expect("fenced owners").contains(owner) {
                return Err(io::Error::other(format!("the library of {} is being deleted", owner.key())));
            }
            if self.is_coming(owner) {
                return Err(io::Error::other(format!("the library of {} {COMING}", owner.key())));
            }
            if !slots.contains_key(owner) && slots.len() >= self.open_at_once {
                // The least recently used that nobody holds goes: one still
                // held, or still opening, stays, lest the same directory be
                // opened twice. Past the bound for a while when all are held.
                let oldest = slots
                    .iter()
                    .filter(|(_, slot)| idle(slot))
                    .min_by_key(|(_, slot)| *slot.used.lock().expect("owner library use"))
                    .map(|(owner, _)| owner.clone());
                if let Some(oldest) = oldest
                    && let Some(slot) = slots.remove(&oldest)
                {
                    self.close(&oldest, &slot);
                }
            }
            slots
                .entry(owner.clone())
                .or_insert_with(|| Arc::new(Slot { library: Mutex::new(None), used: Mutex::new(Instant::now()) }))
                .clone()
        };
        *slot.used.lock().expect("owner library use") = Instant::now();
        let mut library = slot.library.lock().expect("owner library");
        if let Some(library) = library.as_ref() {
            return Ok(Ok(library.clone()));
        }
        // From here it may change: where it stood as it closed holds no more.
        self.stood.lock().expect("stood libraries").remove(owner);
        let directory = self.root.join(owner.directory());
        fastctx_platform::ensure_private_dir(&directory)?;
        let kinds: Arc<dyn fastctx_content::v2::Authorizer> = super::serve::kinds();
        let settled = self.settled.lock().expect("settled libraries").contains(owner);
        if settled {
            let (opened, salvaged) = match Library::reopen_hub(&directory) {
                Ok(reopened) => reopened,
                // A database salvaged before what failed may have lost rows:
                // the next opening checks it whole, as a first one does.
                Err(error) => {
                    self.settled.lock().expect("settled libraries").remove(owner);
                    return Err(io::Error::other(error.to_string()));
                }
            };
            if salvaged.is_none() {
                opened.know(kinds);
                *library = Some(opened.clone());
                return Ok(Ok(opened));
            }
            // A database salvaged as it opened may have lost rows: it is
            // checked again, as at its first opening.
            self.settled.lock().expect("settled libraries").remove(owner);
            return self.settle(owner, opened, kinds, check, &mut library);
        }
        let (opened, _) = Library::open_hub(&directory).map_err(|error| io::Error::other(error.to_string()))?;
        self.settle(owner, opened, kinds, check, &mut library)
    }

    /// A library opened for the first time in this process, or salvaged as
    /// it opened again: checked against the hub's incarnation and its record
    /// of it, and what came of kinds it did not know judged now.
    fn settle(
        &self,
        owner: &Owner,
        opened: Library,
        kinds: Arc<dyn fastctx_content::v2::Authorizer>,
        check: impl FnOnce(&Library) -> io::Result<Option<String>>,
        library: &mut Option<Library>,
    ) -> io::Result<Result<Library, String>> {
        let stamped =
            opened.stamp(STAMP, self.incarnation.clone()).map_err(|error| io::Error::other(error.to_string()))?;
        if stamped != self.incarnation {
            return Ok(Err(format!("the library of {} is another hub's", owner.key())));
        }
        if let Some(why) = check(&opened)? {
            return Ok(Err(why));
        }
        // Records of a kind this hub did not know when they came are judged
        // now that it may (§2.3), before anyone reads the library. The kinds
        // a running hub knows do not change, so once is enough.
        opened.know(kinds.clone());
        opened.reconsider(kinds, true).map_err(|error| io::Error::other(error.to_string()))?;
        self.settled.lock().expect("settled libraries").insert(owner.clone());
        *library = Some(opened.clone());
        Ok(Ok(opened))
    }

    /// Takes the library of `owner` away, closed first: its directory moved
    /// aside under a name no library has, where `discard` removes it, so
    /// what is quick happens now and the slow part later (AD13). An error
    /// while anyone holds it, opens it or is about to. Call on a thread
    /// that may block.
    pub(crate) fn delete(&self, owner: &Owner) -> io::Result<Option<PathBuf>> {
        {
            // Fenced first, under the lock every opening takes: none opens
            // it from now until `deleted`.
            let mut slots = self.slots.lock().expect("owner libraries");
            self.fenced.lock().expect("fenced owners").insert(owner.clone());
            if slots.get(owner).is_some_and(|slot| !idle(slot)) {
                return Err(io::Error::other(format!("the library of {} is in use", owner.key())));
            }
            slots.remove(owner);
            self.forget(owner);
        }
        let directory = self.root.join(owner.directory());
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        let aside = self.root.join(format!("{DISCARDED}{nanos:x}"));
        // The move lasts before the deletion is recorded done (AE06): no
        // power lost after that brings the library back where it was.
        match fastctx_platform::durable_file::rename_durably(&directory, &aside) {
            Ok(()) => Ok(Some(aside)),
            // Gone already: moved by a try that stopped before it made the
            // move last, perhaps. It is made to last now (AF03).
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fastctx_platform::durable_file::sync_directory(&self.root)?;
                // And the folder it was moved out of (AG01).
                if let Some(parent) = directory.parent().filter(|parent| *parent != self.root) {
                    match fastctx_platform::durable_file::sync_directory(parent) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                        _ => {}
                    }
                }
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Removes what deleted libraries left moved aside, those a stop left
    /// among them.
    pub(crate) fn discard(&self) -> io::Result<()> {
        let entries = match fastctx_platform::disk::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(DISCARDED) {
                match fastctx_platform::disk::remove_dir_all(entry.path()) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// The deletion of `owner`'s library is done: a library made for it from
    /// now is a new one.
    pub(crate) fn deleted(&self, owner: &Owner) {
        self.fenced.lock().expect("fenced owners").remove(owner);
    }

    /// The owners whose libraries are fenced off for deletion now.
    pub(crate) fn fenced(&self) -> Vec<Owner> {
        self.fenced.lock().expect("fenced owners").iter().cloned().collect()
    }

    /// Closes the libraries left idle past `IDLE` that nobody holds, and,
    /// past the bound, the least recently used that nobody holds down to it.
    pub(crate) fn sweep(&self) {
        let now = Instant::now();
        let mut slots = self.slots.lock().expect("owner libraries");
        let stale: Vec<Owner> = slots
            .iter()
            .filter(|(_, slot)| idle(slot) && now.duration_since(*slot.used.lock().expect("owner library use")) >= IDLE)
            .map(|(owner, _)| owner.clone())
            .collect();
        for owner in stale {
            if let Some(slot) = slots.remove(&owner) {
                self.close(&owner, &slot);
            }
        }
        while slots.len() > self.open_at_once {
            let oldest = slots
                .iter()
                .filter(|(_, slot)| idle(slot))
                .min_by_key(|(_, slot)| *slot.used.lock().expect("owner library use"))
                .map(|(owner, _)| owner.clone());
            let Some(oldest) = oldest else { break };
            if let Some(slot) = slots.remove(&oldest) {
                self.close(&oldest, &slot);
            }
        }
    }
}

/// Whether nobody holds this slot's library, is opening it, or is about to.
fn idle(slot: &Arc<Slot>) -> bool {
    if Arc::strong_count(slot) > 1 {
        return false;
    }
    match slot.library.try_lock() {
        Ok(library) => library.as_ref().is_none_or(|library| !library.shared()),
        Err(_) => false,
    }
}
