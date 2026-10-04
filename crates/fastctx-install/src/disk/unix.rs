use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::SystemTime;

use xattr::FileExt;

use super::Identity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attributes {
    mode: u32,
    uid: u32,
    gid: u32,
    extended: Vec<(OsString, Vec<u8>)>,
}

impl Attributes {
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

pub(crate) fn restore_modified(file: &File, modified: SystemTime) -> io::Result<()> {
    file.set_modified(modified)
}

pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    File::open(path.parent().expect("absolute path"))?.sync_all()
}
