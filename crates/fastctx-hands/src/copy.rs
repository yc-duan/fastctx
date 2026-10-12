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
//! between the tries starts over.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};

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

/// Walks `root`, a file or a directory, hashing every file. A link given
/// as the root is followed; links inside it are not.
pub fn walk(root: &Path) -> io::Result<Walked> {
    let metadata = std::fs::metadata(root)?;
    let name = root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut walked = Walked {
        manifest: Manifest { directory: metadata.is_dir(), name, entries: Vec::new() },
        sources: Vec::new(),
        problems: Vec::new(),
    };
    if metadata.is_dir() {
        add(&mut walked, root, String::new(), &metadata);
        descend(&mut walked, root, "");
    } else if metadata.is_file() {
        add(&mut walked, root, String::new(), &metadata);
        if walked.manifest.entries.is_empty() {
            return Err(io::Error::other(walked.problems.join("; ")));
        }
    } else {
        return Err(io::Error::other(format!("{} is not a file or a directory", root.display())));
    }
    Ok(walked)
}

fn descend(walked: &mut Walked, directory: &Path, relative: &str) {
    let mut children: Vec<_> = match std::fs::read_dir(directory) {
        Ok(children) => children.filter_map(Result::ok).collect(),
        Err(error) => {
            walked.problems.push(format!("{}: {error}", directory.display()));
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
                add(walked, &path, relative.clone(), &metadata);
                if metadata.is_dir() {
                    descend(walked, &path, &relative);
                }
            }
            Err(error) => walked.problems.push(format!("{}: {error}", path.display())),
        }
    }
}

fn add(walked: &mut Walked, path: &Path, relative: String, metadata: &std::fs::Metadata) {
    let kind = if metadata.is_symlink() {
        match std::fs::read_link(path) {
            Ok(target) => EntryKind::Symlink { target: target.to_string_lossy().into_owned() },
            Err(error) => {
                walked.problems.push(format!("{}: {error}", path.display()));
                return;
            }
        }
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else if metadata.is_file() {
        match hash(path) {
            Ok((size, sha256)) => EntryKind::File { size, sha256 },
            Err(error) => {
                walked.problems.push(format!("{}: {error}", path.display()));
                return;
            }
        }
    } else {
        walked.problems.push(format!("{}: not a file, a directory or a link", path.display()));
        return;
    };
    walked.manifest.entries.push(Entry {
        path: relative,
        kind,
        mode: mode(metadata),
        modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
    });
    walked.sources.push(path.to_path_buf());
}

#[cfg(unix)]
fn mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    Some(metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

/// A file's size and the hex SHA-256 of its bytes.
fn hash(path: &Path) -> io::Result<(u64, String)> {
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
    Ok((size, format!("{:x}", hasher.finalize())))
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
            resumes: false,
            files: 0,
            bytes: 0,
            directories: 0,
            links: 0,
            unchanged: 0,
            problems: Vec::new(),
        })
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// Makes the directories and says which files to send: those not here
    /// already with the same contents. When `resumes`, a file an earlier try
    /// brought part of is asked for from where that part ends.
    pub fn wanted(&mut self, resumes: bool) -> Vec<Wanted> {
        self.resumes = resumes;
        let mut wanted = Vec::new();
        // Two names that differ only in case are one file on Windows.
        let mut names = BTreeSet::new();
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
                EntryKind::File { size, sha256 } => match hash(&target) {
                    Ok((existing, digest)) if existing == *size && digest == *sha256 => {
                        self.unchanged += 1;
                        let _ = std::fs::remove_file(partial(&target, sha256));
                    }
                    _ => wanted.push(Wanted { index, from: self.held(&target, *size, sha256) }),
                },
                EntryKind::Symlink { .. } => {}
            }
        }
        wanted
    }

    /// How many bytes of the file for `target` an earlier try left, when
    /// they can be gone on with; the parts other tries left for other
    /// contents of it are removed.
    fn held(&self, target: &Path, size: u64, sha256: &str) -> u64 {
        let ours = partial(target, sha256);
        let mine = |name: &str| name.starts_with(".fastctx-part-") && name.ends_with(&suffix(target));
        if let Some(parent) = target.parent()
            && let Ok(children) = std::fs::read_dir(parent)
        {
            for child in children.filter_map(Result::ok) {
                if mine(&child.file_name().to_string_lossy()) && child.path() != ours {
                    let _ = std::fs::remove_file(child.path());
                }
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
    pub fn arrive(&mut self, index: usize, from: u64) -> (u64, Arriving) {
        let entry = self.manifest.entries[index].clone();
        let EntryKind::File { size, sha256 } = entry.kind else { unreachable!("only files are wanted") };
        let target = self.target(&entry.path).expect("checked when wanted");
        let mut hasher = Sha256::new();
        let staged = target.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
            if !self.resumes {
                return Staged::create(&target, entry.mode);
            }
            let at = partial(&target, &sha256);
            let (staged, held) = Staged::resume(&target, at.clone(), entry.mode)?;
            if held != from {
                staged.discard();
                return Err(io::Error::other("what an earlier try left of it changed meanwhile"));
            }
            // The bytes already here count toward the file's hash.
            let mut kept = File::open(&at)?.take(from);
            let mut buffer = vec![0u8; 1 << 20];
            loop {
                let read = kept.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
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
    pub fn arrived(&mut self, index: usize, arriving: Arriving) {
        let entry = self.manifest.entries[index].clone();
        let EntryKind::File { size, sha256 } = &entry.kind else { unreachable!("only files are wanted") };
        let target = self.target(&entry.path).expect("checked when wanted");
        if let Some(failure) = arriving.failure {
            if let Some(staged) = arriving.staged {
                staged.discard();
            }
            self.problems.push(failure);
            return;
        }
        if format!("{:x}", arriving.hasher.finalize()) != *sha256 {
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
        self.files += 1;
        self.bytes += size;
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
        if relative.is_empty() {
            return Some(self.destination.clone());
        }
        let mut target = self.destination.clone();
        for name in relative.split('/') {
            if !plain(name) {
                self.problems.push(format!("{relative}: not a name this machine can give a file"));
                return None;
            }
            target.push(name);
        }
        Some(target)
    }
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
