//! The records a hub directory holds, as a move copies them: every file and
//! directory but the hub's identity and what belongs to the one machine it
//! runs on (`local`), each file by its size and SHA-256.
//!
//! Both ends list what they hold the same way, so the old hub sends only
//! what differs, and the new place takes over only when the digest of what
//! it holds is the digest of what the old hub held when it stopped.
//!
//! They are listed and sent in parts (`Part`; the user, 2026-11-22:
//! "只传核心", "停写期间只传核心那部分", "我要求，不传这10G"). The hub stops
//! only for its core: its own records and its own library, a few hundred
//! megabytes of a hub of twenty gigabytes. The owner libraries are the hub's
//! copies of what the machines made, written all the while: a copy of one
//! taken file by file is never the one held a moment later, so with them
//! among what the hub stopped for it stopped for half an hour to send them
//! again whole, and the new place still did not hold what was sent. Nothing
//! of them goes before the switch. Each library's databases go once the old
//! hub serves no more and nothing writes them (`super::old`), and the new
//! hub opens a library when they came; the files of its large objects are
//! of no listing, and follow the databases, those something still refers to
//! (`super::libraries`).
//!
//! A database goes as a closed copy made of it (`super::snapshot`), never
//! as the file a connection is writing, and its log and shared memory beside
//! it are of no listing: they change as it is read.

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use fastctx_net::sign::Digest;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

/// Where the owner libraries are under the hub's directory, the accounts'
/// and the bodies', one directory each.
pub(crate) const LIBRARIES: [&str; 2] = ["assets/account", "assets/body"];

/// Which of a hub's records a listing takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Part {
    /// What the hub cannot serve without: its own records and its own
    /// library.
    Core,
    /// One owner library, by its path under the hub's directory: its
    /// databases.
    Library(String),
}

/// One file or directory of the records, by its path under the hub's
/// directory, with `/` between names.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub path: String,
    /// `None` for a directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Sum>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Sum {
    pub bytes: u64,
    pub sha256: Digest,
}

/// Whether the top-level name `name` is the hub's own on the machine it
/// runs on, never copied nor replaced by a move: its identity and settings
/// (`hub.json`, `tls`, `login.secret`), which the new place makes from what
/// it is given, the records of the move itself, its log and how it was
/// doing, the releases put there for this machine, and the letters set
/// aside to be deleted.
pub(crate) fn local(name: &str) -> bool {
    matches!(
        name,
        "hub.json"
            | crate::store::shelf::WAL
            | crate::store::shelf::SHM
            | crate::store::shelf::JOURNAL
            | "tls"
            | "login.secret"
            | "releases"
            | crate::health::FILE
            | crate::store::TRASH
            | super::ARRIVING
            | super::OUTGOING
            | super::RETIRED
            | super::COMING
    ) || name.starts_with("hub.log")
}

/// Whether the file named `name` is a database's log or shared memory: of
/// no listing, wherever it is.
fn beside_a_database(name: &str) -> bool {
    name.ends_with("-wal") || name.ends_with("-shm")
}

/// Whether the file at `path` is a database, sent as a closed copy of it.
pub(crate) fn database(path: &str) -> bool {
    path.ends_with(".sqlite3")
}

/// Whether `path` is an owner library's: `assets/account/<name>` or
/// `assets/body/<id>`, and nothing above or below.
pub(crate) fn library(path: &str) -> bool {
    LIBRARIES.iter().any(|root| {
        path.strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|name| !name.is_empty() && !name.contains('/') && !name.starts_with('.'))
    })
}

/// The owner library `path` lies in, if it lies in one.
pub(crate) fn library_of(path: &str) -> Option<&str> {
    LIBRARIES.iter().find_map(|root| {
        let rest = path.strip_prefix(root)?.strip_prefix('/')?;
        let name = rest.split('/').next().filter(|name| !name.is_empty())?;
        Some(&path[..root.len() + 1 + name.len()])
    })
}

/// The owner libraries under `dir`, by their paths, sorted.
pub(crate) fn libraries(dir: &Path) -> io::Result<Vec<String>> {
    let mut found = Vec::new();
    for root in LIBRARIES {
        let entries = match fastctx_platform::disk::read_dir(dir.join(root)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with('.') && entry.file_type()?.is_dir() {
                found.push(format!("{root}/{name}"));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The records of `part` under `dir` as they are on the disk, sorted by
/// path. A name starting with a dot is a write in progress, never a record.
pub(crate) fn manifest(dir: &Path, part: &Part) -> io::Result<Vec<Entry>> {
    list(dir, part, false)
}

/// Everything of a hub's records under `dir` as it is on the disk: its core
/// and every owner library.
pub(crate) fn everything(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = manifest(dir, &Part::Core)?;
    for root in LIBRARIES {
        if fastctx_platform::disk::is_there(dir.join(root)) {
            entries.push(Entry { path: root.to_string(), file: None });
        }
    }
    for library in libraries(dir)? {
        entries.extend(manifest(dir, &Part::Library(library))?);
    }
    entries.sort();
    entries.dedup();
    Ok(entries)
}

/// The records of `part` under `dir`, each database as a closed copy just
/// made of it: what the old hub sends.
pub(super) fn snapshot(dir: &Path, part: &Part) -> io::Result<Vec<Entry>> {
    list(dir, part, true)
}

fn list(dir: &Path, part: &Part, copies: bool) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let walking = Walking { dir, part, copies };
    match part {
        Part::Core => walking.walk(dir, "", &mut entries)?,
        Part::Library(library) => {
            if !self::library(library) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{library:?} is no owner library's path"),
                ));
            }
            if fastctx_platform::disk::is_there(dir.join(library)) {
                entries.push(Entry { path: library.clone(), file: None });
                walking.walk(&dir.join(library), library, &mut entries)?;
            }
        }
    }
    entries.sort();
    entries.dedup();
    Ok(entries)
}

struct Walking<'a> {
    dir: &'a Path,
    part: &'a Part,
    copies: bool,
}

impl Walking<'_> {
    fn walk(&self, at: &Path, prefix: &str, entries: &mut Vec<Entry>) -> io::Result<()> {
        for found in fastctx_platform::disk::read_dir(at)? {
            let found = found?;
            let name = found.file_name();
            let name = name.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} has a name that is not UTF-8", found.path().display()),
                )
            })?;
            if name.starts_with('.') || (prefix.is_empty() && local(name)) {
                continue;
            }
            let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
            let kind = found.file_type()?;
            let of_library = matches!(self.part, Part::Library(_));
            if kind.is_dir() {
                // The owner libraries are no part of the core; of a library,
                // what is below its directory is its objects' files.
                if of_library || LIBRARIES.contains(&path.as_str()) {
                    continue;
                }
                entries.push(Entry { path: path.clone(), file: None });
                self.walk(&found.path(), &path, entries)?;
            } else if kind.is_file() {
                if beside_a_database(name) || (of_library && !database(&path)) {
                    continue;
                }
                let file = if self.copies && database(&path) {
                    super::snapshot::capture(self.dir, &path, of_library)?;
                    sum(&super::snapshot::path(self.dir, &path))?
                } else {
                    sum(&found.path())?
                };
                entries.push(Entry { path, file: Some(file) });
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is neither a file nor a directory", found.path().display()),
                ));
            }
        }
        Ok(())
    }
}

/// A file of this length or longer is summed once for as long as it stays
/// the file it was (`SUMS`).
const REMEMBERED: u64 = 1 << 20;

/// How long a file must have been left alone before its sum is remembered:
/// one written again within what its disk tells times apart by would look
/// the same file.
const SETTLED: std::time::Duration = std::time::Duration::from_secs(3);

/// What tells a file from another written in its place: how long it is,
/// when it was last written, and which file of its disk it is.
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    bytes: u64,
    modified: std::time::SystemTime,
    file: u64,
}

impl Stamp {
    fn of(found: &std::fs::Metadata) -> io::Result<Self> {
        #[cfg(unix)]
        let file = std::os::unix::fs::MetadataExt::ino(found);
        #[cfg(not(unix))]
        let file = 0;
        Ok(Self { bytes: found.len(), modified: found.modified()?, file })
    }
}

/// The sums of the long files read whole, each for the file it was then. A
/// move lists what both ends hold several times over, the last of them with
/// the hub stopped; the releases a hub keeps for its machines are most of
/// its core and never change, and read again at each listing they were read
/// four times while every member waited.
static SUMS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<PathBuf, (Stamp, Sum)>>> =
    std::sync::LazyLock::new(Default::default);

fn sum(path: &Path) -> io::Result<Sum> {
    let began = std::time::SystemTime::now();
    let mut file = fastctx_platform::disk::open(path)?;
    let stamp = Stamp::of(&file.metadata()?)?;
    if stamp.bytes >= REMEMBERED
        && let Some((kept, sum)) = SUMS.lock().expect("sums").get(path)
        && *kept == stamp
    {
        return Ok(*sum);
    }
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut bytes = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes += read as u64;
    }
    let sum = Sum { bytes, sha256: Digest(hasher.finalize().into()) };
    // Remembered when it was not written while it was read, nor so lately
    // that a later writing might bear the same time.
    if stamp.bytes >= REMEMBERED
        && bytes == stamp.bytes
        && Stamp::of(&file.metadata()?)? == stamp
        && began.duration_since(stamp.modified).is_ok_and(|since| since >= SETTLED)
    {
        SUMS.lock().expect("sums").insert(path.to_path_buf(), (stamp, sum));
    }
    Ok(sum)
}

/// The digest of a whole listing, as both ends compute it.
pub(crate) fn digest(entries: &[Entry]) -> Digest {
    let mut hasher = sha2::Sha256::new();
    for entry in entries {
        hasher.update(entry.path.as_bytes());
        hasher.update([0]);
        match &entry.file {
            Some(sum) => {
                hasher.update(sum.bytes.to_le_bytes());
                hasher.update(sum.sha256.0);
            }
            None => hasher.update(b"/"),
        }
        hasher.update([b'\n']);
    }
    Digest(hasher.finalize().into())
}

/// Where the record at `path`, as the other end wrote it, is under `dir`;
/// refused when it is not a record's path.
pub(crate) fn place(dir: &Path, path: &str) -> Result<PathBuf, String> {
    let mut place = dir.to_path_buf();
    let mut last = "";
    for (index, name) in path.split('/').enumerate() {
        let odd = name.is_empty()
            || name == "."
            || name == ".."
            || name.starts_with('.')
            || name.contains(['\\', ':', '\0'])
            || (index == 0 && local(name));
        if odd {
            return Err(format!("{path:?} is not the path of a hub's record"));
        }
        place.push(name);
        last = name;
    }
    if beside_a_database(last) {
        return Err(format!("{path:?} is not the path of a hub's record"));
    }
    Ok(place)
}
