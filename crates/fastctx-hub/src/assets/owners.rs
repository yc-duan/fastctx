//! The hub's owner libraries (design §6.2): one per account, one per body
//! that several accounts own together, and one for the hub itself, each a
//! directory under `assets/`. At most 64 are open at once, the least
//! recently used closed first; one left idle ten minutes is closed. A
//! library is opened under its own slot, so a slow opening holds up no
//! other library.

use fastctx_content::v2::Library;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Libraries open at once.
pub(crate) const OPEN_AT_ONCE: usize = 64;
/// How long an idle library stays open.
pub(crate) const IDLE: Duration = Duration::from_secs(600);

/// Whose library.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Owner {
    Account(String),
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
            Self::Account(name) => format!("account:{name}"),
            Self::Body(id) => format!("body:{id}"),
            Self::Hub => "hub".into(),
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key.split_once(':') {
            Some(("account", name)) => Some(Self::Account(name.into())),
            Some(("body", id)) => Some(Self::Body(id.into())),
            None if key == "hub" => Some(Self::Hub),
            _ => None,
        }
    }

    fn directory(&self) -> PathBuf {
        match self {
            Self::Account(name) => PathBuf::from("account").join(directory_name(name)),
            Self::Body(id) => PathBuf::from("body").join(directory_name(id)),
            Self::Hub => PathBuf::from("hub"),
        }
    }
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
}

/// What a library keeps its hub's incarnation under.
const STAMP: &str = "hub";

impl Owners {
    pub(crate) fn new(root: PathBuf, incarnation: Vec<u8>) -> Self {
        Self { root, incarnation, slots: Mutex::new(HashMap::new()) }
    }

    /// Why one of the libraries here is not this hub's, if one is: it bears
    /// another hub's incarnation. Call on a thread that may block.
    pub(crate) fn foreign(&self) -> io::Result<Option<String>> {
        for kind in ["account", "body", "hub"] {
            let base = self.root.join(kind);
            let directories: Vec<PathBuf> = if kind == "hub" {
                vec![base]
            } else {
                match fastctx_platform::disk::read_dir(&base) {
                    Ok(entries) => entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect(),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
                    Err(error) => return Err(error),
                }
            };
            for directory in directories {
                if !fastctx_platform::disk::exists(directory.join(fastctx_content::v2::FILE))? {
                    continue;
                }
                let (library, _) = Library::open_hub(&directory).map_err(|error| io::Error::other(error.to_string()))?;
                let stamped = library.stamped(STAMP).map_err(|error| io::Error::other(error.to_string()))?;
                if stamped.is_some_and(|stamped| stamped != self.incarnation) {
                    return Ok(Some(format!("the library in {} is another hub's", directory.display())));
                }
            }
        }
        Ok(None)
    }

    /// Whether `owner` has a library here.
    pub(crate) fn exists(&self, owner: &Owner) -> io::Result<bool> {
        fastctx_platform::disk::exists(self.root.join(owner.directory()).join(fastctx_content::v2::FILE))
    }

    /// The library of `owner`, opened or made at need. Call on a thread
    /// that may block.
    pub(crate) fn library(&self, owner: &Owner) -> io::Result<Library> {
        let slot = {
            let mut slots = self.slots.lock().expect("owner libraries");
            if !slots.contains_key(owner) && slots.len() >= OPEN_AT_ONCE {
                // The least recently used goes; whoever still holds its
                // library keeps it until they let go.
                let oldest = slots
                    .iter()
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
            return Ok(library.clone());
        }
        let directory = self.root.join(owner.directory());
        fastctx_platform::ensure_private_dir(&directory)?;
        let (opened, _) = Library::open_hub(&directory).map_err(|error| io::Error::other(error.to_string()))?;
        let stamped = opened.stamp(STAMP, self.incarnation.clone()).map_err(|error| io::Error::other(error.to_string()))?;
        if stamped != self.incarnation {
            return Err(io::Error::other(format!("the library of {} is another hub's", owner.key())));
        }
        *library = Some(opened.clone());
        Ok(opened)
    }

    /// Closes the libraries left idle past `IDLE`.
    pub(crate) fn sweep(&self) {
        let now = Instant::now();
        self.slots
            .lock()
            .expect("owner libraries")
            .retain(|_, slot| now.duration_since(*slot.used.lock().expect("owner library use")) < IDLE);
    }
}
