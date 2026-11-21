//! The hub's owner libraries (design §6.2): one per account, one per body
//! that several accounts own together, and one for the hub itself, each a
//! directory under `assets/`. At most 64 are open at once, the least
//! recently used that nobody holds closed first; one left idle ten minutes
//! is closed. A library is opened under its own slot, so a slow opening
//! holds up no other library, and is checked against the hub's record of
//! it as it opens.

use fastctx_content::v2::Library;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a deleted library's directory is moved aside under, before it goes.
const DISCARDED: &str = "discarded-";

/// Libraries open at once.
pub(crate) const OPEN_AT_ONCE: usize = 64;
/// How long an idle library stays open.
pub(crate) const IDLE: Duration = Duration::from_secs(600);

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
}

/// What a library keeps its hub's incarnation under.
const STAMP: &str = "hub";

impl Owners {
    pub(crate) fn new(root: PathBuf, incarnation: Vec<u8>) -> Self {
        Self { root, incarnation, slots: Mutex::new(HashMap::new()), fenced: Mutex::new(std::collections::HashSet::new()) }
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
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(DISCARDED) || !entry.file_type()?.is_dir() {
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
        fastctx_platform::disk::is_dir(self.root.join(owner.directory()))
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
            if !slots.contains_key(owner) && slots.len() >= OPEN_AT_ONCE {
                // The least recently used that nobody holds goes: one still
                // held, or still opening, stays, lest the same directory be
                // opened twice. Past the bound for a while when all are held.
                let oldest = slots
                    .iter()
                    .filter(|(_, slot)| idle(slot))
                    .min_by_key(|(_, slot)| *slot.used.lock().expect("owner library use"))
                    .map(|(owner, _)| owner.clone());
                if let Some(oldest) = oldest {
                    slots.remove(&oldest);
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
        let directory = self.root.join(owner.directory());
        fastctx_platform::ensure_private_dir(&directory)?;
        let (opened, _) = Library::open_hub(&directory).map_err(|error| io::Error::other(error.to_string()))?;
        let stamped = opened.stamp(STAMP, self.incarnation.clone()).map_err(|error| io::Error::other(error.to_string()))?;
        if stamped != self.incarnation {
            return Ok(Err(format!("the library of {} is another hub's", owner.key())));
        }
        if let Some(why) = check(&opened)? {
            return Ok(Err(why));
        }
        // Records of a kind this hub did not know when they came are judged
        // now that it may (§2.3), before anyone reads the library.
        let kinds: Arc<dyn fastctx_content::v2::Authorizer> = super::serve::kinds();
        opened.know(kinds.clone());
        opened.reconsider(kinds, true).map_err(|error| io::Error::other(error.to_string()))?;
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
        }
        let directory = self.root.join(owner.directory());
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
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
        slots.retain(|_, slot| !idle(slot) || now.duration_since(*slot.used.lock().expect("owner library use")) < IDLE);
        while slots.len() > OPEN_AT_ONCE {
            let oldest = slots
                .iter()
                .filter(|(_, slot)| idle(slot))
                .min_by_key(|(_, slot)| *slot.used.lock().expect("owner library use"))
                .map(|(owner, _)| owner.clone());
            let Some(oldest) = oldest else { break };
            slots.remove(&oldest);
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
