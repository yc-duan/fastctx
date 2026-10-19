//! Lossless retirement of objects whose historical contents are unknown.
//!
//! Same-filesystem moves preserve the original object, including links and
//! attributes. Cross-filesystem copies publish a verified complete tree before
//! retiring any source. A durable manifest preserves identities and attributes
//! through interruption; a published archive is never modified on recovery.

use std::fs::{self, File};
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::disk::{self, Identity};

use super::files;

mod flush;
#[cfg(test)]
mod health;
mod metadata;
mod parallel;
mod tree;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Object {
    pub path: PathBuf,
    pub identity: Identity,
}

impl Object {
    pub fn read(path: &Path) -> io::Result<Option<Self>> {
        files::check_path(path.parent().ok_or_else(|| io::Error::other("archive source has no parent"))?)?;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let parent = fs::canonicalize(path.parent().expect("checked above"))?;
        let path = parent.join(path.file_name().ok_or_else(|| io::Error::other("archive source has no name"))?);
        Ok(Some(Self { identity: identity(&path, &metadata)?, path }))
    }

    pub fn verify(&self) -> io::Result<()> {
        if Self::read(&self.path)?.as_ref() != Some(self) {
            return Err(io::Error::other("archive source identity changed; left untouched"));
        }
        Ok(())
    }

    fn read_child(parent: &Self, path: &Path) -> io::Result<Option<Self>> {
        if path.parent() != Some(parent.path.as_path()) {
            return Err(conflict("archive entry is outside its recorded parent"));
        }
        // Bind to the same ordinary directory before and after reading, as a
        // transaction's Snapshot anchor does. The tree validates its root and
        // establishes each child directory's identity before visiting children.
        if let Err(error) = parent.verify_directory() {
            if error.kind() == io::ErrorKind::NotFound
                && fs::symlink_metadata(path).is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
            {
                return Ok(None);
            }
            return Err(error);
        }
        let object = match fs::symlink_metadata(path) {
            Ok(metadata) => Some(Self { path: path.into(), identity: identity(path, &metadata)? }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        parent.verify_directory()?;
        Ok(object)
    }

    fn verify_directory(&self) -> io::Result<()> {
        let file = disk::open_directory(&self.path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || disk::is_link(&metadata) || disk::identity(&file)? != self.identity {
            return Err(conflict("archive parent directory changed; retained"));
        }
        Ok(())
    }
}

pub(super) fn transfer(
    object: &Object,
    destination: &Path,
    locks: &[&File],
    copying: bool,
    previous: &[PathBuf],
    begin_copy: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    if super::processes::within(destination, &object.path) {
        return Err(io::Error::other("archive destination is inside its source; both locations left untouched"));
    }
    object.verify()?;
    let parent = destination.parent().ok_or_else(|| io::Error::other("archive destination has no parent"))?;
    files::check_path(parent)?;
    if !parent.exists() {
        fastctx_platform::ensure_private_dir(parent)?;
    }
    files::check_path(parent)?;
    // The copy manifest is itself durable before any copied source is retired.
    // It also identifies an interrupted copy before the batch result was saved.
    if copying || parent.join("copy-manifest.json").try_exists()? {
        if !copying {
            begin_copy()?;
        }
        return tree::transfer(object, destination, locks, previous);
    }
    object.verify()?;
    match rename_new(&object.path, destination) {
        Ok(()) => {
            disk::sync_parent(&object.path)?;
            disk::sync_parent(destination)
        }
        Err(error) if cross_device(&error) => {
            begin_copy()?;
            tree::transfer(object, destination, locks, previous)
        }
        Err(error) => Err(error),
    }
}

fn conflict(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(super) fn completed_copy(object: &Object, destination: &Path) -> io::Result<bool> {
    tree::completed(object, destination)
}

fn credential(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("").to_ascii_lowercase();
    name.ends_with(".key") || ["auth.json", ".credentials.json", "world.keys"].contains(&name.as_str())
}

struct LockedFile<'a> {
    file: &'a File,
    access: std::sync::Mutex<()>,
}

type LockedFiles<'a> = std::collections::HashMap<(u64, u128), LockedFile<'a>>;

fn source_file(object: &Object, locks: &LockedFiles<'_>) -> io::Result<File> {
    if credential(&object.path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "credential files may be moved intact but are not read or copied across filesystems; original retained",
        ));
    }
    if let Some(file) = locks.get(&(object.identity.volume, object.identity.file)) {
        return file.file.try_clone();
    }
    disk::open(&object.path)
}

pub(super) fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = disk::open(path)?;
    disk::ordinary(&file.metadata()?)?;
    hash_handle(&mut file)
}

pub(super) fn hash_handle(file: &mut File) -> io::Result<String> {
    struct Hash(Sha256);
    impl Write for Hash {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    file.rewind()?;
    let mut hash = Hash(Sha256::new());
    io::copy(file, &mut hash)?;
    file.rewind()?;
    Ok(format!("{:x}", hash.0.finalize()))
}

#[cfg(unix)]
fn directory_for_attributes(path: &Path) -> io::Result<File> {
    disk::open_directory(path)
}

#[cfg(windows)]
fn directory_for_attributes(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::*;
    std::fs::OpenOptions::new()
        .read(true)
        .access_mode(
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | READ_CONTROL | WRITE_DAC | WRITE_OWNER,
        )
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(unix)]
fn delete_copied(source: &File, path: &Path) -> io::Result<()> {
    if source.metadata()?.is_dir() { fs::remove_dir(path) } else { disk::delete(source, path) }
}

#[cfg(windows)]
fn delete_copied(source: &File, path: &Path) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::*;
    let deleting = std::fs::OpenOptions::new()
        .read(true)
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    if disk::identity(&deleting)? != disk::identity(source)? {
        return Err(io::Error::other("source replaced before retirement"));
    }
    // The complete archive already preserves the read-only attribute. Removing
    // this verified source does not require modifying that archived attribute.
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: deletion is bound to the verified source handle; nonempty
    // directories are rejected by the filesystem.
    if unsafe {
        SetFileInformationByHandle(
            deleting.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const info).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn identity(_path: &Path, metadata: &fs::Metadata) -> io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    Ok(Identity { volume: metadata.dev(), file: metadata.ino() as u128 })
}

#[cfg(windows)]
fn identity(path: &Path, _metadata: &fs::Metadata) -> io::Result<Identity> {
    let file = disk::open_directory(path)?;
    disk::identity(&file)
}

fn cross_device(error: &io::Error) -> bool {
    #[cfg(unix)]
    return error.raw_os_error() == Some(libc::EXDEV);
    #[cfg(windows)]
    return error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_NOT_SAME_DEVICE as i32);
}

fn sync_directory(file: &File) -> io::Result<()> {
    if cfg!(windows) { Ok(()) } else { file.sync_all() }
}

/// The installer's no-replace rename, with its way for kernels and
/// filesystems without one.
#[cfg(target_os = "linux")]
pub(super) fn rename_new(source: &Path, destination: &Path) -> io::Result<()> {
    disk::move_preserving(source, destination)
}

#[cfg(target_os = "macos")]
pub(super) fn rename_new(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from = CString::new(source.as_os_str().as_bytes())?;
    let to = CString::new(destination.as_os_str().as_bytes())?;
    // SAFETY: RENAME_EXCL leaves an existing destination unchanged.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
pub(super) fn rename_new(source: &Path, destination: &Path) -> io::Result<()> {
    // Bind the rename to a source handle; the destination is never replaced.
    disk::move_preserving(source, destination)
}
