//! Read-only projections preserve out-of-band changes without submitting them.

use std::{io, path::Path};

fn writable(path: &Path) -> io::Result<()> {
    let mut permissions = fastctx_platform::disk::metadata(path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions)
}

pub(crate) fn delete(path: &Path) -> io::Result<()> {
    match fastctx_platform::disk::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_file() {
                writable(path)?;
            }
            if metadata.file_type().is_dir() {
                for entry in fastctx_platform::disk::read_dir(path)? {
                    delete(&entry?.path())?;
                }
                fastctx_platform::disk::remove_dir(path)
            } else if cfg!(windows) && metadata.file_type().is_symlink() && fastctx_platform::disk::is_dir(path) {
                fastctx_platform::disk::remove_dir(path)
            } else {
                fastctx_platform::disk::remove_file(path)
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn directory(path: &Path) -> io::Result<()> {
    match fastctx_platform::disk::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(io::Error::other("the replica directory was redirected"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fastctx_platform::ensure_private_dir(path)
}

pub(crate) fn publish(path: &Path, text: &str, previous: Option<&str>) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::other("a replica has no parent directory"))?;
    directory(parent)?;
    match fastctx_platform::disk::symlink_metadata(path) {
        Ok(metadata) => {
            let bytes = if metadata.file_type().is_file() { Some(fastctx_platform::disk::read(path)?) } else { None };
            if bytes.as_deref() == Some(text.as_bytes()) {
                return readonly(path);
            }
            let known =
                bytes.as_deref().is_some_and(|bytes| previous.is_some_and(|previous| bytes == previous.as_bytes()));
            if metadata.file_type().is_file() {
                writable(path)?;
            }
            if !known {
                let suffix = jiff::Timestamp::now().as_nanosecond();
                let name =
                    path.file_name().ok_or_else(|| io::Error::other("a replica has no filename"))?.to_string_lossy();
                let archive = parent.join("tampering");
                directory(&archive)?;
                fastctx_platform::disk::rename(path, archive.join(format!("{name}.{suffix}")))?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fastctx_platform::publish(path, text.as_bytes(), fastctx_platform::Permissions::Private)?;
    readonly(path)
}

/// Removes this projection and its quarantined edits when access is withdrawn.
pub(crate) fn withdraw(path: &Path) -> io::Result<()> {
    delete(path)?;
    let Some(parent) = path.parent() else { return Ok(()) };
    let Some(name) = path.file_name() else { return Ok(()) };
    let prefix = format!("{}.", name.to_string_lossy());
    let archive = parent.join("tampering");
    if fastctx_platform::disk::symlink_metadata(&archive).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return delete(&archive);
    }
    match fastctx_platform::disk::read_dir(archive) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    delete(&entry.path())?;
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

fn readonly(path: &Path) -> io::Result<()> {
    let mut permissions = fastctx_platform::disk::metadata(path)?.permissions();
    if !permissions.readonly() {
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
