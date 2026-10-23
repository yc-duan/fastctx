//! Project attributes read from their ledger or the assisting metadata copy.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_types::TownName;
use fastctx_types::layout::WorldDir;
use fastctx_types::project::{Record, version_of};

/// Every version of the town's record, oldest first.
pub fn records(world: &WorldDir, town: &TownName) -> io::Result<Vec<Record>> {
    versions(&world.town(town).ledger().join("project"))
}

/// The town's record as its latest version says; `None` when this machine
/// keeps no ledger of the town.
pub fn latest(world: &WorldDir, town: &TownName) -> io::Result<Option<Record>> {
    if let Some(record) = records(world, town)?.pop() {
        return Ok(Some(record));
    }
    Ok(crate::ledger::metadata::read(world, town)?.map(|index| index.record))
}

/// The current record and its opening attribution, including an index-only copy.
pub fn current(world: &WorldDir, town: &TownName) -> io::Result<Option<(Record, fastctx_types::project::Stamp)>> {
    let records = records(world, town)?;
    if let (Some(first), Some(last)) = (records.first(), records.last()) {
        return Ok(Some((
            last.clone(),
            fastctx_types::project::Stamp { by: first.by.clone(), on: first.on.clone(), at: first.at },
        )));
    }
    Ok(crate::ledger::metadata::read(world, town)?.map(|index| (index.record, index.opened)))
}

/// The versions in a directory of `v<number>.json` files, oldest first.
fn versions<T: serde::de::DeserializeOwned>(directory: &Path) -> io::Result<Vec<T>> {
    let mut found: Vec<(u32, PathBuf)> = read_dir(directory)?
        .into_iter()
        .filter_map(|path| Some((version_of(path.file_name()?.to_str()?)?, path)))
        .collect();
    found.sort();
    found.into_iter().map(|(_, path)| read_json(&path)).collect()
}

fn read_dir(path: &Path) -> io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}
