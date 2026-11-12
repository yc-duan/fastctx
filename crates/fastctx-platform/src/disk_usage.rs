//! Apparent disk usage, with the same link semantics as `du -sb`.
//!
//! What cannot be read (a file another program holds without sharing, a
//! directory refused to this user, an entry gone while it was walked) is
//! skipped and counted, as `du` goes on past it: one such entry never makes
//! the whole measure fail. Only the starting path itself must be readable.

use std::collections::HashSet;
use std::io;
use std::path::Path;

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
/// never following symbolic links or Windows junctions.
pub fn visit(path: &Path, mut visitor: impl FnMut(&Path, &std::fs::Metadata, u64)) -> io::Result<Unread> {
    let mut unread = Unread::default();
    walk(path, true, &mut HashSet::new(), &mut visitor, &mut unread)?;
    Ok(unread)
}

pub fn visit_many<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    mut visitor: impl FnMut(&Path, &std::fs::Metadata, u64),
) -> io::Result<Unread> {
    let mut seen = HashSet::new();
    let mut unread = Unread::default();
    for path in paths {
        walk(path, true, &mut seen, &mut visitor, &mut unread)?;
    }
    Ok(unread)
}

/// Walks `path`; an error there is the caller's only when it is the
/// starting path (`top`), otherwise it is noted and skipped.
fn walk(
    path: &Path,
    top: bool,
    seen: &mut HashSet<(u64, u64)>,
    visitor: &mut impl FnMut(&Path, &std::fs::Metadata, u64),
    unread: &mut Unread,
) -> io::Result<()> {
    let skip = |unread: &mut Unread, error: io::Error| -> io::Result<()> {
        if top {
            return Err(error);
        }
        unread.note(path, &error);
        Ok(())
    };
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return skip(unread, error),
    };
    let link = crate::owned_directory::is_link(&metadata);
    let bytes = if metadata.is_file() && !link {
        match identity(path, &metadata) {
            Ok(identity) if !seen.insert(identity) => 0,
            Ok(_) => metadata.len(),
            // Gone since it was listed.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            // Its size is known even where its identity is not: it is
            // counted, at worst twice for a hard link.
            Err(_) => metadata.len(),
        }
    } else {
        match apparent(path, &metadata, link) {
            Ok(bytes) => bytes,
            Err(error) => return skip(unread, error),
        }
    };
    visitor(path, &metadata, bytes);
    if metadata.is_dir() && !link {
        let listed = match std::fs::read_dir(path) {
            Ok(listed) => listed,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return skip(unread, error),
        };
        let mut entries = Vec::new();
        for entry in listed {
            match entry {
                Ok(entry) => entries.push(entry.path()),
                Err(error) => unread.note(path, &error),
            }
        }
        entries.sort();
        for path in entries {
            walk(&path, false, seen, visitor, unread)?;
        }
    }
    Ok(())
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
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    };
    // Asking for no access but its attributes, sharing everything: a file
    // another program holds open (a database's journal, a running program)
    // is still identified, and nothing of it is read.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)?;
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
}
