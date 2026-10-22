//! Publishing a file whole.
//!
//! Readers see the old bytes or the new ones, never a mix, and a crash
//! leaves at worst a stray temporary file beside the target. The bytes are
//! written to a temporary file in the target's directory, flushed to disk,
//! and moved over the target in one step. On Windows the move uses POSIX
//! semantics, so readers that share delete access can keep reading the old
//! bytes. Windows handles that deny delete sharing can still cause a sharing
//! violation; that failure is returned without truncating the target. On Unix
//! a file root publishes into a directory another account owns becomes that
//! account's, as the files around it are: a hub's service runs as root over a
//! hub kept in its user's World, and the user's own commands read its records.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Who may read and write a published file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permissions {
    /// The owner alone: the World's own records. On Windows the file takes
    /// the access of its directory, which the World keeps private.
    Private,
    /// Whatever the file being replaced had; a new file gets what the
    /// platform gives new files. For files that belong to the user.
    KeepExisting,
}

/// Replaces `path` with `bytes`, or creates it.
pub fn publish(path: &Path, bytes: &[u8], permissions: Permissions) -> io::Result<()> {
    publish_checked(path, bytes, permissions, true, || Ok(())).map_err(|failure| failure.error)
}

/// Creates `path` with `bytes`, failing with `AlreadyExists` if something is
/// there already: for records that are written once and never change.
pub fn publish_new(path: &Path, bytes: &[u8], permissions: Permissions) -> io::Result<()> {
    publish_checked(path, bytes, permissions, false, || Ok(())).map_err(|failure| failure.error)
}

/// A publication failure distinguishes a failed replacement from a failure to
/// make an already-visible replacement durable. Callers must record the latter.
#[derive(Debug)]
pub struct PublishFailure {
    pub error: io::Error,
    pub published: bool,
}

/// Stages and flushes the complete new file, then verifies the expected revision
/// immediately before publication. The caller must serialize cooperating writers;
/// operating systems do not offer a portable content-based compare-and-rename.
/// A guard returned by `verify` remains held through publication and its durability check.
pub fn publish_checked<G>(
    path: &Path,
    bytes: &[u8],
    permissions: Permissions,
    replace: bool,
    verify: impl FnOnce() -> io::Result<G>,
) -> Result<(), PublishFailure> {
    let mut published = false;
    imp::publish(path, bytes, permissions, replace, verify, &mut published)
        .map_err(|error| PublishFailure { error, published })
}

/// A file written piece by piece beside its target and moved over it whole
/// once complete, for contents too large to hold in memory. Until then the
/// target is untouched; dropped uncommitted, the staged file is removed.
pub struct Staged {
    inner: imp::Staged,
}

impl Staged {
    /// Starts staging a new version of `target`. On Unix the file gets
    /// `mode`, or what the platform gives new files.
    pub fn create(target: &Path, mode: Option<u32>) -> io::Result<Self> {
        Ok(Self { inner: imp::Staged::create(target, temporary_beside(target)?, mode)? })
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_all(bytes)
    }

    /// Goes on staging `target` in `partial`, a file beside it that an
    /// earlier try left: what it holds stays and what is written is added
    /// after it, or it is made when there is none. Dropped uncommitted, it
    /// is kept for the next try (`world-network.md`, 断开不是结束); returns
    /// how many bytes it held already.
    pub fn resume(target: &Path, partial: PathBuf, mode: Option<u32>) -> io::Result<(Self, u64)> {
        let (inner, held) = imp::Staged::resume(target, partial, mode)?;
        Ok((Self { inner }, held))
    }

    /// Flushes the contents to disk and moves them over the target.
    pub fn commit(self) -> io::Result<()> {
        self.inner.commit()
    }

    /// Removes what was staged, kept for another try or not.
    pub fn discard(self) {
        self.inner.discard();
    }
}

/// A name beside `path` that no other writer, in this process or another,
/// uses at the same moment; it starts with a dot so listings pass over it.
fn temporary_beside(path: &Path) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    path.file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{} names no file", path.display())))?;
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    // The target may already use the filesystem's maximum name length; adding
    // a suffix to it would make an otherwise valid target impossible to publish.
    Ok(path.with_file_name(format!(".fastctx-{}-{sequence}.tmp", std::process::id())))
}

#[cfg(unix)]
mod imp {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Write as _};
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    use std::path::Path;

    use super::{Permissions, temporary_beside};

    pub fn publish<G>(
        path: &Path,
        bytes: &[u8],
        permissions: Permissions,
        replace: bool,
        verify: impl FnOnce() -> io::Result<G>,
        published: &mut bool,
    ) -> io::Result<()> {
        let temporary = temporary_beside(path)?;
        // 0666 lets the umask decide for new user files, as any program's
        // new file would; private records never pass through a wider mode.
        let creation_mode = match permissions {
            Permissions::Private => 0o600,
            Permissions::KeepExisting => 0o666,
        };
        let mut file = OpenOptions::new().write(true).create_new(true).mode(creation_mode).open(&temporary)?;
        let result = (|| {
            file.write_all(bytes)?;
            if permissions == Permissions::KeepExisting
                && let Ok(existing) = fs::metadata(path)
            {
                file.set_permissions(fs::Permissions::from_mode(existing.permissions().mode() & 0o7777))?;
            }
            owned_as_directory(&file, path)?;
            file.sync_all()?;
            let _authority = verify()?;
            if replace {
                fs::rename(&temporary, path)?;
                *published = true;
            } else {
                #[cfg(target_os = "android")]
                {
                    rename_new(&temporary, path)?;
                    *published = true;
                }
                #[cfg(not(target_os = "android"))]
                {
                    // A hard link fails if the target exists, which a rename
                    // would silently overwrite.
                    fs::hard_link(&temporary, path)?;
                    *published = true;
                    fs::remove_file(&temporary)?;
                }
            }
            sync_directory(path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    #[cfg(target_os = "android")]
    fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let from = CString::new(from.as_os_str().as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let to = CString::new(to.as_os_str().as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        // Android app SELinux policy forbids hard links, including between
        // the app's own files. The kernel's no-replace rename retains atomic
        // publication and collision detection without requiring that permission.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    pub struct Staged {
        file: Option<File>,
        temporary: std::path::PathBuf,
        target: std::path::PathBuf,
        /// Kept when dropped uncommitted, for another try.
        kept: bool,
    }

    impl Staged {
        pub fn create(target: &Path, temporary: std::path::PathBuf, mode: Option<u32>) -> io::Result<Self> {
            let file = OpenOptions::new().write(true).create_new(true).mode(mode.unwrap_or(0o666)).open(&temporary)?;
            if let Some(mode) = mode {
                // The umask narrowed the creation mode; the mode asked for is
                // the source's own.
                file.set_permissions(fs::Permissions::from_mode(mode & 0o7777))?;
            }
            Ok(Self { file: Some(file), temporary, target: target.to_path_buf(), kept: false })
        }

        pub fn resume(target: &Path, partial: std::path::PathBuf, mode: Option<u32>) -> io::Result<(Self, u64)> {
            let file = OpenOptions::new().append(true).create(true).mode(mode.unwrap_or(0o666)).open(&partial)?;
            if let Some(mode) = mode {
                file.set_permissions(fs::Permissions::from_mode(mode & 0o7777))?;
            }
            let held = file.metadata()?.len();
            Ok((Self { file: Some(file), temporary: partial, target: target.to_path_buf(), kept: true }, held))
        }

        pub fn discard(mut self) {
            if self.file.take().is_some() {
                let _ = fs::remove_file(&self.temporary);
            }
        }

        pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.file.as_mut().expect("open until committed").write_all(bytes)
        }

        pub fn commit(mut self) -> io::Result<()> {
            let file = self.file.take().expect("open until committed");
            owned_as_directory(&file, &self.target)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&self.temporary, &self.target)?;
            sync_directory(&self.target)
        }
    }

    impl Drop for Staged {
        fn drop(&mut self) {
            if self.file.is_some() && !self.kept {
                let _ = fs::remove_file(&self.temporary);
            }
        }
    }

    /// Makes the new directory entry itself durable.
    fn sync_directory(path: &Path) -> io::Result<()> {
        match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => File::open(parent)?.sync_all(),
            _ => File::open(".")?.sync_all(),
        }
    }

    /// Gives `file`, about to become `target`, the owner of the directory it
    /// goes into when root writes it there for another account.
    fn owned_as_directory(file: &File, target: &Path) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt as _;
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } != 0 {
            return Ok(());
        }
        let directory = match target.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => fs::metadata(parent)?,
            _ => fs::metadata(".")?,
        };
        if directory.uid() == 0 {
            return Ok(());
        }
        std::os::unix::fs::fchown(file, Some(directory.uid()), Some(directory.gid()))
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::io;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_DISPOSITION_INFO, FILE_END, FILE_RENAME_INFO,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo, FileRenameInfoEx, FlushFileBuffers,
        OPEN_ALWAYS, SetFileInformationByHandle, SetFilePointerEx, WriteFile,
    };

    use super::{Permissions, temporary_beside};

    const FILE_RENAME_FLAG_REPLACE_IF_EXISTS: u32 = 0x1;
    const FILE_RENAME_FLAG_POSIX_SEMANTICS: u32 = 0x2;

    pub fn publish<G>(
        path: &Path,
        bytes: &[u8],
        _permissions: Permissions,
        replace: bool,
        verify: impl FnOnce() -> io::Result<G>,
        published: &mut bool,
    ) -> io::Result<()> {
        let temporary = temporary_beside(path)?;
        let target = std::path::absolute(path)?;
        let handle = Handle(create_temporary(&temporary)?);
        let result = (|| {
            write_all(handle.0, bytes)?;
            // SAFETY: a valid handle opened for writing.
            if unsafe { FlushFileBuffers(handle.0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let _authority = verify()?;
            rename_by_handle(handle.0, &target, replace)?;
            *published = true;
            Ok(())
        })();
        if result.is_err() {
            delete_on_close(handle.0);
        }
        drop(handle);
        result
    }

    pub struct Staged {
        handle: Option<Handle>,
        target: std::path::PathBuf,
        /// Kept when dropped uncommitted, for another try.
        kept: bool,
    }

    impl Staged {
        pub fn create(target: &Path, temporary: std::path::PathBuf, _mode: Option<u32>) -> io::Result<Self> {
            let target = std::path::absolute(target)?;
            Ok(Self { handle: Some(Handle(create_temporary(&temporary)?)), target, kept: false })
        }

        pub fn resume(target: &Path, partial: std::path::PathBuf, _mode: Option<u32>) -> io::Result<(Self, u64)> {
            let target = std::path::absolute(target)?;
            let handle = Handle(open_partial(&partial)?);
            let mut end = 0i64;
            // SAFETY: a valid handle; the new position is written to `end`.
            if unsafe { SetFilePointerEx(handle.0, 0, &mut end, FILE_END) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok((Self { handle: Some(handle), target, kept: true }, end as u64))
        }

        pub fn discard(mut self) {
            if let Some(handle) = self.handle.take() {
                delete_on_close(handle.0);
            }
        }

        pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            write_all(self.handle.as_ref().expect("open until committed").0, bytes)
        }

        pub fn commit(mut self) -> io::Result<()> {
            let handle = self.handle.as_ref().expect("open until committed").0;
            // SAFETY: a valid handle opened for writing.
            if unsafe { FlushFileBuffers(handle) } == 0 {
                return Err(io::Error::last_os_error());
            }
            rename_by_handle(handle, &self.target, true)?;
            drop(self.handle.take());
            Ok(())
        }
    }

    impl Drop for Staged {
        fn drop(&mut self) {
            if let Some(handle) = &self.handle
                && !self.kept
            {
                delete_on_close(handle.0);
            }
        }
    }

    struct Handle(HANDLE);

    // SAFETY: the handle is owned by this value alone; a file handle may be
    // used and closed from any thread.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle is owned here and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain([0]).collect()
    }

    fn create_temporary(path: &Path) -> io::Result<HANDLE> {
        let name = wide(path.as_os_str());
        // DELETE access is what lets the handle rename itself over the target.
        // SAFETY: a NUL-terminated path and no security attributes.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE | DELETE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(handle)
    }

    /// Opens what an earlier try staged at `path`, or makes it, to write on
    /// at its end; with DELETE access, as `create_temporary`'s.
    fn open_partial(path: &Path) -> io::Result<HANDLE> {
        let name = wide(path.as_os_str());
        // SAFETY: a NUL-terminated path and no security attributes.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE | DELETE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(handle)
    }

    fn write_all(handle: HANDLE, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let chunk = bytes.len().min(u32::MAX as usize) as u32;
            let mut written = 0u32;
            // SAFETY: the buffer outlives the call; no overlapped I/O.
            if unsafe { WriteFile(handle, bytes.as_ptr(), chunk, &mut written, ptr::null_mut()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if written == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "temporary file write made no progress"));
            }
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }

    /// Moves the open file to `target` with POSIX semantics: the old file's
    /// readers keep their bytes, and the name switches in one step.
    fn rename_by_handle(handle: HANDLE, target: &Path, replace: bool) -> io::Result<()> {
        // The Win32 layer reads the name up to its NUL even though the length
        // is given; without the NUL the new name gains whatever follows.
        let name = wide(target.as_os_str());
        let name_bytes = (name.len() - 1) * size_of::<u16>();
        let header = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
        let total = header + (name.len() * size_of::<u16>());
        let mut buffer = vec![0u64; total.div_ceil(size_of::<u64>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        let flags = FILE_RENAME_FLAG_POSIX_SEMANTICS | if replace { FILE_RENAME_FLAG_REPLACE_IF_EXISTS } else { 0 };
        // SAFETY: the buffer is zeroed, aligned for FILE_RENAME_INFO, and
        // large enough for the header and the NUL-terminated name.
        unsafe {
            (*info).Anonymous.Flags = flags;
            (*info).RootDirectory = ptr::null_mut();
            (*info).FileNameLength = u32::try_from(name_bytes).map_err(|_| io::Error::other("path too long"))?;
            ptr::copy_nonoverlapping(name.as_ptr(), (&raw mut (*info).FileName).cast::<u16>(), name.len());
            if SetFileInformationByHandle(handle, FileRenameInfoEx, info.cast(), total as u32) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    fn delete_on_close(handle: HANDLE) {
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: a valid handle opened with DELETE access.
        unsafe {
            SetFileInformationByHandle(
                handle,
                FileDispositionInfo,
                (&raw const info).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            );
        }
    }
}
