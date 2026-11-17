//! Project attributes read from their ledger or the assisting metadata copy.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_types::TownName;
use fastctx_types::layout::WorldDir;
use fastctx_types::project::{Record, version_of};

/// The town's record as its latest version says; `None` when this machine
/// keeps no ledger of the town. Only that version is read, however many
/// came before it.
pub fn latest(world: &WorldDir, town: &TownName) -> io::Result<Option<Record>> {
    if let Some((_, last)) = ends(&world.town(town).ledger().join("project"))? {
        return Ok(Some(last));
    }
    Ok(crate::ledger::metadata::read(world, town)?.map(|index| index.record))
}

/// The current record and its opening attribution, including an index-only
/// copy. The first version says who opened the project, the last what it is
/// now: those two are read, not every edit between them.
pub fn current(world: &WorldDir, town: &TownName) -> io::Result<Option<(Record, fastctx_types::project::Stamp)>> {
    if let Some((first, last)) = ends::<Record>(&world.town(town).ledger().join("project"))? {
        let opened = fastctx_types::project::Stamp { by: first.by.clone(), on: first.on.clone(), at: first.at };
        return Ok(Some((last, opened)));
    }
    Ok(crate::ledger::metadata::read(world, town)?.map(|index| (index.record, index.opened)))
}

/// The oldest and the newest of the versions in a directory of
/// `v<number>.json` files: its names are listed, and those two files read.
fn ends<T: serde::de::DeserializeOwned>(directory: &Path) -> io::Result<Option<(T, T)>> {
    let found: Vec<(u32, PathBuf)> = read_dir(directory)?
        .into_iter()
        .filter_map(|path| Some((version_of(path.file_name()?.to_str()?)?, path)))
        .collect();
    let (Some((_, first)), Some((_, last))) =
        (found.iter().min_by_key(|(version, _)| *version), found.iter().max_by_key(|(version, _)| *version))
    else {
        return Ok(None);
    };
    Ok(Some((read_json(first)?, read_json(last)?)))
}

fn read_dir(path: &Path) -> io::Result<Vec<PathBuf>> {
    match fastctx_platform::disk::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&fastctx_platform::disk::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}
