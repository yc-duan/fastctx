//! Scratch coordinates travel beside nested on output, never in its byte streams.

use fastctx_types::{MachineName, scratch::Location};
use std::{io, path::Path};

pub(crate) fn locations(record: &Path, machine: &MachineName) -> io::Result<Vec<Location>> {
    let room = fastctx_hands::run::supervisor::room(record)?;
    let mut locations = vec![Location {
        hub: None,
        machine: machine.clone(),
        path: Ok(fastctx_hands::run::present::display_path(&room.scratch().to_string_lossy())),
    }];
    locations.extend(read(record)?);
    Ok(locations)
}

fn read(record: &Path) -> io::Result<Vec<Location>> {
    match std::fs::read(record.with_extension("scratch")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

pub(crate) fn add(record: &Path, paths: Vec<Location>) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    if !record.try_exists()? {
        return Err(io::Error::new(io::ErrorKind::NotFound, "the parent job is no longer on record"));
    }
    let mut known = read(record)?;
    let mut changed = false;
    for path in paths {
        match known.iter_mut().find(|held| held.hub == path.hub && held.machine == path.machine) {
            Some(held) if *held == path => {}
            Some(held) => {
                *held = path;
                changed = true;
            }
            None => {
                known.push(path);
                changed = true;
            }
        }
    }
    if changed {
        fastctx_platform::publish(
            &record.with_extension("scratch"),
            &serde_json::to_vec(&known).map_err(io::Error::other)?,
            fastctx_platform::Permissions::Private,
        )?;
    }
    Ok(())
}
