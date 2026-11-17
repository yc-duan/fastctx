//! The format of the World's records. Local data only moves forward: a node
//! claims the World's format before it migrates anything, and one that finds
//! the World written by a newer format refuses to open it, saying which
//! version wrote it, instead of taking the records it moved away for none.
//! The release before this marker cannot read it, so it is stopped instead
//! where it opens the node database (`record_database`).

use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize};
use std::io;

/// Format 2 keeps the records in databases; the release before it, which
/// never wrote this file, kept them in files. Format 3 keeps the texts of
/// plan and history copies apart from their metadata (`replicas::bodies`).
/// Format 4 retains source assets for the unified synchronization foundation.
pub(crate) const CURRENT: u32 = 4;

#[derive(Serialize, Deserialize)]
struct Format {
    format: u32,
    written_by: String,
    /// Survives death between the file fence and the SQL upgrade transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_format: Option<u32>,
}

fn path(world: &WorldDir) -> std::path::PathBuf {
    world.path().join("format.json")
}

fn read(world: &WorldDir) -> io::Result<Option<Format>> {
    match fastctx_platform::disk::read(path(world)) {
        Ok(bytes) => {
            let found: Format = serde_json::from_slice(&bytes)?;
            fastctx_storage::format_upgrade::validate_written_by(&found.written_by).map_err(io::Error::other)?;
            if found.format == 0 || found.previous_format.is_some_and(|old| old == 0 || old >= found.format) {
                return Err(io::Error::other("invalid World format provenance"));
            }
            Ok(Some(found))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Refuses a World a newer format wrote.
pub(crate) fn check(world: &WorldDir) -> io::Result<()> {
    match read(world)? {
        Some(found) if found.format > CURRENT => Err(io::Error::other(format!(
            "this World was written by Fastctx {}, which is newer than this one ({}); install {} or a newer \
             version to open it",
            found.written_by,
            env!("CARGO_PKG_VERSION"),
            found.written_by
        ))),
        _ => Ok(()),
    }
}

/// Marks the World as this format's, before anything is moved into it. Only
/// the node that holds the World does this.
pub(crate) fn claim(world: &WorldDir) -> io::Result<()> {
    check(world)?;
    let version = env!("CARGO_PKG_VERSION");
    let found = read(world)?;
    if found.as_ref().is_some_and(|found| found.format == CURRENT && found.written_by == version) {
        return Ok(());
    }
    let previous_format =
        found.and_then(|found| if found.format < CURRENT { Some(found.format) } else { found.previous_format });
    let bytes =
        serde_json::to_vec_pretty(&Format { format: CURRENT, written_by: version.to_string(), previous_format })?;
    fastctx_platform::publish(&path(world), &bytes, fastctx_platform::Permissions::Private)
}

/// The file fence is ahead of SQL; an interrupted opening retains its old format.
pub(crate) fn previous(world: &WorldDir) -> io::Result<Option<u32>> {
    Ok(read(world)?.and_then(|found| found.previous_format))
}
