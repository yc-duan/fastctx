//! Import journals are published under one gate, which a project's name is
//! also claimed under: a claim never interleaves with a carry's publication.

use fastctx_platform::disk;
use serde::Serialize;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

static GATE: Mutex<()> = Mutex::new(());

pub(crate) fn with_import_gate<T>(work: impl FnOnce() -> T) -> T {
    let _gate = GATE.lock().expect("never poisoned");
    work()
}

pub(crate) fn keep_import(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let _gate = GATE.lock().expect("never poisoned");
    fastctx_net::link::state::write(path, value)
}

pub(crate) fn import_journals(parent: &Path, name: &str) -> io::Result<Vec<PathBuf>> {
    let entries = match disk::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let path = entry.path().join(name);
            if disk::exists(&path)? {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}
