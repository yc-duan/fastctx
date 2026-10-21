//! Durable, per-session accounting receipts written through the official mod filesystem API.
use fastctx_platform::{Permissions, publish};
use std::io;
use std::path::Path;

pub(super) fn promote(directory: &Path, writer_ended: bool) -> io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(directory.join("accounting")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut problems = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let name = path.file_name().expect("receipt name");
        if directory.join("applied").join(name).exists() {
            clean(directory, name)?;
            continue;
        }
        if !writer_ended && !path.with_extension("ready").exists() {
            continue;
        }
        let bytes = match super::exchange::checked_report(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                problems.push(format!("accounting receipt {} cannot be recovered: {error}", path.display()));
                continue;
            }
        };
        fastctx_platform::ensure_private_dir(&directory.join("events"))?;
        let event = directory.join("events").join(name);
        if !event.with_extension("done").exists() {
            publish(&event, &bytes, Permissions::Private)?;
            publish(&event.with_extension("done"), b"", Permissions::Private)?;
        }
    }
    Ok(problems)
}

pub(super) fn clean(directory: &Path, name: &std::ffi::OsStr) -> io::Result<()> {
    let path = directory.join("accounting").join(name);
    for file in [&path, &path.with_extension("ready")] {
        match std::fs::remove_file(file) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    match std::fs::remove_dir_all(path.with_extension("parts")) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
