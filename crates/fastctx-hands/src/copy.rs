//! The files of a copy: what the source walks and reads, and how the
//! destination decides what it needs and puts each file in place
//! (`fastctx_types::copy`).
//!
//! The source walks its path once, hashing every file, and the manifest it
//! makes is what the destination checks every byte against. Links are
//! copied as links, never followed. The destination skips files it already
//! has with the same contents, so copying the same thing again sends only
//! what changed. Each file is staged beside its target and moved into place
//! whole once its bytes check out; a file that cannot be copied is named in
//! the report and the rest go on.
//!
//! Over a network a file's bytes are staged where a try that broke can find
//! them again (`world-network.md`, 断开不是结束): beside the target, under a
//! name made of the contents promised and the target's name, so the next try
//! asks only for the bytes after them, and a file whose contents changed
//! between the tries starts over. What the tries read of the files is kept
//! as well (`Known`), so the next one reads only what changed.

use std::collections::{BTreeSet, HashMap};
use std::fs::{File, Metadata};
use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

pub mod local;

use fastctx_platform::Staged;
use fastctx_types::copy::{Entry, EntryKind, Manifest};
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};

/// What the source found: the manifest, the file on disk behind each
/// entry, and what it could not read.
pub struct Walked {
    pub manifest: Manifest,
    pub sources: Vec<PathBuf>,
    pub problems: Vec<String>,
}

/// What copying on this machine learned of files' contents, so that a copy
/// going on after a break does not read again what it read before: the hash
/// of each file read whole, with the file's size and the time it was last
/// written then, and the hash so far of each file that arrived in part.
///
/// A file that still has that size and time is taken to hold the bytes it
/// held. Were it written again under the same time all the same, the check
/// of every byte that arrives against its hash turns it away and says so:
/// what is known spares reading, and never lets a wrong file be kept.
#[derive(Default)]
pub struct Known {
    files: HashMap<PathBuf, Read>,
    parts: HashMap<PathBuf, Part>,
}

/// A file as it was when it was read whole.
struct Read {
    size: u64,
    modified: SystemTime,
    sha256: [u8; 32],
}

/// What arrived of a file before a copy broke, as it was left.
struct Part {
    size: u64,
    modified: SystemTime,
    hasher: Sha256,
}

/// How coarsely a file system may keep the time a file was written: FAT
/// keeps it to two seconds. A file written that shortly before it was read
/// may be written again under the same time, so what was read of it is not
/// taken to last.
const COARSEST_TIME: Duration = Duration::from_secs(2);

impl Known {
    /// The size and hash of the file at `path`, whose metadata is `metadata`:
    /// as `was` knew it, while the file has that size and time still, or
    /// read now; learned either way.
    fn hash(&mut self, path: &Path, metadata: &Metadata, was: Option<Read>) -> io::Result<(u64, [u8; 32])> {
        if let Some(was) = was
            && was.size == metadata.len()
            && metadata.modified().is_ok_and(|modified| modified == was.modified)
        {
            let known = (was.size, was.sha256);
            self.files.insert(path.to_path_buf(), was);
            return Ok(known);
        }
        let reading = SystemTime::now();
        let (size, sha256) = digest(path)?;
        self.learn(path, size, sha256, reading);
        Ok((size, sha256))
    }

    /// Learns that the file at `path` held `size` bytes hashing to `sha256`
    /// at `then`, when it was last written well before.
    fn learn(&mut self, path: &Path, size: u64, sha256: [u8; 32], then: SystemTime) {
        let read = std::fs::metadata(path).ok().and_then(|now| {
            let modified = now.modified().ok()?;
            let settled = modified.checked_add(COARSEST_TIME).is_some_and(|after| after <= then);
            (now.len() == size && settled).then_some(Read { size, modified, sha256 })
        });
        match read {
            Some(read) => self.files.insert(path.to_path_buf(), read),
            None => self.files.remove(path),
        };
    }
}

/// Walks `root`, a file or a directory, hashing every file. A link given
/// as the root is followed; links inside it are not.
pub fn walk(root: &Path) -> io::Result<Walked> {
    walk_knowing(root, &mut Known::default())
}

/// Walks `root` as `walk` does, reading only the files that are not as
/// `known` knows them, and learning what it reads.
pub fn walk_knowing(root: &Path, known: &mut Known) -> io::Result<Walked> {
    let metadata = std::fs::metadata(root)?;
    let name = root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut walk = Walk {
        walked: Walked {
            manifest: Manifest { directory: metadata.is_dir(), name, entries: Vec::new() },
            sources: Vec::new(),
            problems: Vec::new(),
        },
        // What is no longer there is forgotten: the walk keeps what it finds.
        was: std::mem::take(&mut known.files),
        known,
    };
    if metadata.is_dir() {
        walk.add(root, String::new(), &metadata);
        walk.descend(root, "");
    } else if metadata.is_file() {
        walk.add(root, String::new(), &metadata);
        if walk.walked.manifest.entries.is_empty() {
            return Err(io::Error::other(walk.walked.problems.join("; ")));
        }
    } else {
        return Err(io::Error::other(format!("{} is not a file or a directory", root.display())));
    }
    Ok(walk.walked)
}

/// One walk under way.
struct Walk<'a> {
    walked: Walked,
    /// What was known before the walk, of files it has not come to yet.
    was: HashMap<PathBuf, Read>,
    known: &'a mut Known,
}

impl Walk<'_> {
    fn descend(&mut self, directory: &Path, relative: &str) {
        let mut children: Vec<_> = match std::fs::read_dir(directory) {
            Ok(children) => children.filter_map(Result::ok).collect(),
            Err(error) => {
                self.walked.problems.push(format!("{}: {error}", directory.display()));
                return;
            }
        };
        children.sort_by_key(|child| child.file_name());
        for child in children {
            let path = child.path();
            let name = child.file_name().to_string_lossy().into_owned();
            let relative = if relative.is_empty() { name } else { format!("{relative}/{name}") };
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    self.add(&path, relative.clone(), &metadata);
                    if metadata.is_dir() {
                        self.descend(&path, &relative);
                    }
                }
                Err(error) => self.walked.problems.push(format!("{}: {error}", path.display())),
            }
        }
    }

    fn add(&mut self, path: &Path, relative: String, metadata: &Metadata) {
        let kind = if metadata.is_symlink() {
            match std::fs::read_link(path) {
                Ok(target) => EntryKind::Symlink { target: target.to_string_lossy().into_owned() },
                Err(error) => {
                    self.walked.problems.push(format!("{}: {error}", path.display()));
                    return;
                }
            }
        } else if metadata.is_dir() {
            EntryKind::Directory
        } else if metadata.is_file() {
            let was = self.was.remove(path);
            match self.known.hash(path, metadata, was) {
                Ok((size, sha256)) => EntryKind::File { size, sha256: hex(&sha256) },
                Err(error) => {
                    self.walked.problems.push(format!("{}: {error}", path.display()));
                    return;
                }
            }
        } else {
            self.walked.problems.push(format!("{}: not a file, a directory or a link", path.display()));
            return;
        };
        self.walked.manifest.entries.push(Entry {
            path: relative,
            kind,
            mode: mode(metadata),
            modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
        });
        self.walked.sources.push(path.to_path_buf());
    }
}

#[cfg(unix)]
fn mode(metadata: &Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    Some(metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode(_metadata: &Metadata) -> Option<u32> {
    None
}

/// A file's size and the SHA-256 of its bytes.
fn digest(path: &Path) -> io::Result<(u64, [u8; 32])> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hasher.finalize().into()))
}

/// A hash as a manifest writes it.
fn hex(sha256: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    sha256.iter().fold(String::with_capacity(64), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// A file the destination needs: which entry, and from which byte, where
/// an earlier try brought the ones before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub index: usize,
    pub from: u64,
}

/// The destination of a copy, deciding what it needs and putting it in
/// place.
pub struct Receiver {
    destination: PathBuf,
    manifest: Manifest,
    world: Option<PathBuf>,
    /// Whether files are staged to go on with after a break.
    resumes: bool,
    pub files: u64,
    pub bytes: u64,
    pub directories: u64,
    pub links: u64,
    pub unchanged: u64,
    pub problems: Vec<String>,
}

/// One file arriving.
pub struct Arriving {
    staged: Option<Staged>,
    hasher: Sha256,
    failure: Option<String>,
}

impl Arriving {
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.hasher.update(bytes);
        if let Some(staged) = &mut self.staged
            && let Err(error) = staged.write_all(bytes)
        {
            // The rest of its bytes are still taken, to stay in step.
            self.staged = None;
            self.failure = Some(error.to_string());
        }
        Ok(())
    }
}

impl Receiver {
    /// The destination for `manifest` at `to`: `to` itself, or when `to`
    /// ends in a separator, the source's name inside it.
    pub fn new(to: &str, manifest: Manifest) -> Result<Self, String> {
        let into = to.ends_with('/') || (cfg!(windows) && to.ends_with('\\'));
        let base = fastctx_platform::local_path(to).map_err(|error| error.to_string())?;
        if !base.is_absolute() {
            return Err(format!("{to} is not an absolute path"));
        }
        let destination = if into {
            if manifest.name.is_empty() || !plain(&manifest.name) {
                return Err(format!("the source has no name to give its copy in {to}"));
            }
            base.join(&manifest.name)
        } else {
            base
        };
        match std::fs::symlink_metadata(&destination) {
            Ok(existing) if existing.is_dir() && !manifest.directory => {
                return Err(format!(
                    "{} is a directory; name the file to create, or end the path with / to copy into it",
                    destination.display()
                ));
            }
            Ok(existing) if !existing.is_dir() && manifest.directory => {
                return Err(format!("{} is not a directory, and the copy is of a directory", destination.display()));
            }
            _ => {}
        }
        Ok(Self {
            destination,
            manifest,
            world: None,
            resumes: false,
            files: 0,
            bytes: 0,
            directories: 0,
            links: 0,
            unchanged: 0,
            problems: Vec::new(),
        })
    }

    /// Checks every destination before making any directory or temporary file.
    pub fn protect(mut self, world: &Path) -> Result<Self, String> {
        self.world = Some(world.to_path_buf());
        for entry in self.manifest.entries.clone() {
            if self.target(&entry.path).is_none() {
                return Err(self.problems.join("; "));
            }
        }
        Ok(self)
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// Makes the directories and says which files to send: those not here
    /// already with the same contents. When `resumes`, a file an earlier try
    /// brought part of is asked for from where that part ends.
    pub fn wanted(&mut self, resumes: bool, known: &mut Known) -> Vec<Wanted> {
        self.resumes = resumes;
        let mut wanted = Vec::new();
        // Two names that differ only in case are one file on Windows.
        let mut names = BTreeSet::new();
        // The parts earlier tries left, listed once for each directory.
        let mut parts = HashMap::new();
        for (index, entry) in self.manifest.entries.clone().iter().enumerate() {
            let Some(target) = self.target(&entry.path) else { continue };
            if cfg!(windows) && !names.insert(entry.path.to_lowercase()) {
                self.problems.push(format!(
                    "{}: only its case differs from another name, which this machine does not tell apart",
                    entry.path
                ));
                continue;
            }
            match &entry.kind {
                EntryKind::Directory => match std::fs::create_dir_all(&target) {
                    Ok(()) => self.directories += 1,
                    Err(error) => self.problems.push(format!("{}: {error}", target.display())),
                },
                EntryKind::File { size, sha256 } => {
                    if holds(&target, *size, sha256, known) {
                        self.unchanged += 1;
                        let _ = std::fs::remove_file(partial(&target, sha256));
                    } else {
                        let beside = parts_beside(&mut parts, &target);
                        wanted.push(Wanted { index, from: self.held(&target, *size, sha256, beside) });
                    }
                }
                EntryKind::Symlink { .. } => {}
            }
        }
        wanted
    }

    /// How many bytes of the file for `target` an earlier try left, when
    /// they can be gone on with; the parts other tries left for other
    /// contents of it, among those `beside` it, are removed.
    fn held(&self, target: &Path, size: u64, sha256: &str, beside: &[PathBuf]) -> u64 {
        let ours = partial(target, sha256);
        let mine = suffix(target);
        for part in beside {
            if *part != ours && part.file_name().is_some_and(|name| name.to_string_lossy().ends_with(&mine)) {
                let _ = std::fs::remove_file(part);
            }
        }
        match std::fs::metadata(&ours) {
            Ok(metadata) if self.resumes && metadata.is_file() && metadata.len() <= size => metadata.len(),
            Ok(_) => {
                let _ = std::fs::remove_file(&ours);
                0
            }
            Err(_) => 0,
        }
    }

    /// How many bytes of the file at `index` are still to come, from
    /// `from`, and where they go.
    pub fn arrive(&mut self, index: usize, from: u64, known: &mut Known) -> (u64, Arriving) {
        let entry = self.manifest.entries[index].clone();
        let EntryKind::File { size, sha256 } = entry.kind else { unreachable!("only files are wanted") };
        let Some(target) = self.target(&entry.path) else {
            return (
                size.saturating_sub(from),
                Arriving {
                    staged: None,
                    hasher: Sha256::new(),
                    failure: Some("copy destination is now protected".into()),
                },
            );
        };
        let mut hasher = Sha256::new();
        let staged = target.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
            if !self.resumes {
                return Staged::create(&target, entry.mode);
            }
            let at = partial(&target, &sha256);
            // What the try that left the part had hashed of it, while the
            // part is as it was left.
            let left = known.parts.remove(&at).filter(|part| {
                std::fs::metadata(&at).is_ok_and(|now| {
                    now.len() == part.size && now.modified().is_ok_and(|modified| modified == part.modified)
                })
            });
            let (staged, held) = Staged::resume(&target, at.clone(), entry.mode)?;
            if held != from {
                staged.discard();
                return Err(io::Error::other("what an earlier try left of it changed meanwhile"));
            }
            // The bytes already here count toward the file's hash.
            match left.filter(|part| part.size == held) {
                Some(part) => hasher = part.hasher,
                None => {
                    let mut kept = File::open(&at)?.take(from);
                    let mut buffer = vec![0u8; 1 << 20];
                    loop {
                        let read = kept.read(&mut buffer)?;
                        if read == 0 {
                            break;
                        }
                        hasher.update(&buffer[..read]);
                    }
                }
            }
            Ok(staged)
        });
        let (staged, failure) = match staged.map_err(|error| format!("{}: {error}", target.display())) {
            Ok(staged) => (Some(staged), None),
            Err(error) => (None, Some(error)),
        };
        (size.saturating_sub(from), Arriving { staged, hasher, failure })
    }

    /// Puts an arrived file in place if its bytes are the ones promised.
    pub fn arrived(&mut self, index: usize, arriving: Arriving, known: &mut Known) {
        let entry = self.manifest.entries[index].clone();
        let EntryKind::File { size, sha256 } = &entry.kind else { unreachable!("only files are wanted") };
        let Some(target) = self.target(&entry.path) else {
            if let Some(staged) = arriving.staged {
                staged.discard();
            }
            return;
        };
        if let Some(failure) = arriving.failure {
            if let Some(staged) = arriving.staged {
                staged.discard();
            }
            self.problems.push(failure);
            return;
        }
        let digest: [u8; 32] = arriving.hasher.finalize().into();
        if hex(&digest) != *sha256 {
            if let Some(staged) = arriving.staged {
                staged.discard();
            }
            self.problems
                .push(format!("{}: its bytes changed while it was copied, or on the way; not kept", entry.path));
            return;
        }
        let staged = arriving.staged.expect("present without a failure");
        if let Err(error) = staged.commit() {
            self.problems.push(format!("{}: {error}", target.display()));
            return;
        }
        if let Some(modified) = entry.modified
            && let Ok(file) = File::options().write(true).open(&target)
        {
            let _ = file.set_modified(modified.into());
        }
        known.learn(&target, *size, digest, SystemTime::now());
        self.files += 1;
        self.bytes += size;
    }

    /// Keeps what arrived of the file at `index` before the copy broke, with
    /// its hash so far, for the try that goes on with it.
    pub fn broke(&mut self, index: usize, arriving: Arriving, known: &mut Known) {
        let Arriving { staged, hasher, failure } = arriving;
        let (Some(staged), None, true) = (staged, failure, self.resumes) else { return };
        // Closed, it is kept as it stands.
        drop(staged);
        let entry = self.manifest.entries[index].clone();
        let EntryKind::File { sha256, .. } = &entry.kind else { unreachable!("only files are wanted") };
        let Some(target) = self.target(&entry.path) else {
            return;
        };
        let at = partial(&target, sha256);
        if let Ok(now) = std::fs::metadata(&at)
            && let Ok(modified) = now.modified()
        {
            known.parts.insert(at, Part { size: now.len(), modified, hasher });
        }
    }

    /// Makes the links, once everything they may point to is in place.
    pub fn link(&mut self) {
        for entry in self.manifest.entries.clone() {
            let EntryKind::Symlink { target: points_to } = &entry.kind else { continue };
            let Some(target) = self.target(&entry.path) else { continue };
            if std::fs::symlink_metadata(&target).is_ok() && std::fs::remove_file(&target).is_err() {
                self.problems.push(format!("{}: a link, and something that is not one is there", target.display()));
                continue;
            }
            match make_link(points_to, &target) {
                Ok(()) => self.links += 1,
                Err(error) => {
                    self.problems.push(format!("{}: a link to {points_to}, not made: {error}", target.display()))
                }
            }
        }
    }

    /// Where the entry at `relative` goes; `None`, with the reason noted,
    /// when the name cannot be one here.
    fn target(&mut self, relative: &str) -> Option<PathBuf> {
        let mut target = self.destination.clone();
        for name in relative.split('/').filter(|_| !relative.is_empty()) {
            if !plain(name) {
                self.problems.push(format!("{relative}: not a name this machine can give a file"));
                return None;
            }
            target.push(name);
        }
        if let Some(world) = &self.world
            && let Err(reason) = crate::write::check_path_policy(world, &target)
        {
            self.problems.push(reason);
            return None;
        }
        Some(target)
    }
}

/// Whether the file at `target` is there with `size` bytes hashing to
/// `sha256`.
fn holds(target: &Path, size: u64, sha256: &str, known: &mut Known) -> bool {
    let Ok(metadata) = std::fs::metadata(target) else { return false };
    if !metadata.is_file() || metadata.len() != size {
        return false;
    }
    let was = known.files.remove(target);
    known.hash(target, &metadata, was).is_ok_and(|(_, digest)| hex(&digest) == sha256)
}

/// The parts earlier tries left in the directory of `target`, listed the
/// first time a file of that directory is asked about.
fn parts_beside<'a>(listed: &'a mut HashMap<PathBuf, Vec<PathBuf>>, target: &Path) -> &'a [PathBuf] {
    let Some(parent) = target.parent() else { return &[] };
    listed.entry(parent.to_path_buf()).or_insert_with(|| {
        std::fs::read_dir(parent)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|child| child.file_name().to_string_lossy().starts_with(".fastctx-part-"))
            .map(|child| child.path())
            .collect()
    })
}

/// Where the bytes of `target`'s file with the contents `sha256` are
/// staged, to be gone on with after a break: beside it, hidden, named by
/// the contents and by the target's own name.
fn partial(target: &Path, sha256: &str) -> PathBuf {
    target.with_file_name(format!(".fastctx-part-{}{}", &sha256[..sha256.len().min(16)], suffix(target)))
}

/// The end of the names of the parts staged for `target`, whatever their
/// contents.
fn suffix(target: &Path) -> String {
    let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    format!("-{}", &format!("{:x}", Sha256::digest(name.as_bytes()))[..8])
}

/// One name of a path, with nothing that leads elsewhere: not empty, not
/// `.` or `..`, and no separator, colon or NUL; on Windows, nothing a
/// Windows file name cannot be.
fn plain(name: &str) -> bool {
    let mut components = Path::new(name).components();
    let single = matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    let forbidden: &[char] =
        if cfg!(windows) { &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'] } else { &['/', '\0'] };
    let reserved = cfg!(windows) && {
        let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit())
            || name.ends_with('.')
            || name.ends_with(' ')
    };
    single && !name.contains(forbidden) && !reserved
}

#[cfg(unix)]
fn make_link(points_to: &str, at: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(points_to, at)
}

#[cfg(windows)]
fn make_link(points_to: &str, at: &Path) -> io::Result<()> {
    // Windows links are made as file or directory links; what the link
    // points to, resolved beside it, says which.
    let resolved = at.parent().map(|parent| parent.join(points_to)).unwrap_or_else(|| PathBuf::from(points_to));
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(points_to, at)
    } else {
        std::os::windows::fs::symlink_file(points_to, at)
    }
}
