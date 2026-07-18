//! File-identity locks that serialize edits across aliases, processes, and server instances.

use fs2::FileExt;
use sha2::{Digest, Sha256};
#[cfg(any(test, unix))]
use std::fs;
use std::fs::File;
use std::path::Path;

/// Stable identity used for same-file decisions and lock ordering.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PathIdentity {
    material: Vec<u8>,
}

impl PathIdentity {
    /// Resolves an existing file by filesystem identity, or a missing file by parent identity/name.
    #[cfg(test)]
    pub(crate) fn for_path(path: &Path) -> Result<Self, String> {
        match fs::metadata(path) {
            Ok(_) => existing_identity(path).map(|material| Self { material }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing_identity(path).map(|material| Self { material })
            }
            Err(error) => Err(crate::paths::io_error_message(path, &error)),
        }
    }

    /// Returns the stable canonical-parent/name identity regardless of whether the entry exists.
    pub(crate) fn for_name(path: &Path) -> Result<Self, String> {
        missing_identity(path).map(|material| Self { material })
    }

    /// Returns the filesystem identity of an entry that must already exist.
    pub(crate) fn for_existing(path: &Path) -> Result<Self, String> {
        existing_identity(path).map(|material| Self { material })
    }

    fn lock_key(&self) -> String {
        hex::encode(Sha256::digest(&self.material))
    }
}

pub(crate) struct FilePathLock {
    file: File,
}

#[cfg(test)]
pub(crate) fn is_lock_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        error.raw_os_error() == Some(33)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

impl FilePathLock {
    pub(crate) fn acquire(identity: &PathIdentity, path: &Path) -> Result<Self, String> {
        let directory = super::private_storage::edit_lock_directory();
        super::private_storage::ensure_private_directory(&directory, "edit-lock")?;
        let lock_path = directory.join(format!("{}.lock", identity.lock_key()));
        let file = super::private_storage::open_lock_file(&lock_path, "edit lock")?;
        file.lock_exclusive().map_err(|error| {
            format!(
                "Cannot lock {} for editing: {error}. Retry after the other FastCtx process finishes.",
                crate::paths::display_path(path)
            )
        })?;
        Ok(Self { file })
    }

    #[cfg(test)]
    fn try_acquire(identity: &PathIdentity, path: &Path) -> Result<Option<Self>, String> {
        let directory = super::private_storage::edit_lock_directory();
        super::private_storage::ensure_private_directory(&directory, "edit-lock")?;
        let lock_path = directory.join(format!("{}.lock", identity.lock_key()));
        let file = super::private_storage::open_lock_file(&lock_path, "edit lock")?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) if is_lock_contended(&error) => Ok(None),
            Err(error) => Err(format!(
                "Cannot try-lock {} for editing: {error}.",
                crate::paths::display_path(path)
            )),
        }
    }
}

impl Drop for FilePathLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

fn missing_identity(path: &Path) -> Result<Vec<u8>, String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Parent directory does not exist: {}",
            crate::paths::display_path(path)
        )
    })?;
    let parent = crate::paths::canonical_existing(parent).map_err(|_| {
        format!(
            "Parent directory does not exist: {}",
            crate::paths::display_path(parent)
        )
    })?;
    if !parent.is_dir() {
        return Err(format!(
            "Parent directory does not exist: {}",
            crate::paths::display_path(&parent)
        ));
    }
    let file_name = path.file_name().ok_or_else(|| {
        format!(
            "Parent directory does not exist: {}",
            crate::paths::display_path(path)
        )
    })?;
    let mut material = b"missing\0".to_vec();
    material.extend_from_slice(&existing_identity(&parent)?);
    material.push(0);
    append_normalized_file_name(&mut material, file_name);
    Ok(material)
}

#[cfg(unix)]
fn existing_identity(path: &Path) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::MetadataExt;

    let metadata =
        fs::metadata(path).map_err(|error| crate::paths::io_error_message(path, &error))?;
    let mut material = b"unix\0".to_vec();
    material.extend_from_slice(&metadata.dev().to_le_bytes());
    material.extend_from_slice(&metadata.ino().to_le_bytes());
    Ok(material)
}

#[cfg(windows)]
fn existing_identity(path: &Path) -> Result<Vec<u8>, String> {
    use std::mem::MaybeUninit;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
        OPEN_EXISTING,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(crate::paths::io_error_message(
            path,
            &std::io::Error::last_os_error(),
        ));
    }
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let success = unsafe { GetFileInformationByHandle(handle, information.as_mut_ptr()) };
    let error = if success == 0 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    unsafe {
        CloseHandle(handle);
    }
    if let Some(error) = error {
        return Err(crate::paths::io_error_message(path, &error));
    }
    let information = unsafe { information.assume_init() };
    let file_index = ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64;
    let mut material = b"windows\0".to_vec();
    material.extend_from_slice(&information.dwVolumeSerialNumber.to_le_bytes());
    material.extend_from_slice(&file_index.to_le_bytes());
    Ok(material)
}

#[cfg(not(any(unix, windows)))]
fn existing_identity(path: &Path) -> Result<Vec<u8>, String> {
    let canonical = crate::paths::canonical_existing(path)
        .map_err(|error| crate::paths::io_error_message(path, &error))?;
    Ok(format!("other\0{}", canonical.to_string_lossy()).into_bytes())
}

#[cfg(unix)]
fn append_normalized_file_name(material: &mut Vec<u8>, file_name: &std::ffi::OsStr) {
    use std::os::unix::ffi::OsStrExt;
    material.extend_from_slice(file_name.as_bytes());
}

#[cfg(windows)]
fn append_normalized_file_name(material: &mut Vec<u8>, file_name: &std::ffi::OsStr) {
    let normalized = file_name
        .to_string_lossy()
        .to_lowercase()
        .trim_end_matches([' ', '.'])
        .to_string();
    material.extend_from_slice(normalized.as_bytes());
}

#[cfg(not(any(unix, windows)))]
fn append_normalized_file_name(material: &mut Vec<u8>, file_name: &std::ffi::OsStr) {
    material.extend_from_slice(file_name.to_string_lossy().as_bytes());
}

#[cfg(test)]
mod tests {
    use super::{FilePathLock, PathIdentity};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    struct ChildGuard(Option<std::process::Child>);

    impl ChildGuard {
        fn new(child: std::process::Child) -> Self {
            Self(Some(child))
        }

        fn kill_and_wait(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                child.wait().expect("failed to reap lock helper");
            }
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    #[test]
    fn identity_uses_the_file_not_its_spelling() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.txt");
        let alias = temp.path().join("alias.txt");
        std::fs::write(&target, b"identity").unwrap();
        std::fs::hard_link(&target, &alias).unwrap();
        assert_eq!(
            PathIdentity::for_path(&target).unwrap(),
            PathIdentity::for_path(&alias).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let symlink_path = temp.path().join("symlink.txt");
            symlink(&target, &symlink_path).unwrap();
            assert_eq!(
                PathIdentity::for_path(&target).unwrap(),
                PathIdentity::for_path(&symlink_path).unwrap()
            );
        }

        let missing = temp.path().join("New.txt");
        let same_spelling = temp.path().join("New.txt");
        assert_eq!(
            PathIdentity::for_path(&missing).unwrap(),
            PathIdentity::for_path(&same_spelling).unwrap()
        );
        let name_before_create = PathIdentity::for_name(&missing).unwrap();
        std::fs::write(&missing, b"created").unwrap();
        assert_eq!(
            name_before_create,
            PathIdentity::for_name(&missing).unwrap(),
            "the stable parent/name lock must bridge missing to existing"
        );
        #[cfg(windows)]
        {
            assert_eq!(
                name_before_create,
                PathIdentity::for_name(&temp.path().join("new.TXT")).unwrap()
            );
            assert_eq!(
                PathIdentity::for_name(&temp.path().join("name")).unwrap(),
                PathIdentity::for_name(&temp.path().join("name...   ")).unwrap()
            );
        }
    }

    #[test]
    #[ignore]
    fn cross_process_lock_helper() {
        let Some(path) = std::env::var_os("FASTCTX_LOCK_HELPER_PATH") else {
            return;
        };
        let ready = std::env::var_os("FASTCTX_LOCK_HELPER_READY").unwrap();
        let path = PathBuf::from(path);
        let identity = PathIdentity::for_path(&path).unwrap();
        let _guard = FilePathLock::acquire(&identity, &path).unwrap();
        std::fs::write(ready, b"ready").unwrap();
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn cross_process_alias_lock_blocks_and_a_crash_releases_it() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.txt");
        let alias = temp.path().join("alias.txt");
        let ready = temp.path().join("ready");
        std::fs::write(&target, b"locked").unwrap();
        std::fs::hard_link(&target, &alias).unwrap();

        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("edit::locks::tests::cross_process_lock_helper")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("FASTCTX_LOCK_HELPER_PATH", &target)
            .env("FASTCTX_LOCK_HELPER_READY", &ready)
            .spawn()
            .unwrap();
        let mut child = ChildGuard::new(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            ready.exists(),
            "child never acquired the cross-process lock"
        );

        let identity = PathIdentity::for_path(&alias).unwrap();
        assert!(
            FilePathLock::try_acquire(&identity, &alias)
                .unwrap()
                .is_none(),
            "the alias unexpectedly bypassed the held identity lock"
        );

        child.kill_and_wait();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(guard) = FilePathLock::try_acquire(&identity, &alias).unwrap() {
                drop(guard);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the kernel did not release the lock after the holder crashed"
            );
            std::thread::yield_now();
        }
    }
}
