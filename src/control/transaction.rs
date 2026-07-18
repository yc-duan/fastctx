//! Atomic cross-file writes, conflict detection, and in-memory rollback.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Desired state of one transaction target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileAction {
    /// Replace or create the target with the supplied bytes.
    Write(Vec<u8>),
    /// Delete the target.
    Delete,
}

/// One file change frozen during preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileChange {
    /// Absolute target path.
    pub target: PathBuf,
    /// Original bytes read during preview, or `None` when the target was absent.
    pub original: Option<Vec<u8>>,
    /// New state to commit after confirmation.
    pub action: FileAction,
    /// Unix mode for a new file; existing files normally retain their original mode.
    pub unix_mode: Option<u32>,
    /// Allow the Windows rename-old update path for a running binary.
    pub locked_binary_fallback: bool,
}

impl FileChange {
    /// Returns whether this target would actually change.
    pub fn is_changed(&self) -> bool {
        match &self.action {
            FileAction::Write(bytes) => self.original.as_deref() != Some(bytes.as_slice()),
            FileAction::Delete => self.original.is_some(),
        }
    }
}

/// Reads a target snapshot and rejects targets whose symlink entry would be replaced.
pub fn read_snapshot(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Cannot inspect {}: {error}",
                crate::paths::display_path(path)
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "Refusing to replace symbolic link {}. Point fastctx at a regular file or update it manually.",
            crate::paths::display_path(path)
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Cannot replace {} because it is not a regular file.",
            crate::paths::display_path(path)
        ));
    }
    fs::read(path)
        .map(Some)
        .map_err(|error| format!("Cannot read {}: {error}", crate::paths::display_path(path)))
}

/// Returns the Unix mode of an existing file, or `None` on other platforms.
pub fn existing_unix_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .ok()
            .map(|metadata| metadata.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Commits every change produced by the same immutable preview.
pub fn commit(changes: &[FileChange]) -> Result<(), String> {
    commit_with_hook(changes, |_| {})
}

/// Revalidates an immutable preview without writing any target.
///
/// Unapply uses this before it performs its intentional process-termination step, so a stale
/// preview or a deterministic path/permission failure cannot kill jobs and only then reject the
/// configuration transaction.
pub fn validate(changes: &[FileChange]) -> Result<(), String> {
    let changes = changes
        .iter()
        .filter(|change| change.is_changed())
        .collect::<Vec<_>>();
    validate_changes(&changes)
}

fn commit_with_hook(
    changes: &[FileChange],
    mut before_apply: impl FnMut(usize),
) -> Result<(), String> {
    let changes = changes
        .iter()
        .filter(|change| change.is_changed())
        .collect::<Vec<_>>();
    if changes.is_empty() {
        return Ok(());
    }

    validate_changes(&changes)?;

    let mut created_directories = Vec::new();
    for change in &changes {
        let parent = change.target.parent().ok_or_else(|| {
            format!(
                "Cannot determine the parent directory for {}.",
                crate::paths::display_path(&change.target)
            )
        })?;
        create_parent_chain(parent, &mut created_directories)?;
    }

    // There are no file-level backups: same-directory atomic replacement is rolled back from
    // change.original in memory after a mid-transaction failure (2026-07-12).
    let mut committed: Vec<&FileChange> = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        before_apply(index);
        let result = read_snapshot(&change.target).and_then(|current| {
            if current != change.original {
                Err(format!(
                    "{} changed while the transaction was in progress. The pending write was refused.",
                    crate::paths::display_path(&change.target)
                ))
            } else {
                apply_change(change)
            }
        });
        if let Err(error) = result {
            let mut rollback_errors = Vec::new();
            for applied in committed.iter().rev() {
                if let Err(rollback_error) = restore_change(applied) {
                    rollback_errors.push(rollback_error);
                }
            }
            cleanup_directories(&created_directories);
            if rollback_errors.is_empty() {
                return Err(format!(
                    "Cannot update {}: {error}. Earlier writes were rolled back.",
                    crate::paths::display_path(&change.target)
                ));
            }
            return Err(format!(
                "Cannot update {}: {error}. Rollback also failed: {}.",
                crate::paths::display_path(&change.target),
                rollback_errors.join("; ")
            ));
        }
        committed.push(*change);
    }

    Ok(())
}

fn validate_changes(changes: &[&FileChange]) -> Result<(), String> {
    for change in changes {
        let current = read_snapshot(&change.target)?;
        if current != change.original {
            return Err(format!(
                "{} changed after the preview. No files were written; preview again and retry.",
                crate::paths::display_path(&change.target)
            ));
        }
        if current.is_some() {
            ensure_target_writable(&change.target)?;
        }
        let parent = change.target.parent().ok_or_else(|| {
            format!(
                "Cannot determine the parent directory for {}.",
                crate::paths::display_path(&change.target)
            )
        })?;
        validate_parent_chain(parent)?;
    }
    Ok(())
}

fn validate_parent_chain(path: &Path) -> Result<(), String> {
    let mut cursor = path;
    while !cursor.exists() {
        cursor = cursor.parent().ok_or_else(|| {
            format!(
                "Cannot find an existing parent for {}.",
                crate::paths::display_path(path)
            )
        })?;
    }
    if !cursor.is_dir() {
        return Err(format!(
            "Cannot create files under {} because an ancestor is not a directory.",
            crate::paths::display_path(path)
        ));
    }
    Ok(())
}

fn ensure_target_writable(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "Cannot inspect permissions for {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    if metadata.permissions().readonly() {
        return Err(format!(
            "Cannot update read-only file {}. Make it writable and retry; no files were written.",
            crate::paths::display_path(path)
        ));
    }
    Ok(())
}

fn apply_change(change: &FileChange) -> Result<(), String> {
    match &change.action {
        FileAction::Write(bytes) => atomic_replace(
            &change.target,
            bytes,
            change.unix_mode,
            change.locked_binary_fallback,
        ),
        FileAction::Delete => match fs::remove_file(&change.target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        },
    }
}

fn restore_change(change: &FileChange) -> Result<(), String> {
    let current = read_snapshot(&change.target)?;
    let expected = match &change.action {
        FileAction::Write(bytes) => Some(bytes.as_slice()),
        FileAction::Delete => None,
    };
    if current.as_deref() != expected {
        return Err(format!(
            "refusing to roll back {} because it changed after fastctx wrote it",
            crate::paths::display_path(&change.target)
        ));
    }
    match &change.original {
        Some(bytes) => atomic_replace(
            &change.target,
            bytes,
            change.unix_mode,
            change.locked_binary_fallback,
        ),
        None => match fs::remove_file(&change.target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "cannot remove newly-created {}: {error}",
                crate::paths::display_path(&change.target)
            )),
        },
    }
}

/// Writes a temporary file beside the target and atomically replaces the target.
pub fn atomic_replace(
    target: &Path,
    bytes: &[u8],
    unix_mode: Option<u32>,
    locked_binary_fallback: bool,
) -> Result<(), String> {
    let parent = target.parent().ok_or_else(|| {
        format!(
            "Cannot determine the parent directory for {}.",
            crate::paths::display_path(target)
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Cannot create {}: {error}",
            crate::paths::display_path(parent)
        )
    })?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let filename = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("fastctx");
    let temporary = parent.join(format!(
        ".{filename}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    let write_result = write_temporary(&temporary, bytes, unix_mode);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    let replace_result = replace_path(&temporary, target, locked_binary_fallback);
    if let Err(error) = replace_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn write_temporary(path: &Path, bytes: &[u8], unix_mode: Option<u32>) -> Result<(), String> {
    #[cfg(not(unix))]
    let _ = unix_mode;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(unix_mode.unwrap_or(0o600));
    }
    let mut file = options.open(path).map_err(|error| {
        format!(
            "Cannot create temporary file {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    #[cfg(unix)]
    if let Some(mode) = unix_mode {
        use std::os::unix::fs::PermissionsExt;
        // OpenOptionsExt::mode is filtered by umask; restore the exact existing mode before publish.
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|error| {
                format!(
                    "Cannot preserve permissions on temporary file {}: {error}",
                    crate::paths::display_path(path)
                )
            })?;
    }
    file.write_all(bytes).map_err(|error| {
        format!(
            "Cannot write temporary file {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    file.sync_all().map_err(|error| {
        format!(
            "Cannot sync temporary file {}: {error}",
            crate::paths::display_path(path)
        )
    })
}

fn create_parent_chain(path: &Path, created: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.exists() {
        if path.is_dir() {
            return Ok(());
        }
        return Err(format!(
            "Cannot create files under {} because it is not a directory.",
            crate::paths::display_path(path)
        ));
    }
    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor.parent().ok_or_else(|| {
            format!(
                "Cannot find an existing parent for {}.",
                crate::paths::display_path(path)
            )
        })?;
    }
    if !cursor.is_dir() {
        return Err(format!(
            "Cannot create files under {} because an ancestor is not a directory.",
            crate::paths::display_path(path)
        ));
    }
    for directory in missing.iter().rev() {
        fs::create_dir(directory).map_err(|error| {
            format!(
                "Cannot create directory {}: {error}",
                crate::paths::display_path(directory)
            )
        })?;
        created.push(directory.clone());
    }
    Ok(())
}

fn cleanup_directories(created: &[PathBuf]) {
    for directory in created.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
}

#[cfg(unix)]
fn replace_path(temporary: &Path, target: &Path, _locked_binary: bool) -> Result<(), String> {
    fs::rename(temporary, target).map_err(|error| {
        format!(
            "Cannot atomically replace {}: {error}",
            crate::paths::display_path(target)
        )
    })
}

#[cfg(windows)]
fn replace_path(temporary: &Path, target: &Path, locked_binary: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH, ReplaceFileW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let temporary_wide = wide(temporary);
    let target_wide = wide(target);
    if !target.exists() {
        let moved = unsafe {
            MoveFileExW(
                temporary_wide.as_ptr(),
                target_wide.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        };
        if moved != 0 {
            return Ok(());
        }
        return Err(format!(
            "Cannot atomically create {}: {}",
            crate::paths::display_path(target),
            io::Error::last_os_error()
        ));
    }

    let replaced = unsafe {
        ReplaceFileW(
            target_wide.as_ptr(),
            temporary_wide.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced != 0 {
        return Ok(());
    }
    let replace_error = io::Error::last_os_error();
    if !locked_binary || !matches!(replace_error.raw_os_error(), Some(5 | 32 | 33)) {
        return Err(format!(
            "Cannot atomically replace {}: {replace_error}",
            crate::paths::display_path(target)
        ));
    }

    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let filename = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("fastctx.exe");
    let old = target.with_file_name(format!(
        ".{filename}.fastctx-old-{}.{}",
        std::process::id(),
        sequence
    ));
    let old_wide = wide(&old);
    let renamed = unsafe {
        MoveFileExW(
            target_wide.as_ptr(),
            old_wide.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if renamed == 0 {
        return Err(format!(
            "Cannot move the running binary aside at {}: {}",
            crate::paths::display_path(target),
            io::Error::last_os_error()
        ));
    }
    let installed = unsafe {
        MoveFileExW(
            temporary_wide.as_ptr(),
            target_wide.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if installed == 0 {
        let install_error = io::Error::last_os_error();
        let restored = unsafe {
            MoveFileExW(
                old_wide.as_ptr(),
                target_wide.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        };
        if restored == 0 {
            return Err(format!(
                "Cannot install {}: {install_error}; restoring the previous binary also failed: {}",
                crate::paths::display_path(target),
                io::Error::last_os_error()
            ));
        }
        return Err(format!(
            "Cannot install {}: {install_error}; the previous binary was restored.",
            crate::paths::display_path(target)
        ));
    }
    let _ = fs::remove_file(old);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::atomic_replace;
    use super::{FileAction, FileChange, commit, commit_with_hook, read_snapshot};

    #[test]
    fn a_conflict_after_preview_refuses_every_write() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();
        let first_original = read_snapshot(&first).unwrap();
        let second_original = read_snapshot(&second).unwrap();
        let changes = vec![
            FileChange {
                target: first.clone(),
                original: first_original,
                action: FileAction::Write(b"ONE".to_vec()),
                unix_mode: None,
                locked_binary_fallback: false,
            },
            FileChange {
                target: second.clone(),
                original: second_original,
                action: FileAction::Write(b"TWO".to_vec()),
                unix_mode: None,
                locked_binary_fallback: false,
            },
        ];
        std::fs::write(&second, b"user edit").unwrap();

        let error = commit(&changes).unwrap_err();
        assert!(error.contains("changed after the preview"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"user edit");
    }

    #[test]
    fn a_change_between_two_writes_is_detected_and_the_first_write_rolls_back() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();
        let changes = vec![
            FileChange {
                target: first.clone(),
                original: read_snapshot(&first).unwrap(),
                action: FileAction::Write(b"ONE".to_vec()),
                unix_mode: None,
                locked_binary_fallback: false,
            },
            FileChange {
                target: second.clone(),
                original: read_snapshot(&second).unwrap(),
                action: FileAction::Write(b"TWO".to_vec()),
                unix_mode: None,
                locked_binary_fallback: false,
            },
        ];

        let error = commit_with_hook(&changes, |index| {
            if index == 1 {
                std::fs::write(&second, b"user edit during commit").unwrap();
            }
        })
        .unwrap_err();
        assert!(error.contains("changed while the transaction was in progress"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"user edit during commit");
    }

    #[cfg(windows)]
    #[test]
    fn replace_file_w_preserves_existing_windows_file_attributes() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_HIDDEN, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
        };

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hidden.txt");
        std::fs::write(&path, b"old").unwrap();
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let original = unsafe { GetFileAttributesW(wide.as_ptr()) };
        assert_ne!(original, INVALID_FILE_ATTRIBUTES);
        assert_ne!(
            unsafe { SetFileAttributesW(wide.as_ptr(), original | FILE_ATTRIBUTE_HIDDEN) },
            0
        );

        atomic_replace(&path, b"new", None, false).unwrap();
        let replaced = unsafe { GetFileAttributesW(wide.as_ptr()) };
        assert_ne!(replaced, INVALID_FILE_ATTRIBUTES);
        assert_ne!(replaced & FILE_ATTRIBUTE_HIDDEN, 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");

        assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), original) }, 0);
    }
}
