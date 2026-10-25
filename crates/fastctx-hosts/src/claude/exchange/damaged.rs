//! Reconsiders reports set aside by an older decoder, without replaying an
//! applied report or changing its original evidence.

use super::{ModEvent, read_report};
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use std::{io, path::Path};

/// Restores reports the current decoder understands. Originals remain in
/// quarantine for diagnosis; the ordinary applied marker prevents repetition.
pub fn recover_readable(directory: &Path) -> io::Result<usize> {
    let entries = match std::fs::read_dir(directory.join("damaged")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut recovered = 0;
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let name = path.file_name().expect("directory entry");
        if directory.join("applied").join(name).try_exists()? {
            continue;
        }
        let body = read_report(&path)?;
        if serde_json::from_slice::<ModEvent>(&body).is_err() {
            continue;
        }
        let target = directory.join("events").join(name);
        ensure_private_dir(target.parent().expect("event directory"))?;
        match std::fs::read_dir(path.with_extension("parts")) {
            Ok(parts) => {
                let destination = target.with_extension("parts");
                ensure_private_dir(&destination)?;
                for part in parts {
                    let part = part?;
                    identical(&destination.join(part.file_name()), &std::fs::read(part.path())?)?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let written = identical(&target, &std::fs::read(&path)?)?;
        if written {
            std::fs::OpenOptions::new().write(true).open(&target)?.set_modified(path.metadata()?.modified()?)?;
        }
        identical(&target.with_extension("done"), b"")?;
        recovered += usize::from(written);
    }
    Ok(recovered)
}

fn identical(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    match publish_new(path, bytes, Permissions::Private) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if std::fs::read(path)? == bytes {
                Ok(false)
            } else {
                Err(io::Error::other(format!("a recovered report conflicts with {}", path.display())))
            }
        }
        Err(error) => Err(error),
    }
}
