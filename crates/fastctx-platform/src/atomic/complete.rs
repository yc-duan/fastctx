//! Durable publication of a closed file produced by an external writer such as SQLite.
use crate::rooted_path::Directory;
use std::{
    fs::{self, OpenOptions},
    io,
    path::Path,
};

/// Flushes a closed regular file and its ancestry. The caller exclusively owns
/// the private directory until publication; no writer may reopen the file.
pub fn sync_existing(path: &Path) -> io::Result<()> {
    crate::roles::in_place("sync completed file", || {
        let parent = directory(path)?;
        regular(path)?;
        OpenOptions::new().read(true).write(true).open(path)?.sync_all()?;
        parent.sync_chain()
    })
}

/// Publishes a closed file without replacing any existing destination. Both
/// names must be in the same private directory. On error the caller reconciles
/// the names: publication may have happened before a durability error.
pub fn publish_existing_new(source: &Path, target: &Path) -> io::Result<()> {
    crate::roles::in_place("publish completed file", || {
        if source.parent() != target.parent() || source == target {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "publication needs distinct sibling names"));
        }
        let parent = directory(source)?;
        regular(source)?;
        OpenOptions::new().read(true).write(true).open(source)?.sync_all()?;
        publish(source, target)?;
        parent.sync_chain()
    })
}
fn directory(path: &Path) -> io::Result<Directory> {
    let parent = path.parent().ok_or_else(|| io::Error::other("completed file has no parent"))?;
    Directory::open_root(parent, false)
}
fn regular(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "completed file is not a regular file"));
    }
    Ok(())
}
#[cfg(all(unix, not(target_os = "android")))]
fn publish(source: &Path, target: &Path) -> io::Result<()> {
    fs::hard_link(source, target)?;
    // Keep the source until the new link is durable. A crash can leave both.
    directory(target)?.sync_chain()?;
    fs::remove_file(source)
}
#[cfg(target_os = "android")]
fn publish(source: &Path, target: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let target = CString::new(target.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: live NUL-terminated paths, with atomic no-replace semantics.
    if unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(windows)]
fn publish(source: &Path, target: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let source = crate::win32_path::wide(source)?;
    let target = crate::win32_path::wide(target)?;
    // SAFETY: live NUL-terminated sibling paths. Neither replacement nor a
    // cross-volume copy is allowed; write-through completes the move on disk.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
