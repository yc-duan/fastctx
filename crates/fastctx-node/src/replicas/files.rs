//! Read-only projections preserve out-of-band changes without submitting them.
//!
//! A projection's length and modification time, kept as it is written, tell
//! whether the file still holds it without reading it; only a file that
//! moved is read.

use super::{bodies::Text, state::Projected};
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

fn modified(metadata: &std::fs::Metadata) -> Option<i128> {
    let since = metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    i128::try_from(since.as_nanos()).ok()
}

/// Whether `path` still holds what was projected there, by its length and
/// modification time alone.
pub(crate) fn intact(path: &Path, projected: &Projected) -> bool {
    fastctx_platform::disk::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_file()
            && metadata.len() == projected.text.len
            && modified(&metadata).is_some_and(|time| Some(time) == projected.modified)
    })
}

/// Puts `text` at `path`. What stands there and is neither `text` nor what
/// this computer projected there (`projected`, or for a copy that never kept
/// it, `previous`) was changed out of band: it is moved under `tampering/`,
/// never submitted and never deleted. What `path` holds now.
pub(crate) fn publish(
    path: &Path,
    text: &str,
    projected: Option<&Projected>,
    previous: Option<&Text>,
) -> io::Result<Projected> {
    let parent = path.parent().ok_or_else(|| io::Error::other("a replica has no parent directory"))?;
    directory(parent)?;
    let named = Text::of(text);
    if !set_aside(path, Some(&named), projected, previous)? {
        readonly(path)?;
        return stamp(path, named);
    }
    fastctx_platform::publish(path, text.as_bytes(), fastctx_platform::Permissions::Private)?;
    readonly(path)?;
    stamp(path, named)
}

/// Takes a retired document's projection away, keeping what was changed in
/// it out of band as `publish` does.
pub(crate) fn retire(path: &Path, projected: Option<&Projected>, previous: Option<&Text>) -> io::Result<()> {
    if set_aside(path, None, projected, previous)? {
        match fastctx_platform::disk::symlink_metadata(path) {
            Ok(_) => delete(path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn stamp(path: &Path, text: Text) -> io::Result<Projected> {
    Ok(Projected { text, modified: modified(&fastctx_platform::disk::metadata(path)?) })
}

/// Readies `path` to hold `text`, or nothing. False when it holds `text`
/// already.
fn set_aside(
    path: &Path,
    text: Option<&Text>,
    projected: Option<&Projected>,
    previous: Option<&Text>,
) -> io::Result<bool> {
    let metadata = match fastctx_platform::disk::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() {
        quarantine(path, None)?;
        return Ok(true);
    }
    // Read whole: a change that kept the file's length and time is still a
    // change, and replacing a projection is rare enough to afford it.
    let bytes = fastctx_platform::disk::read(path)?;
    let held = Text::of_bytes(&bytes);
    if text.is_some_and(|text| *text == held) {
        return Ok(false);
    }
    let ours =
        projected.is_some_and(|projected| projected.text == held) || previous.is_some_and(|previous| *previous == held);
    writable(path)?;
    if !ours {
        quarantine(path, Some(&bytes[..]))?;
    }
    Ok(true)
}

/// Keeps what replaced a projection under `tampering/<file name>/`, one
/// entry each time it is found, never over an earlier one.
fn quarantine(path: &Path, bytes: Option<&[u8]>) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::other("a replica has no parent directory"))?;
    let name = path.file_name().ok_or_else(|| io::Error::other("a replica has no filename"))?;
    let archive = parent.join("tampering");
    directory(&archive)?;
    let archive = archive.join(name);
    directory(&archive)?;
    let stamp = jiff::Timestamp::now().as_nanosecond();
    for attempt in 0_u32.. {
        let kept = archive.join(if attempt == 0 { format!("{stamp}") } else { format!("{stamp}-{attempt}") });
        let result = match bytes {
            Some(bytes) => fastctx_platform::publish_new(&kept, bytes, fastctx_platform::Permissions::Private)
                .and_then(|()| fastctx_platform::disk::remove_file(path)),
            // Not a file, or not read: what stands there is moved aside whole.
            None if fastctx_platform::disk::symlink_metadata(&kept).is_ok() => {
                Err(io::Error::from(io::ErrorKind::AlreadyExists))
            }
            None => fastctx_platform::disk::rename(path, &kept),
        };
        match result {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            result => return result,
        }
    }
    unreachable!("the attempts are unbounded")
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
    delete(&archive.join(name))?;
    // Releases before format 3 kept them beside each other.
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
