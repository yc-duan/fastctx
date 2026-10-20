//! Metadata for objects that cannot be opened as ordinary byte streams.

#[cfg(unix)]
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Special {
    mode: u32,
    uid: u32,
    gid: u32,
    modified: (i64, i64),
    target: Option<std::ffi::OsString>,
    extended: Vec<(std::ffi::OsString, Vec<u8>)>,
}

#[cfg(unix)]
impl Special {
    pub fn read(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let metadata = fs::symlink_metadata(path)?;
        let kind = metadata.file_type();
        if !(kind.is_symlink() || kind.is_socket() || kind.is_fifo()) {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "device objects are retained in place"));
        }
        let mut extended = Vec::new();
        for name in xattr::list(path)? {
            extended
                .push((name.clone(), xattr::get(path, name)?.ok_or_else(|| super::conflict("attribute disappeared"))?));
        }
        extended.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Self {
            mode: metadata.mode(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            target: if kind.is_symlink() { Some(fs::read_link(path)?.into_os_string()) } else { None },
            extended,
        })
    }

    pub fn create(&self, _source: &Path, destination: &Path) -> io::Result<()> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let name = CString::new(destination.as_os_str().as_bytes())?;
        if let Some(target) = &self.target {
            std::os::unix::fs::symlink(target, destination)?;
        } else {
            // A retired socket has no stream data. An unbound inode also avoids
            // sockaddr_un's address limit in the longer archive directory.
            // SAFETY: a terminated path; only FIFO/socket modes pass read().
            if unsafe { libc::mknod(name.as_ptr(), self.mode as _, 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let metadata = fs::symlink_metadata(destination)?;
        if metadata.uid() != self.uid || metadata.gid() != self.gid {
            // SAFETY: lchown addresses the new link itself, never its target.
            if unsafe { libc::lchown(name.as_ptr(), self.uid, self.gid) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        for (name, value) in &self.extended {
            xattr::set(destination, name, value)?;
        }
        if self.target.is_none() {
            fs::set_permissions(destination, fs::Permissions::from_mode(self.mode & 0o7777))?;
        }
        let times = [
            libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_OMIT },
            libc::timespec { tv_sec: self.modified.0, tv_nsec: self.modified.1 as _ },
        ];
        // SAFETY: two initialized timestamps; symlinks are not followed.
        if unsafe { libc::utimensat(libc::AT_FDCWD, name.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Special {
    attributes: crate::disk::Attributes,
    modified: std::time::SystemTime,
    directory: bool,
    reparse: Vec<u8>,
}

#[cfg(windows)]
impl Special {
    pub fn read(path: &Path) -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::{IO::DeviceIoControl, Ioctl::FSCTL_GET_REPARSE_POINT};
        let file = crate::disk::open_directory(path)?;
        if !crate::disk::is_link(&file.metadata()?) {
            return Err(super::conflict("reparse point was replaced"));
        }
        // MAXIMUM_REPARSE_DATA_BUFFER_SIZE is the filesystem control ABI limit.
        let mut reparse = vec![0u8; 16 * 1024];
        let mut used = 0;
        // SAFETY: a no-follow handle and an output buffer of the documented size.
        if unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_GET_REPARSE_POINT,
                std::ptr::null(),
                0,
                reparse.as_mut_ptr().cast(),
                reparse.len() as u32,
                &mut used,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        reparse.truncate(used as usize);
        Ok(Self {
            attributes: crate::disk::Attributes::read(&file, path)?,
            modified: file.metadata()?.modified()?,
            directory: file.metadata()?.is_dir(),
            reparse,
        })
    }

    pub fn create(&self, source: &Path, destination: &Path) -> io::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::*;
        let from: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
        let tag = self.reparse.get(..4).map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")));
        // File symlinks require COPY_SYMLINK. OPEN_AND_COPY_REPARSE_POINT by
        // itself follows file symlinks on Windows 11 (verified 2026-10-04).
        let reparse_flags = match (self.directory, tag) {
            (false, Some(0xa000000c)) => COPY_FILE_COPY_SYMLINK,
            (true, Some(0xa000000c | 0xa0000003)) => COPY_FILE_DIRECTORY | COPY_FILE_OPEN_AND_COPY_REPARSE_POINT,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unrecognized Windows reparse type retained at its original location",
                ));
            }
        };
        let parameters = COPYFILE2_EXTENDED_PARAMETERS {
            dwSize: std::mem::size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32,
            dwCopyFlags: COPY_FILE_FAIL_IF_EXISTS | reparse_flags,
            ..Default::default()
        };
        // SAFETY: CopyFile2 copies the reparse object, its raw tag and streams;
        // it never traverses the link. No downgrade to a different link type.
        let result = unsafe { CopyFile2(from.as_ptr(), to.as_ptr(), &parameters) };
        if result < 0 {
            return Err(io::Error::other(format!("CopyFile2 reparse copy failed: HRESULT {result:#x}")));
        }
        let target = super::directory_for_attributes(destination)?;
        self.attributes.protect(&target)?;
        self.attributes.apply_reparse_header(&target)?;
        crate::disk::restore_modified(&target, self.modified)
    }
}

pub(super) fn delete_special(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        super::delete_copied(&crate::disk::open_directory(path)?, path)
    }
    #[cfg(unix)]
    {
        fs::remove_file(path)
    }
}

pub(super) fn mount(path: &Path) -> io::Result<u64> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(path.as_os_str().as_bytes())?;
        // 2026-10-20: device IDs cannot distinguish a bind mount from its source.
        let fallback = || fastctx_platform::mountinfo::id(path);
        // SAFETY: initialized output and terminated path, queried without following links.
        let mut data: libc::statx = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::syscall(
                libc::SYS_statx,
                libc::AT_FDCWD,
                name.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
                libc::STATX_MNT_ID,
                &mut data,
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ENOSYS | libc::EINVAL | libc::EPERM) => fallback(),
                _ => Err(error),
            };
        }
        if data.stx_mask & libc::STATX_MNT_ID == 0 {
            return fallback();
        }
        Ok(data.stx_mnt_id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(super::Object::read(path)?.ok_or_else(|| super::conflict("source disappeared"))?.identity.volume)
    }
}
