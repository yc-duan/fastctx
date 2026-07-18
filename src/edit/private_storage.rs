//! User-private runtime storage for cross-process replacement locks.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

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
    fs::create_dir_all(path).map_err(|error| {
        format!(
            "Cannot create the {label} directory {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    verify_directory(path, label)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            format!(
                "Cannot secure the {label} directory {}: {error}",
                crate::paths::display_path(path)
            )
        })?;
    }
    #[cfg(windows)]
    secure_windows_managed_chain(path, label)?;
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
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            let runtime = PathBuf::from(runtime);
            if runtime.is_absolute() {
                return runtime.join(format!("fastctx-{component}"));
            }
        }
        let uid = unsafe { libc::geteuid() };
        std::env::temp_dir().join(format!("fastctx-{component}-{uid}"))
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join("AppData").join("Local"))
            })
            .unwrap_or_else(std::env::temp_dir);
        base.join("fastctx").join("runtime").join(component)
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::env::temp_dir().join(format!("fastctx-{component}"))
    }
}

fn verify_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "Cannot inspect the {label} directory {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
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
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW);
}

#[cfg(windows)]
fn configure_no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(any(unix, windows)))]
fn configure_no_follow(_options: &mut OpenOptions) {}

#[cfg(windows)]
fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn secure_windows_managed_chain(path: &Path, label: &str) -> Result<(), String> {
    let mut ancestors = path.ancestors().map(Path::to_path_buf).collect::<Vec<_>>();
    ancestors.reverse();
    let start = ancestors
        .iter()
        .position(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("fastctx"))
        })
        .unwrap_or_else(|| ancestors.len().saturating_sub(1));
    for directory in &ancestors[start..] {
        verify_directory(directory, label)?;
        secure_windows_directory(directory, label)?;
    }
    Ok(())
}

#[cfg(windows)]
fn secure_windows_directory(path: &Path, label: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SetFileSecurityW,
    };

    let descriptor_text = std::ffi::OsStr::new("D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)")
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor_text.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(format!(
            "Cannot build the private ACL for the {label} directory {}: {}",
            crate::paths::display_path(path),
            std::io::Error::last_os_error()
        ));
    }
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let secured = unsafe {
        SetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    let error = (secured == 0).then(std::io::Error::last_os_error);
    unsafe {
        LocalFree(descriptor);
    }
    if let Some(error) = error {
        Err(format!(
            "Cannot secure the {label} directory {}: {error}",
            crate::paths::display_path(path)
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ensure_private_directory, open_lock_file};

    #[test]
    fn private_directories_and_lock_files_are_regular_and_usable() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("fastctx/runtime/edit-locks");
        ensure_private_directory(&directory, "edit-lock").unwrap();
        let lock = directory.join("test.lock");
        let file = open_lock_file(&lock, "edit lock").unwrap();
        assert!(file.metadata().unwrap().is_file());
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
