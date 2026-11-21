//! A login's directory, copied whole to its staging directory with what a
//! filesystem keeps of each entry — kind, owner, mode, times, extended
//! attributes (POSIX ACLs among them), hard links — and checked entry by
//! entry against the original, content by digest. What is copied is listed
//! in a manifest, by which the original set aside is checked again before it
//! is deleted.

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};

/// The name of the file in the staging directory that says which move made
/// it; taken out once the move is done.
pub const MARKER: &str = ".fastctx-login-move";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File,
    Link(PathBuf),
    /// A socket, a pipe or a device: not copied.
    Other,
}

/// One entry of a directory as found, by its path inside it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub mtime: (i64, i64),
    pub ctime: (i64, i64),
    pub size: u64,
    pub device: u64,
    pub inode: u64,
    pub links: u64,
}

impl Entry {
    fn of(root: &Path, path: PathBuf) -> io::Result<Self> {
        let full = root.join(&path);
        let metadata = fs::symlink_metadata(&full)?;
        let kind = if metadata.file_type().is_symlink() {
            Kind::Link(fs::read_link(&full)?)
        } else if metadata.is_dir() {
            Kind::Directory
        } else if metadata.is_file() {
            Kind::File
        } else {
            Kind::Other
        };
        Ok(Self {
            path,
            kind,
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode() & 0o7777,
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
            size: metadata.size(),
            device: metadata.dev(),
            inode: metadata.ino(),
            links: metadata.nlink(),
        })
    }

    /// What stands for the entry staying as it was: changed in any way —
    /// written, its mode, owner or attributes set — it differs.
    fn stamp(&self) -> (u64, u64, (i64, i64), (i64, i64)) {
        (self.inode, self.size, self.mtime, self.ctime)
    }
}

/// Every entry under `root`, `root` itself first as the empty path, each
/// directory before what it holds, in the order of their names. Links are
/// not followed.
pub fn walk(root: &Path) -> io::Result<Vec<Entry>> {
    let mut found = vec![Entry::of(root, PathBuf::new())?];
    let mut index = 0;
    while index < found.len() {
        if found[index].kind == Kind::Directory {
            let directory = found[index].path.clone();
            let mut names: Vec<OsString> =
                fs::read_dir(root.join(&directory))?.map(|entry| entry.map(|entry| entry.file_name())).collect::<io::Result<_>>()?;
            names.sort();
            let mut inner = Vec::with_capacity(names.len());
            for name in names {
                inner.push(Entry::of(root, directory.join(name))?);
            }
            found.splice(index + 1..index + 1, inner);
        }
        index += 1;
    }
    Ok(found)
}

/// What a walk found that a copy does not take as it is: entries not
/// copied, and links leading out of the directory by a relative path, which
/// are written as the absolute path they led to; entries of another owner
/// than a caller who is not the superuser can give a copy, entries on
/// another filesystem than the directory, and relative links out of it whose
/// place cannot be told for sure.
pub struct Findings {
    pub bytes: u64,
    pub others: Vec<PathBuf>,
    pub outward: Vec<(PathBuf, PathBuf)>,
    pub foreign: Vec<PathBuf>,
    pub mounted: Vec<PathBuf>,
    pub unsure: Vec<PathBuf>,
}

/// Looks at `entries` of `root` before anything is copied: what it holds,
/// and, for a caller who is not the superuser, entries it could not give the
/// copy as they are owned.
pub fn findings(root: &Path, entries: &[Entry], superuser: bool, uid: u32) -> Findings {
    let mut findings =
        Findings { bytes: 0, others: Vec::new(), outward: Vec::new(), foreign: Vec::new(), mounted: Vec::new(), unsure: Vec::new() };
    let device = entries.first().map(|first| first.device);
    for entry in entries {
        if Some(entry.device) != device {
            findings.mounted.push(entry.path.clone());
        }
        match &entry.kind {
            Kind::File => findings.bytes += entry.size,
            Kind::Other => findings.others.push(entry.path.clone()),
            Kind::Link(target) => match outward(root, &entry.path, target) {
                Ok(Some(absolute)) => findings.outward.push((entry.path.clone(), absolute)),
                Ok(None) => {}
                Err(()) => findings.unsure.push(entry.path.clone()),
            },
            Kind::Directory => {}
        }
        if !superuser && entry.uid != uid && entry.kind != Kind::Other {
            findings.foreign.push(entry.path.clone());
        }
    }
    findings
}

/// The absolute path a relative link at `path` in `root` leads to, where it
/// leads out of `root`, as the system resolves it; none for one that stays
/// in it or is absolute. One that leads nowhere is told by its words alone
/// where they cannot mean another place — only `..` before any name — and
/// is unsure (`Err`) otherwise.
fn outward(root: &Path, path: &Path, target: &Path) -> Result<Option<PathBuf>, ()> {
    if target.is_absolute() {
        return Ok(None);
    }
    // The link's own directory: a directory of `root`, reached through none.
    let base = root.join(path.parent().unwrap_or(Path::new("")));
    if let Ok(led) = fs::canonicalize(base.join(target)) {
        return Ok((!led.starts_with(root)).then_some(led));
    }
    let mut named = false;
    let mut at: Vec<Component> = base.components().collect();
    for part in target.components() {
        match part {
            Component::ParentDir if named => return Err(()),
            Component::ParentDir => {
                if at.len() > 1 {
                    at.pop();
                }
            }
            Component::CurDir => {}
            other => {
                named = true;
                at.push(other);
            }
        }
    }
    let led: PathBuf = at.iter().collect();
    Ok((!led.starts_with(root)).then_some(led))
}

fn cstring(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn ssize(result: libc::ssize_t) -> io::Result<usize> {
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result as usize) }
}

/// The extended attributes of the entry at `path`, its link itself where it
/// is one, sorted by name.
fn attributes(path: &Path) -> io::Result<Vec<(OsString, Vec<u8>)>> {
    let name = cstring(path)?;
    // SAFETY: a null buffer of length 0 asks the size.
    let size = ssize(unsafe { libc::llistxattr(name.as_ptr(), std::ptr::null_mut(), 0) })?;
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut names = vec![0_u8; size];
    // SAFETY: the buffer is `size` bytes long.
    let size = ssize(unsafe { libc::llistxattr(name.as_ptr(), names.as_mut_ptr().cast(), names.len()) })?;
    names.truncate(size);
    let mut found = Vec::new();
    for attribute in names.split(|byte| *byte == 0).filter(|part| !part.is_empty()) {
        let key = CString::new(attribute).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        // SAFETY: as above.
        let size = ssize(unsafe { libc::lgetxattr(name.as_ptr(), key.as_ptr(), std::ptr::null_mut(), 0) })?;
        let mut value = vec![0_u8; size];
        // SAFETY: the buffer is `size` bytes long.
        let size =
            ssize(unsafe { libc::lgetxattr(name.as_ptr(), key.as_ptr(), value.as_mut_ptr().cast(), value.len()) })?;
        value.truncate(size);
        found.push((OsString::from_vec(attribute.to_vec()), value));
    }
    found.sort();
    Ok(found)
}

fn set_attributes(path: &Path, attributes: &[(OsString, Vec<u8>)]) -> io::Result<()> {
    let name = cstring(path)?;
    for (key, value) in attributes {
        let key = CString::new(key.as_bytes()).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        // SAFETY: the value is `value.len()` bytes long.
        check(unsafe { libc::lsetxattr(name.as_ptr(), key.as_ptr(), value.as_ptr().cast(), value.len(), 0) })?;
    }
    Ok(())
}

/// Gives the entry at `path` — its link itself where it is one — the
/// owner, mode and times of `entry`.
fn settle(path: &Path, entry: &Entry) -> io::Result<()> {
    let name = cstring(path)?;
    // SAFETY: the name is terminated.
    check(unsafe { libc::lchown(name.as_ptr(), entry.uid, entry.gid) })?;
    if !matches!(entry.kind, Kind::Link(_)) {
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(entry.mode))?;
    }
    let times = [
        libc::timespec { tv_sec: entry.mtime.0, tv_nsec: entry.mtime.1 },
        libc::timespec { tv_sec: entry.mtime.0, tv_nsec: entry.mtime.1 },
    ];
    // SAFETY: two timespecs, the name terminated.
    check(unsafe { libc::utimensat(libc::AT_FDCWD, name.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) })
}

/// One line of the manifest: what the entry is, and its content's digest
/// or where it leads.
fn line(entry: &Entry, content: &str) -> String {
    let kind = match &entry.kind {
        Kind::Directory => "d",
        Kind::File => "f",
        Kind::Link(_) => "l",
        Kind::Other => "o",
    };
    format!(
        "{kind} {:o} {} {} {}.{:09} {} {} {}\n",
        entry.mode,
        entry.uid,
        entry.gid,
        entry.mtime.0,
        entry.mtime.1,
        if entry.kind == Kind::File { entry.size } else { 0 },
        content,
        escape(entry.path.as_os_str())
    )
}

fn escape(path: &OsStr) -> String {
    let mut text = String::new();
    for byte in path.as_bytes() {
        match byte {
            b'\\' => text.push_str("\\\\"),
            b'\n' => text.push_str("\\n"),
            0x20..=0x7e => text.push(*byte as char),
            other => text.push_str(&format!("\\x{other:02x}")),
        }
    }
    text
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What a copy made: the manifest of the original, and the digest of each
/// file copied, read once as it was copied.
pub struct Copy {
    pub manifest: String,
    pub digests: BTreeMap<PathBuf, String>,
    pub entries: u64,
    pub bytes: u64,
}

/// Copies `entries` of `source` into `staging`, which holds the marker
/// alone. Every file and directory is on disk when it returns.
pub fn copy(source: &Path, staging: &Path, entries: &[Entry], outward_links: &[(PathBuf, PathBuf)]) -> anyhow::Result<Copy> {
    let rewritten: BTreeMap<&Path, &Path> = outward_links.iter().map(|(at, to)| (at.as_path(), to.as_path())).collect();
    let mut made: BTreeMap<(u64, u64), PathBuf> = BTreeMap::new();
    let mut copied = Copy { manifest: String::new(), digests: BTreeMap::new(), entries: 0, bytes: 0 };
    let mut directories = Vec::new();
    for entry in entries {
        let from = source.join(&entry.path);
        let to = staging.join(&entry.path);
        let content = match &entry.kind {
            Kind::Other => {
                copied.manifest.push_str(&line(entry, "-"));
                continue;
            }
            Kind::Directory => {
                if !entry.path.as_os_str().is_empty() {
                    fs::DirBuilder::new().mode(0o700).create(&to).with_context(|| format!("making {}", to.display()))?;
                }
                directories.push(entry);
                "-".to_string()
            }
            Kind::Link(target) => {
                let led = rewritten.get(entry.path.as_path()).copied().unwrap_or(target);
                std::os::unix::fs::symlink(led, &to).with_context(|| format!("making the link {}", to.display()))?;
                set_attributes(&to, &attributes(&from)?).with_context(|| format!("giving {} its attributes", to.display()))?;
                settle(&to, entry).with_context(|| format!("settling {}", to.display()))?;
                escape(target.as_os_str())
            }
            Kind::File => {
                if entry.links > 1
                    && let Some(first) = made.get(&(entry.device, entry.inode))
                {
                    fs::hard_link(first, &to).with_context(|| format!("linking {} to {}", to.display(), first.display()))?;
                    let digest = copied.digests[&first.strip_prefix(staging).expect("in the staging directory").to_path_buf()].clone();
                    copied.digests.insert(entry.path.clone(), digest.clone());
                    copied.manifest.push_str(&line(entry, &digest));
                    copied.entries += 1;
                    continue;
                }
                let digest = copy_file(&from, &to, entry)?;
                set_attributes(&to, &attributes(&from)?).with_context(|| format!("giving {} its attributes", to.display()))?;
                settle(&to, entry).with_context(|| format!("settling {}", to.display()))?;
                // What was given it on disk too, as its bytes are.
                File::open(&to).and_then(|file| file.sync_all()).with_context(|| format!("keeping {}", to.display()))?;
                if entry.links > 1 {
                    made.insert((entry.device, entry.inode), to.clone());
                }
                copied.bytes += entry.size;
                copied.digests.insert(entry.path.clone(), digest.clone());
                digest
            }
        };
        copied.manifest.push_str(&line(entry, &content));
        copied.entries += 1;
    }
    // Directories last, deepest first: what was made in them moved their times.
    for entry in directories.iter().rev() {
        let to = staging.join(&entry.path);
        set_attributes(&to, &attributes(&source.join(&entry.path))?)
            .with_context(|| format!("giving {} its attributes", to.display()))?;
        settle(&to, entry).with_context(|| format!("settling {}", to.display()))?;
        File::open(&to).and_then(|directory| directory.sync_all()).with_context(|| format!("keeping {}", to.display()))?;
    }
    Ok(copied)
}

fn copy_file(from: &Path, to: &Path, entry: &Entry) -> anyhow::Result<String> {
    let mut reading = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(from)
        .with_context(|| format!("reading {}", from.display()))?;
    let mut writing = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)
        .with_context(|| format!("writing {}", to.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    let mut total = 0_u64;
    loop {
        let read = reading.read(&mut buffer).with_context(|| format!("reading {}", from.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        writing.write_all(&buffer[..read]).with_context(|| format!("writing {}", to.display()))?;
        total += read as u64;
    }
    if total != entry.size {
        bail!("{} changed as it was copied", from.display());
    }
    writing.sync_all().with_context(|| format!("keeping {}", to.display()))?;
    Ok(hex(&digest.finalize()))
}

fn digest_of(path: &Path) -> io::Result<String> {
    let mut file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

/// What differs between the copy in `staging` and `source` as `entries`
/// found it, each said once; none where the copy holds the same and the
/// source did not change meanwhile.
pub fn differences(
    source: &Path,
    staging: &Path,
    entries: &[Entry],
    copied: &Copy,
    outward_links: &[(PathBuf, PathBuf)],
) -> anyhow::Result<Vec<String>> {
    let rewritten: BTreeMap<&Path, &Path> = outward_links.iter().map(|(at, to)| (at.as_path(), to.as_path())).collect();
    let mut differs = Vec::new();
    let now = walk(source).with_context(|| format!("reading {} again", source.display()))?;
    let then: BTreeMap<&Path, &Entry> = entries.iter().map(|entry| (entry.path.as_path(), entry)).collect();
    if now.len() != entries.len() || now.iter().any(|entry| then.get(entry.path.as_path()).is_none_or(|was| was.stamp() != entry.stamp()))
    {
        differs.push(format!("{} changed while it was copied", source.display()));
        return Ok(differs);
    }
    let copy = walk(staging).with_context(|| format!("reading {}", staging.display()))?;
    let copy: BTreeMap<PathBuf, Entry> =
        copy.into_iter().filter(|entry| entry.path != Path::new(MARKER)).map(|entry| (entry.path.clone(), entry)).collect();
    let mut groups: BTreeMap<(u64, u64), u64> = BTreeMap::new();
    let wanted = entries.iter().filter(|entry| entry.kind != Kind::Other).count();
    if copy.len() != wanted {
        differs.push(format!("the copy holds {} entries, the original {wanted}", copy.len()));
    }
    for entry in entries.iter().filter(|entry| entry.kind != Kind::Other) {
        let Some(made) = copy.get(&entry.path) else {
            differs.push(format!("{} is not in the copy", entry.path.display()));
            continue;
        };
        let kind = match &entry.kind {
            Kind::Link(target) => Kind::Link(rewritten.get(entry.path.as_path()).map_or_else(|| target.clone(), |led| led.to_path_buf())),
            other => other.clone(),
        };
        if made.kind != kind {
            differs.push(format!("{} is not the same kind of entry in the copy", entry.path.display()));
            continue;
        }
        if (made.uid, made.gid) != (entry.uid, entry.gid) {
            differs.push(format!("{} has another owner in the copy", entry.path.display()));
        }
        if !matches!(made.kind, Kind::Link(_)) && made.mode != entry.mode {
            differs.push(format!("{} has another mode in the copy", entry.path.display()));
        }
        if made.mtime != entry.mtime {
            differs.push(format!("{} has another time in the copy", entry.path.display()));
        }
        if attributes(&staging.join(&entry.path))? != attributes(&source.join(&entry.path))? {
            differs.push(format!("{} has other attributes in the copy", entry.path.display()));
        }
        if entry.kind == Kind::File {
            if made.size != entry.size || digest_of(&staging.join(&entry.path))? != copied.digests[&entry.path] {
                differs.push(format!("{} holds something else in the copy", entry.path.display()));
            }
            if entry.links > 1 {
                match groups.get(&(entry.device, entry.inode)) {
                    Some(inode) if *inode != made.inode => {
                        differs.push(format!("{} is not linked to its other names in the copy", entry.path.display()))
                    }
                    Some(_) => {}
                    None => {
                        groups.insert((entry.device, entry.inode), made.inode);
                    }
                }
            }
        }
        if differs.len() > 20 {
            break;
        }
    }
    Ok(differs)
}

/// The manifest of `root` as it is now, as `copy` wrote the original's:
/// to check the original, set aside, did not change since.
pub fn manifest(root: &Path) -> anyhow::Result<String> {
    let mut manifest = String::new();
    for entry in walk(root).with_context(|| format!("reading {}", root.display()))? {
        let content = match &entry.kind {
            Kind::File => digest_of(&root.join(&entry.path)).with_context(|| format!("reading {}", entry.path.display()))?,
            Kind::Link(target) => escape(target.as_os_str()),
            Kind::Directory | Kind::Other => "-".to_string(),
        };
        manifest.push_str(&line(&entry, &content));
    }
    Ok(manifest)
}

/// The digest a manifest goes by.
pub fn digest(manifest: &str) -> String {
    hex(&Sha256::digest(manifest.as_bytes()))
}

/// Makes the staging directory, private, with the marker in it saying
/// which move it is, both on disk.
pub fn stage(staging: &Path, op: &str) -> anyhow::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(staging).with_context(|| format!("making {}", staging.display()))?;
    let marker = staging.join(MARKER);
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&marker)?;
    file.write_all(op.as_bytes())?;
    file.sync_all()?;
    File::open(staging)?.sync_all()?;
    fastctx_platform::durable_file::sync_directory(staging.parent().expect("a staging directory's parent"))?;
    Ok(())
}

/// Whether the directory at `path` holds the marker of the move `op`.
pub fn marked_by(path: &Path, op: &str) -> bool {
    fs::read(path.join(MARKER)).is_ok_and(|bytes| bytes == op.as_bytes())
}

/// Whether the directory at `path` is the one the move `op` staged: its
/// marker says so, or it is empty as a staging directory is before its
/// marker is written (its name is the move's own).
pub fn staged_by(path: &Path, op: &str) -> bool {
    match fs::read(path.join(MARKER)) {
        Ok(bytes) => bytes == op.as_bytes(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
        }
        Err(_) => false,
    }
}

/// How a rename failed: not made, or made but not known to be on disk.
#[derive(Debug)]
pub enum Renamed {
    Not(io::Error),
    Unkept(io::Error),
}

impl std::fmt::Display for Renamed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Not(error) => write!(f, "{error}"),
            Self::Unkept(error) => write!(f, "renamed, but it could not be put on disk: {error}"),
        }
    }
}

impl std::error::Error for Renamed {}

/// Renames `from` to `to`, which must not be there, and puts the directories
/// that hold both names on disk.
pub fn rename_new(from: &Path, to: &Path) -> Result<(), Renamed> {
    let (from_name, to_name) = (cstring(from).map_err(Renamed::Not)?, cstring(to).map_err(Renamed::Not)?);
    // SAFETY: both names are terminated; renameat2 reads them only.
    let result = unsafe {
        libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, from_name.as_ptr(), libc::AT_FDCWD, to_name.as_ptr(), libc::RENAME_NOREPLACE)
    };
    if result != 0 {
        return Err(Renamed::Not(io::Error::last_os_error()));
    }
    let mut parents = vec![to.parent().expect("a renamed entry's parent")];
    if from.parent() != to.parent() {
        parents.push(from.parent().expect("a renamed entry's parent"));
    }
    for parent in parents {
        fastctx_platform::durable_file::sync_directory(parent).map_err(Renamed::Unkept)?;
    }
    Ok(())
}
