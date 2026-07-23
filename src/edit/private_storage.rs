//! User-private storage for edit locks and other process-shared runtime state.

use std::fs::{self, File, OpenOptions};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub(crate) fn edit_lock_directory() -> PathBuf {
    runtime_component("edit-locks")
}

pub(crate) fn job_control_directory() -> PathBuf {
    runtime_component("job-control")
}

pub(crate) fn update_check_directory() -> PathBuf {
    runtime_component("update-check")
}

pub(crate) fn ensure_private_directory(path: &Path, label: &str) -> Result<(), String> {
    validate_private_directory_path(path, label)?;
    #[cfg(unix)]
    {
        unix::ensure_private_directory(path, label)
    }
    #[cfg(windows)]
    {
        windows::ensure_private_directory(path, label)
    }
    #[cfg(not(any(unix, windows)))]
    {
        ensure_private_directory_portable(path, label)
    }
}

fn validate_private_directory_path(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(format!(
            "Cannot use the {label} directory {} because its path is not an absolute, normalized path.",
            crate::paths::display_path(path)
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn ensure_private_directory_portable(path: &Path, label: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| {
        format!(
            "Cannot create the {label} directory {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    verify_directory(path, label)
}

fn inspect_regular_file(path: &Path, label: &str) -> Result<Option<fs::Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
                Err(format!(
                    "Cannot use the {label} path {} because it is not a private regular file. Remove it and retry.",
                    crate::paths::display_path(path)
                ))
            } else {
                Ok(Some(metadata))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "Cannot inspect the {label} path {}: {error}",
            crate::paths::display_path(path)
        )),
    }
}

pub(crate) fn open_lock_file(path: &Path, label: &str) -> Result<File, String> {
    inspect_regular_file(path, label)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    configure_no_follow(&mut options);
    let file = options.open(path).map_err(|error| {
        format!(
            "Cannot open the {label} {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    verify_open_file(&file, path, label)?;
    Ok(file)
}

fn runtime_component(component: &str) -> PathBuf {
    #[cfg(unix)]
    {
        unix::runtime_component(component)
    }
    #[cfg(windows)]
    {
        windows::runtime_component(component)
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::env::temp_dir().join(format!("fastctx-{component}"))
    }
}

pub(super) fn verify_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "Cannot inspect the {label} directory {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    validate_directory_metadata(path, label, &metadata)
}

pub(super) fn validate_directory_metadata(
    path: &Path,
    label: &str,
    metadata: &fs::Metadata,
) -> Result<(), String> {
    if is_reparse_or_symlink(metadata) || !metadata.is_dir() {
        return Err(format!(
            "Cannot use the {label} path {} because it is not a private directory. Remove it and retry.",
            crate::paths::display_path(path)
        ));
    }
    Ok(())
}

fn verify_open_file(file: &File, path: &Path, label: &str) -> Result<(), String> {
    let metadata = file.metadata().map_err(|error| {
        format!(
            "Cannot inspect the open {label} {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(format!(
            "Cannot use the {label} path {} because it is not a private regular file. Remove it and retry.",
            crate::paths::display_path(path)
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn configure_no_follow(options: &mut OpenOptions) {
    unix::configure_no_follow(options);
}

#[cfg(windows)]
fn configure_no_follow(options: &mut OpenOptions) {
    windows::configure_no_follow(options);
}

#[cfg(not(any(unix, windows)))]
fn configure_no_follow(_options: &mut OpenOptions) {}

#[cfg(windows)]
fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    windows::is_reparse_or_symlink(metadata)
}

#[cfg(unix)]
fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    unix::is_reparse_or_symlink(metadata)
}

#[cfg(not(any(unix, windows)))]
fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::{ensure_private_directory, open_lock_file};
    use std::path::Path;

    #[test]
    fn private_directories_and_lock_files_are_regular_and_usable() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("fastctx/runtime/edit-locks");
        ensure_private_directory(&directory, "edit-lock").unwrap();
        let lock = directory.join("test.lock");
        let file = open_lock_file(&lock, "edit lock").unwrap();
        assert!(file.metadata().unwrap().is_file());
    }

    #[test]
    fn relative_or_lexically_escaping_private_directory_paths_are_rejected() {
        let relative =
            ensure_private_directory(Path::new("relative/edit-locks"), "edit-lock").unwrap_err();
        assert!(relative.contains("absolute, normalized path"), "{relative}");

        let temp = tempfile::tempdir().unwrap();
        let escaping = temp.path().join("inside/../outside");
        let error = ensure_private_directory(&escaping, "edit-lock").unwrap_err();
        assert!(error.contains("absolute, normalized path"), "{error}");
        assert!(!temp.path().join("outside").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_entries_are_rejected_instead_of_followed() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("edit-locks");
        ensure_private_directory(&directory, "edit-lock").unwrap();
        let target = temp.path().join("outside");
        std::fs::write(&target, b"outside").unwrap();
        let entry = directory.join("escape.lock");
        symlink(target, &entry).unwrap();
        let error = open_lock_file(&entry, "edit lock").unwrap_err();
        assert!(error.contains("not a private regular file"), "{error}");
    }
}
