use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::SystemTime;

use xattr::FileExt;

use super::Identity;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Attributes {
    mode: u32,
    uid: u32,
    gid: u32,
    extended: Vec<(OsString, Vec<u8>)>,
}

impl Attributes {
    pub(crate) fn revision_bytes(&self) -> io::Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(io::Error::other)
    }

    pub(crate) fn publication_bytes(&self) -> io::Result<Vec<u8>> {
        self.revision_bytes()
    }

    pub fn has_named_data(&self) -> bool {
        !self.extended.is_empty()
    }

    pub fn apply_directory(&self, file: &File, path: &Path) -> io::Result<()> {
        self.apply(file, path)
    }

    pub fn protect(&self, _file: &File) -> io::Result<()> {
        // Existing files are staged with mode 0600 until apply() restores
        // their complete ownership, ACL and mode.
        Ok(())
    }

    pub fn read(file: &File, _path: &Path) -> io::Result<Self> {
        let metadata = file.metadata()?;
        let mut extended = Vec::new();
        for name in file.list_xattr()? {
            let value =
                file.get_xattr(&name)?.ok_or_else(|| io::Error::other("extended attribute changed while reading"))?;
            extended.push((name, value));
        }
        extended.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Self { mode: metadata.mode() & 0o7777, uid: metadata.uid(), gid: metadata.gid(), extended })
    }

    pub fn apply(&self, file: &File, _path: &Path) -> io::Result<()> {
        let metadata = file.metadata()?;
        if metadata.uid() != self.uid || metadata.gid() != self.gid {
            // SAFETY: the file descriptor is live and owned by the caller.
            if unsafe { libc::fchown(file.as_raw_fd(), self.uid, self.gid) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        for name in file.list_xattr()? {
            if !self.extended.iter().any(|(expected, _)| *expected == name) {
                file.remove_xattr(&name)?;
            }
        }
        for (name, value) in &self.extended {
            file.set_xattr(name, value)?;
        }
        // Ownership changes can clear set-id bits; restore mode last.
        file.set_permissions(fs::Permissions::from_mode(self.mode))
    }

    pub fn writable(&self) -> bool {
        self.mode & 0o222 != 0
    }
}

pub(crate) fn open(path: &Path) -> io::Result<File> {
    // O_NONBLOCK prevents a concurrent replacement with a FIFO from hanging
    // before the caller can reject its file type.
    OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)
}

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(path)
}

pub(crate) fn create(path: &Path, mode: u32) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).create_new(true).mode(mode).open(path)
}

pub(crate) fn identity(file: &File) -> io::Result<Identity> {
    let metadata = file.metadata()?;
    Ok(Identity { volume: metadata.dev(), file: metadata.ino() as u128 })
}

pub(crate) fn is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub(crate) fn publish(file: &File, stage: &Path, target: &Path, replace: bool) -> io::Result<()> {
    let _ = file;
    if replace {
        fs::rename(stage, target)
    } else {
        // Creating a link is an atomic no-clobber publication. The staging
        // name is removed separately so that failure still records publication.
        fs::hard_link(stage, target)
    }
}

pub(crate) fn stage_survives_publication(replace: bool) -> bool {
    !replace
}

pub(crate) fn delete_staged(_file: &File, path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

pub(crate) fn delete(_file: &File, path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

/// Unlinking does not need or change the file object's permissions.
pub(crate) fn delete_owned(file: &File, path: &Path) -> io::Result<()> {
    delete(file, path)
}

pub(crate) fn restore_modified(file: &File, modified: SystemTime) -> io::Result<()> {
    file.set_modified(modified)
}

pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    sync_directory(path.parent().expect("absolute path"))
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    open_directory(path)?.sync_all()
}

pub(crate) fn replace_preserving(source: &Path, target: &Path, displaced: &Path) -> io::Result<()> {
    // An interruption between these operations leaves the displaced object
    // at source. Both names are in the journal before the exchange occurs.
    match exchange(source, target) {
        Ok(()) => {}
        // Without the atomic exchange the old object moves aside first and
        // the new one takes its name. An interruption between the two leaves
        // the target missing and the old object displaced, as ReplaceFile
        // can on Windows, and the journal completes it the same way.
        Err(error) if unsupported(&error) => {
            rename_exclusive(target, displaced)?;
            sync_parent(displaced)?;
            rename_exclusive(source, target)?;
            return sync_parent(target);
        }
        Err(error) => return Err(error),
    }
    sync_parent(source)?;
    rename_exclusive(source, displaced)?;
    sync_parent(displaced)
}

pub(crate) fn move_preserving(source: &Path, destination: &Path) -> io::Result<()> {
    rename_exclusive(source, destination)
}

#[cfg(target_os = "linux")]
fn rename_with_flags(source: &Path, destination: &Path, flags: u32) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
    // SAFETY: both C strings remain live throughout the syscall.
    let result = unsafe {
        libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, source.as_ptr(), libc::AT_FDCWD, destination.as_ptr(), flags)
    };
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(target_os = "macos")]
fn rename_with_flags(source: &Path, destination: &Path, flags: u32) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
    // SAFETY: both paths are terminated strings. Unsupported filesystems fail;
    // falling back to an overwriting rename would destroy the displaced version.
    let result = unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), flags) };
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Whether the kernel or the filesystem lacks the flagged rename: Linux has
/// renameat2 from 3.15 on, and a filesystem may refuse its flags.
#[cfg(target_os = "linux")]
fn unsupported(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL))
}

/// A Mac's filesystem without the flagged rename fails: falling back to an
/// overwriting rename would destroy the displaced version.
#[cfg(not(target_os = "linux"))]
fn unsupported(_: &io::Error) -> bool {
    false
}

/// Renames `source` to `destination` unless that name is taken.
#[cfg(target_os = "linux")]
fn rename_if_free(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(io::Error::from_raw_os_error(libc::EEXIST)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::rename(source, destination),
        Err(error) => Err(error),
    }
}

fn exchange(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    return rename_with_flags(source, destination, libc::RENAME_EXCHANGE);
    #[cfg(target_os = "macos")]
    return rename_with_flags(source, destination, libc::RENAME_SWAP);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (source, destination);
        Err(io::Error::new(io::ErrorKind::Unsupported, "atomic file exchange is unavailable on this platform"))
    }
}

fn rename_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    // Without the flagged rename a name already taken is refused first: the
    // names are the installer's own, and nothing else writes them.
    #[cfg(target_os = "linux")]
    return match rename_with_flags(source, destination, libc::RENAME_NOREPLACE) {
        Err(error) if unsupported(&error) => rename_if_free(source, destination),
        result => result,
    };
    #[cfg(target_os = "macos")]
    return rename_with_flags(source, destination, libc::RENAME_EXCL);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (source, destination);
        Err(io::Error::new(io::ErrorKind::Unsupported, "exclusive rename is unavailable on this platform"))
    }
}
