//! Files whose visibility is published by a separate durable database commit.
//!
//! These functions do not provide atomic replacement. The caller owns an
//! unpublished generation exclusively until its bytes and directory are durable.

use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

/// Creates missing private directories one level at a time. Existing directories
/// are never replaced or chmodded. The requested directory must not be a link;
/// its ancestors are within the caller's trusted canonical storage root.
pub fn prepare_directory(path: &Path) -> io::Result<()> {
    crate::roles::in_place("prepare durable content directory", || prepare(path))
}
fn prepare(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "content directories must be absolute"));
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || crate::owned_directory::is_link(&metadata) {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "content path is not a plain directory"));
            }
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent =
        path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "content directory has no parent"))?;
    prepare(parent)?;
    create_directory(path)
}

/// Reads one bounded, ordinary content file and charges all bytes consumed,
/// including a partial read that ultimately fails.
pub fn read_bounded(path: &Path, max_bytes: u32) -> io::Result<Vec<u8>> {
    crate::roles::in_place("read bounded durable content", || {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || crate::owned_directory::is_link(&metadata) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "content is not a plain file"));
        }
        let file = crate::disk::open_regular(path)?;
        let mut body = Vec::with_capacity(max_bytes as usize);
        let result = file.take(u64::from(max_bytes) + 1).read_to_end(&mut body);
        crate::cost::read(body.len());
        result?;
        if body.len() > max_bytes as usize {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "content exceeds its read bound"));
        }
        Ok(body)
    })
}

/// Writes a generation hidden by the database. The caller must hold its content
/// lease and must not point a published representation at this path yet. A failed
/// write leaves an unpublished file for the caller's durable retry/cleanup intent.
pub fn write_unpublished(path: &Path, bytes: &[u8]) -> io::Result<()> {
    crate::roles::in_place("write durable unpublished content", || {
        let parent =
            path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "content file has no parent"))?;
        prepare(parent)?;
        if let Ok(metadata) = fs::symlink_metadata(path)
            && (!metadata.is_file() || crate::owned_directory::is_link(&metadata))
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "unpublished content is not a plain file"));
        }
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        configure(&mut options);
        let mut file = options.open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        sync_parent(path)
    })
}

/// Deletes an unreferenced generation after its readers have drained. Windows
/// first durably truncates it: a delayed namespace deletion must not restore
/// readable private content after an exposure-cleanup receipt (2026-11-08).
pub fn delete_unreferenced(path: &Path) -> io::Result<()> {
    crate::roles::in_place("delete durable content", || {
        let metadata = match fs::symlink_metadata(path) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return sync_parent(path),
            Err(error) => return Err(error),
        };
        if !metadata.is_file() || crate::owned_directory::is_link(&metadata) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "content deletion requires a plain file"));
        }
        truncate_before_delete(path)?;
        fs::remove_file(path)?;
        sync_parent(path)
    })
}

#[cfg(unix)]
fn configure(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}
#[cfg(windows)]
fn configure(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH;
    // CreateFileW documents write-through metadata flushing, including renames.
    options.custom_flags(FILE_FLAG_WRITE_THROUGH);
}
#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    fs::File::open(path.parent().ok_or_else(|| io::Error::other("content has no parent"))?)?.sync_all()
}
#[cfg(windows)]
fn sync_parent(_: &Path) -> io::Result<()> {
    Ok(())
}
#[cfg(unix)]
fn truncate_before_delete(_: &Path) -> io::Result<()> {
    Ok(())
}
#[cfg(windows)]
fn truncate_before_delete(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true);
    configure(&mut options);
    let file = options.open(path)?;
    file.set_len(0)?;
    file.sync_all()
}
#[cfg(unix)]
fn create_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            prepare(path)?;
        }
        Err(error) => return Err(error),
    }
    sync_parent(path)
}
#[cfg(windows)]
fn create_directory(path: &Path) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or_else(|| io::Error::other("content has no parent"))?;
    let temporary = loop {
        let candidate =
            parent.join(format!(".content-dir-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        crate::ensure_private_dir(&temporary)?;
        let from = crate::win32_path::wide(&temporary)?;
        let to = crate::win32_path::wide(path)?;
        // SAFETY: both paths are NUL-terminated and remain live through the call.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(80 | 183)) { prepare(path) } else { Err(error) }
    })();
    if temporary.exists() {
        let _ = fs::remove_dir(&temporary);
    }
    result
}
