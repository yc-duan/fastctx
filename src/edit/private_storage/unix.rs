//! Unix private-directory creation and no-follow enforcement.

use std::ffi::{CString, OsString};
use std::fs::{self, File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(super) fn runtime_component(component: &str) -> PathBuf {
    resolve_runtime_component(
        component,
        std::env::var_os("XDG_RUNTIME_DIR"),
        std::env::temp_dir(),
    )
}

fn resolve_runtime_component(
    component: &str,
    xdg_runtime_dir: Option<OsString>,
    temp_dir: PathBuf,
) -> PathBuf {
    if let Some(runtime) = xdg_runtime_dir {
        let runtime = PathBuf::from(runtime);
        if runtime.is_absolute() {
            return runtime.join(format!("fastctx-{component}"));
        }
    }
    let base = if temp_dir.is_absolute() {
        temp_dir
    } else {
        PathBuf::from("/tmp")
    };
    let uid = unsafe { libc::geteuid() };
    base.join(format!("fastctx-{component}-{uid}"))
}

pub(super) fn ensure_private_directory(path: &Path, label: &str) -> Result<(), String> {
    let chain = managed_chain(path);
    let root = chain.first().expect("managed chains are never empty");
    let parent_path = root.parent().ok_or_else(|| {
        format!(
            "Cannot create the {label} directory {} because its managed root has no parent.",
            crate::paths::display_path(path)
        )
    })?;

    let mut parent_options = OpenOptions::new();
    parent_options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC);
    let root_parent = parent_options.open(parent_path).map_err(|error| {
        format!(
            "Cannot open the parent of the {label} directory {}: {error}",
            crate::paths::display_path(parent_path)
        )
    })?;

    let mut opened_chain: Vec<File> = Vec::with_capacity(chain.len());
    for directory_path in &chain {
        let name = directory_path.file_name().ok_or_else(|| {
            format!(
                "Cannot create the {label} directory {} because a managed component has no name.",
                crate::paths::display_path(directory_path)
            )
        })?;
        let name = CString::new(name.as_bytes()).map_err(|_| {
            format!(
                "Cannot create the {label} directory {} because a path component contains a NUL byte.",
                crate::paths::display_path(directory_path)
            )
        })?;
        let parent_fd = opened_chain
            .last()
            .map_or(root_parent.as_raw_fd(), AsRawFd::as_raw_fd);
        let created = unsafe { libc::mkdirat(parent_fd, name.as_ptr(), 0o700) };
        if created != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(format!(
                    "Cannot create the {label} directory {}: {error}",
                    crate::paths::display_path(directory_path)
                ));
            }
        }

        super::verify_directory(directory_path, label)?;
        let fd = unsafe {
            libc::openat(
                parent_fd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(format!(
                "Cannot securely open the {label} directory {}: {}",
                crate::paths::display_path(directory_path),
                std::io::Error::last_os_error()
            ));
        }
        let directory = unsafe { File::from_raw_fd(fd) };
        let opened = directory.metadata().map_err(|error| {
            format!(
                "Cannot inspect the open {label} directory {}: {error}",
                crate::paths::display_path(directory_path)
            )
        })?;
        if !opened.is_dir() {
            return Err(format!(
                "Cannot use the {label} path {} because it is not a private directory. Remove it and retry.",
                crate::paths::display_path(directory_path)
            ));
        }
        directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| {
                format!(
                    "Cannot secure the {label} directory {}: {error}",
                    crate::paths::display_path(directory_path)
                )
            })?;

        let secured = directory.metadata().map_err(|error| {
            format!(
                "Cannot verify the secured {label} directory {}: {error}",
                crate::paths::display_path(directory_path)
            )
        })?;
        if secured.permissions().mode() & 0o777 != 0o700 {
            return Err(format!(
                "Cannot secure the {label} directory {} because its permissions are not owner-only after chmod.",
                crate::paths::display_path(directory_path)
            ));
        }

        let current = fs::symlink_metadata(directory_path).map_err(|error| {
            format!(
                "Cannot re-inspect the {label} directory {} after securing it: {error}",
                crate::paths::display_path(directory_path)
            )
        })?;
        super::validate_directory_metadata(directory_path, label, &current)?;
        if opened.dev() != secured.dev()
            || opened.ino() != secured.ino()
            || secured.dev() != current.dev()
            || secured.ino() != current.ino()
        {
            return Err(format!(
                "Cannot secure the {label} directory {} because it changed while permissions were applied. Retry after the path stops changing.",
                crate::paths::display_path(directory_path)
            ));
        }
        opened_chain.push(directory);
    }
    Ok(())
}

fn managed_chain(path: &Path) -> Vec<PathBuf> {
    let root = path
        .ancestors()
        .find_map(|ancestor| {
            let parent = ancestor.parent()?;
            if ancestor.file_name().is_some_and(|name| name == "runtime")
                && parent.file_name().is_some_and(|name| name == "fastctx")
            {
                Some(parent)
            } else if ancestor.file_name().is_some_and(|name| name == ".fastctx") {
                Some(ancestor)
            } else {
                None
            }
        })
        .unwrap_or(path);
    let mut chain = Vec::new();
    for ancestor in path.ancestors() {
        chain.push(ancestor.to_path_buf());
        if ancestor == root {
            break;
        }
    }
    chain.reverse();
    chain
}

pub(super) fn configure_no_follow(options: &mut OpenOptions) {
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
}

pub(super) fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::{ensure_private_directory, resolve_runtime_component};
    use std::ffi::OsString;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    #[test]
    fn private_directory_mode_is_owner_only() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("edit-locks");
        ensure_private_directory(&directory, "edit-lock").unwrap();
        let mode = std::fs::metadata(directory).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn every_new_directory_in_the_managed_chain_starts_owner_only() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("fastctx");
        let second = first.join("runtime");
        let directory = second.join("edit-locks");
        ensure_private_directory(&directory, "edit-lock").unwrap();

        for managed in [first, second, directory] {
            let mode = std::fs::metadata(managed).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn existing_managed_chain_is_converged_to_owner_only() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join(".fastctx");
        let directory = parent.join("jobs");
        std::fs::create_dir_all(&directory).unwrap();
        for managed in [&parent, &directory] {
            std::fs::set_permissions(managed, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        ensure_private_directory(&directory, "background job registry").unwrap();

        for managed in [parent, directory] {
            let mode = std::fs::metadata(managed).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn managed_symlink_anchor_is_rejected_before_creating_children() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let anchor = home.join(".fastctx");
        symlink(&outside, &anchor).unwrap();

        let error =
            ensure_private_directory(&anchor.join("jobs"), "background job registry").unwrap_err();
        assert!(error.contains("not a private directory"), "{error}");
        assert!(!outside.join("jobs").exists());
    }

    #[test]
    fn symlink_in_the_trusted_runtime_base_remains_supported() {
        let temp = tempfile::tempdir().unwrap();
        let real_base = temp.path().join("real-runtime");
        std::fs::create_dir(&real_base).unwrap();
        let linked_base = temp.path().join("runtime-link");
        symlink(&real_base, &linked_base).unwrap();
        let directory = linked_base.join("fastctx-edit-locks");

        ensure_private_directory(&directory, "edit-lock").unwrap();

        let actual = real_base.join("fastctx-edit-locks");
        assert!(actual.is_dir());
        let mode = std::fs::metadata(actual).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn directory_symlink_is_rejected_without_chmodding_the_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("outside");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = temp.path().join("edit-locks");
        symlink(&target, &link).unwrap();

        let error = ensure_private_directory(&link, "edit-lock").unwrap_err();
        assert!(error.contains("not a private directory"), "{error}");
        let target_mode = std::fs::metadata(target).unwrap().permissions().mode() & 0o777;
        assert_eq!(target_mode, 0o755);
    }

    #[test]
    fn relative_runtime_environment_never_produces_a_relative_lock_path() {
        let path = resolve_runtime_component(
            "edit-locks",
            Some(OsString::from("relative-xdg")),
            PathBuf::from("relative-temp"),
        );
        assert!(path.is_absolute(), "{}", path.display());
        assert!(path.starts_with("/tmp"), "{}", path.display());
    }

    #[test]
    fn absolute_xdg_runtime_directory_is_used_without_an_architecture_split() {
        let path = resolve_runtime_component(
            "edit-locks",
            Some(OsString::from("/run/user/1234")),
            PathBuf::from("/ignored"),
        );
        assert_eq!(path, PathBuf::from("/run/user/1234/fastctx-edit-locks"));
    }
}
