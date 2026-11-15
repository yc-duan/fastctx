//! Apparent disk usage, with the same link semantics as `du -sb`.
//!
//! What cannot be read (a file another program holds without sharing, a
//! directory refused to this user, an entry gone while it was walked) is
//! skipped and counted, as `du` goes on past it: one such entry never makes
//! the whole measure fail. Only the starting path itself must be readable.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

/// What a walk could not read and skipped: how many entries, and the first
/// of them with why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unread {
    pub entries: u64,
    pub first: Option<String>,
}

impl Unread {
    fn note(&mut self, path: &Path, error: &io::Error) {
        self.entries += 1;
        if self.first.is_none() {
            self.first = Some(format!("{}: {error}", path.display()));
        }
    }

    pub fn add(&mut self, other: Unread) {
        self.entries += other.entries;
        if self.first.is_none() {
            self.first = other.first;
        }
    }
}

/// Visits directories and files, counting each hard-linked file once and
/// never following symbolic links or Windows junctions. Each is told in the
/// order it is walked: a directory before what it holds, a directory's
/// entries by name.
pub fn visit(path: &Path, mut visitor: impl FnMut(&Path, &std::fs::Metadata, u64)) -> io::Result<Unread> {
    let mut seen = HashSet::new();
    let mut walk = Walk { seen: &mut seen, visitor: &mut visitor, unread: Unread::default(), met: Vec::new(), asked: 0 };
    walk.start(path)?;
    Ok(walk.unread)
}

pub fn visit_many<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    mut visitor: impl FnMut(&Path, &std::fs::Metadata, u64),
) -> io::Result<Unread> {
    let mut seen = HashSet::new();
    let mut walk = Walk { seen: &mut seen, visitor: &mut visitor, unread: Unread::default(), met: Vec::new(), asked: 0 };
    for path in paths {
        walk.start(path)?;
    }
    Ok(walk.unread)
}

/// How many files a walk asks about at once (`describe`), and how much of
/// what it met it holds before it tells it.
const ASKED_AT_ONCE: usize = 1024;
const HELD_AT_ONCE: usize = 4 * ASKED_AT_ONCE;

/// What a walk met, held until it is told.
enum Met {
    /// A directory or a link, with the bytes it counts for.
    Counted { path: PathBuf, metadata: std::fs::Metadata, bytes: u64 },
    /// A file: as the walk read it, or, `asked`, to be asked itself. On
    /// Windows a directory's listing gives each entry's size and times as
    /// they were when a handle to the file last closed, so one a program
    /// holds open (a database, a log) shows what it was then; each file is
    /// asked through a handle of its own, many at once.
    File { path: PathBuf, metadata: std::fs::Metadata, asked: bool },
}

/// A file as its own handle describes it, with its identity.
type Described = io::Result<(std::fs::Metadata, io::Result<(u64, u64)>)>;

struct Walk<'a, V> {
    seen: &'a mut HashSet<(u64, u64)>,
    visitor: &'a mut V,
    unread: Unread,
    met: Vec<Met>,
    asked: usize,
}

impl<V: FnMut(&Path, &std::fs::Metadata, u64)> Walk<'_, V> {
    /// Walks from `path`, which must be readable unless it is not there.
    fn start(&mut self, path: &Path) -> io::Result<()> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let walked = self.walk(path.to_path_buf(), metadata, true);
        self.tell();
        walked
    }

    /// Walks `path`, described by `metadata`: the starting path as asked of
    /// it, any other as its directory's listing gave it. An error there is
    /// the caller's only when it is the starting path (`top`), otherwise it
    /// is noted and skipped.
    fn walk(&mut self, path: PathBuf, metadata: std::fs::Metadata, top: bool) -> io::Result<()> {
        let link = crate::owned_directory::is_link(&metadata);
        if metadata.is_file() && !link {
            let asked = cfg!(windows) && !top;
            self.asked += usize::from(asked);
            self.met.push(Met::File { path, metadata, asked });
            self.hold();
            return Ok(());
        }
        let bytes = match apparent(&path, &metadata, link) {
            Ok(bytes) => bytes,
            Err(error) => return self.skip(&path, top, error),
        };
        let directory = metadata.is_dir() && !link;
        self.met.push(Met::Counted { path: path.clone(), metadata, bytes });
        self.hold();
        if !directory {
            return Ok(());
        }
        let listed = match std::fs::read_dir(&path) {
            Ok(listed) => listed,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return self.skip(&path, top, error),
        };
        let mut entries = Vec::new();
        for entry in listed {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.unread.note(&path, &error);
                    continue;
                }
            };
            // On Windows the listing describes each entry; elsewhere this
            // is the one `lstat` the entry needs.
            match entry.metadata() {
                Ok(metadata) => entries.push((entry.path(), metadata)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => self.unread.note(&entry.path(), &error),
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, metadata) in entries {
            self.walk(path, metadata, false)?;
        }
        Ok(())
    }

    fn skip(&mut self, path: &Path, top: bool, error: io::Error) -> io::Result<()> {
        if top {
            return Err(error);
        }
        self.unread.note(path, &error);
        Ok(())
    }

    /// Tells what was met once enough of it is held.
    fn hold(&mut self) {
        if self.asked >= ASKED_AT_ONCE || self.met.len() >= HELD_AT_ONCE {
            self.tell();
        }
    }

    /// Tells the visitor what was met, in order, the files to be asked
    /// asked first.
    fn tell(&mut self) {
        let met = std::mem::take(&mut self.met);
        self.asked = 0;
        let described = describe(&met);
        for (one, described) in met.into_iter().zip(described) {
            match one {
                Met::Counted { path, metadata, bytes } => (self.visitor)(&path, &metadata, bytes),
                Met::File { path, metadata, asked } => {
                    let (metadata, identity) = match (asked, described) {
                        (true, Some(Ok(described))) => described,
                        // Gone since it was listed.
                        (true, Some(Err(error))) if error.kind() == io::ErrorKind::NotFound => continue,
                        // Not to be opened: as listed, its identity unknown.
                        (true, Some(Err(error))) => (metadata, Err(error)),
                        _ => {
                            let identity = identity(&path, &metadata);
                            (metadata, identity)
                        }
                    };
                    let bytes = match identity {
                        Ok(identity) if !self.seen.insert(identity) => 0,
                        Ok(_) => metadata.len(),
                        // Gone since it was listed.
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        // Its size is known even where its identity is not:
                        // it is counted, at worst twice for a hard link.
                        Err(_) => metadata.len(),
                    };
                    (self.visitor)(&path, &metadata, bytes);
                }
            }
        }
    }
}

/// The files of `met` to be asked, each asked itself: none off Windows,
/// where what the walk read of each is its own.
#[cfg(not(windows))]
fn describe(met: &[Met]) -> Vec<Option<Described>> {
    met.iter().map(|_| None).collect()
}

/// The files of `met` to be asked, each asked through a handle of its own,
/// on as many threads as the machine has to (at most eight): an open is the
/// most of a file's cost, and a file system answers several at once.
#[cfg(windows)]
fn describe(met: &[Met]) -> Vec<Option<Described>> {
    let asked: Vec<(usize, &Path)> = met
        .iter()
        .enumerate()
        .filter_map(|(index, one)| match one {
            Met::File { path, asked: true, .. } => Some((index, path.as_path())),
            _ => None,
        })
        .collect();
    let mut told: Vec<Option<Described>> = met.iter().map(|_| None).collect();
    if asked.is_empty() {
        return told;
    }
    let workers = std::thread::available_parallelism().map_or(1, std::num::NonZero::get).clamp(1, 8);
    let part = asked.len().div_ceil(workers);
    let answers: Vec<Described> = std::thread::scope(|scope| {
        let parts: Vec<_> = asked
            .chunks(part)
            .map(|chunk| scope.spawn(move || chunk.iter().map(|(_, path)| described(path)).collect::<Vec<_>>()))
            .collect();
        parts.into_iter().flat_map(|part| part.join().expect("a file described")).collect()
    });
    for ((index, _), answer) in asked.into_iter().zip(answers) {
        told[index] = Some(answer);
    }
    told
}

/// A file as a handle to it describes it: its size and times as they are,
/// where its directory's listing may hold older ones. Only its attributes
/// are asked for, sharing everything: a file another program holds open (a
/// database's journal, a running program) is still described, and nothing
/// of it is read. A link put in its place since is described, not followed.
#[cfg(windows)]
fn described(path: &Path) -> Described {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    Ok((metadata, handle_identity(&file)))
}

#[cfg(unix)]
fn apparent(_path: &Path, metadata: &std::fs::Metadata, _link: bool) -> io::Result<u64> {
    Ok(metadata.len())
}

#[cfg(windows)]
fn apparent(path: &Path, metadata: &std::fs::Metadata, link: bool) -> io::Result<u64> {
    if link {
        if reparse_tag(path)? == 0x80000023 {
            // IO_REPARSE_TAG_AF_UNIX is a socket, not a filesystem alias.
            return Ok(metadata.len());
        }
        // MSYS du measures the UTF-8 link target without the NT path prefix.
        let target = std::fs::read_link(path)?;
        let target = target.to_string_lossy();
        let bytes = if let Some(unc) = target.strip_prefix("\\\\?\\UNC\\") {
            unc.len() + 2
        } else {
            target.strip_prefix("\\\\?\\").unwrap_or(&target).len()
        };
        return Ok(bytes as u64);
    }
    Ok(if metadata.is_dir() { 0 } else { metadata.len() })
}

#[cfg(windows)]
fn reparse_tag(path: &Path) -> io::Result<u32> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};
    let name = crate::win32_path::wide(path)?;
    let mut information: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    // SAFETY: the name and output buffer live through this synchronous call.
    let handle = unsafe { FindFirstFileW(name.as_ptr(), &mut information) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        FindClose(handle);
    }
    Ok(information.dwReserved0)
}

#[cfg(unix)]
fn identity(_path: &Path, metadata: &std::fs::Metadata) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn identity(path: &Path, _metadata: &std::fs::Metadata) -> io::Result<(u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    // Asking for its attributes alone, sharing everything: a file another
    // program holds open is still identified, and nothing of it is read.
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)?;
    handle_identity(&file)
}

/// The volume and file index of an open file: what its hard links share.
#[cfg(windows)]
fn handle_identity(file: &std::fs::File) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: an owned file handle and a valid output buffer.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        u64::from(information.dwVolumeSerialNumber),
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_another_program_holds_is_still_measured() {
        let root = std::env::temp_dir().join(format!("fastctx-disk-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("inner")).unwrap();
        std::fs::write(root.join("inner/held"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("free"), vec![0u8; 24]).unwrap();
        // Held open without any sharing, as a database holds its journal.
        #[cfg(windows)]
        let _held = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(root.join("inner/held")).unwrap()
        };
        let mut files = 0;
        let mut bytes = 0;
        let unread = visit(&root, |_, metadata, counted| {
            if metadata.is_file() {
                files += 1;
                bytes += counted;
            }
        })
        .unwrap();
        assert_eq!((files, bytes), (2, 1024));
        assert_eq!(unread, Unread::default());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_linked_twice_is_counted_once_and_every_entry_is_visited() {
        let root = std::env::temp_dir().join(format!("fastctx-disk-usage-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/deep/deeper")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("a/once"), vec![0u8; 300]).unwrap();
        std::fs::hard_link(root.join("a/once"), root.join("b/again")).unwrap();
        std::fs::write(root.join("a/deep/deeper/last"), vec![0u8; 7]).unwrap();
        let mut seen = Vec::new();
        let mut bytes = 0;
        visit(&root, |path, metadata, counted| {
            seen.push(path.strip_prefix(&root).unwrap().to_path_buf());
            if metadata.is_file() {
                bytes += counted;
            }
        })
        .unwrap();
        assert_eq!(bytes, 307);
        let names: Vec<_> = seen.iter().map(|path| path.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, ["", "a", "a/deep", "a/deep/deeper", "a/deep/deeper/last", "a/once", "b", "b/again"]);
        // Walked from the second directory alone, the file counts there.
        let mut alone = 0;
        visit(&root.join("b"), |_, metadata, counted| {
            if metadata.is_file() {
                alone += counted;
            }
        })
        .unwrap();
        assert_eq!(alone, 300);
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn a_file_grown_through_another_name_or_held_open_counts_whole() {
        use std::io::Write as _;
        let root = std::env::temp_dir().join(format!("fastctx-disk-usage-grown-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Made empty, linked, and grown through the link: a Windows listing
        // of the first name still says it is empty.
        std::fs::write(root.join("a"), b"").unwrap();
        std::fs::hard_link(root.join("a"), root.join("z")).unwrap();
        std::fs::OpenOptions::new().append(true).open(root.join("z")).unwrap().write_all(&[1u8; 70_000]).unwrap();
        // Grown and held open, as a database is.
        let mut held = std::fs::File::create(root.join("held")).unwrap();
        held.write_all(&[2u8; 50_000]).unwrap();
        held.sync_all().unwrap();
        let mut bytes = 0;
        visit(&root, |_, metadata, counted| {
            if metadata.is_file() {
                bytes += counted;
            }
        })
        .unwrap();
        assert_eq!(bytes, 120_000);
        drop(held);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// How long a walk of the tree `FASTCTX_MEASURE` names takes, and how
    /// long it took when each file was asked for its metadata and identity
    /// itself: by hand, `cargo test --release -p fastctx-platform --lib
    /// walks_a_real_tree -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn walks_a_real_tree() {
        let Some(root) = std::env::var_os("FASTCTX_MEASURE") else { return };
        let root = std::path::PathBuf::from(root);
        let walk = || {
            let began = std::time::Instant::now();
            let (mut entries, mut bytes) = (0u64, 0u64);
            let unread = visit(&root, |_, _, counted| {
                entries += 1;
                bytes += counted;
            })
            .unwrap();
            println!("walked {entries} entries, {bytes} bytes, {} unread, in {:?}", unread.entries, began.elapsed());
        };
        walk();
        let began = std::time::Instant::now();
        let mut files = 0u64;
        visit(&root, |path, metadata, _| {
            let _ = std::fs::symlink_metadata(path);
            if metadata.is_file() {
                let _ = identity(path, metadata);
                files += 1;
            }
        })
        .unwrap();
        println!("each file asked itself as well ({files} files): {:?}", began.elapsed());
        walk();
    }
}
