//! One encoded-byte entry point for files and SQLite-backed representations.

use crate::{
    blocks::{files::Files, location::Location, rows::Stored},
    identity::{Digest, OperationId},
    store::{Error, sql},
};
use fastctx_storage::{
    Database,
    rusqlite::{OptionalExtension, params},
};

pub(super) fn encoded(
    database: &Database,
    files: &Files,
    worker: &OperationId,
    stored: &Stored,
) -> Result<Vec<u8>, Error> {
    let representation = &stored.representation;
    let bytes = match &stored.location {
        Location::File(key) => files.read(key, representation.encoded_bytes)?,
        Location::Inline => database.read_now(|db| {
            let id = &representation.content.id;
            db.query_row(
                "SELECT bytes FROM sync_asset_inline WHERE hub=?1 AND scope=?2 AND hash=?3 AND representation=?4 AND length(bytes)=?5",
                params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str(), representation.id().as_str(), representation.encoded_bytes],
                |row| row.get::<_, Vec<u8>>(0),
            ).optional()?.ok_or_else(|| fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Corrupt, "published inline content is missing or has an invalid length"))
        }).map_err(|error| sql::error(error, worker.clone()))?,
    };
    if Digest::of(&bytes) != representation.encoded_digest {
        return Err(Error::Corrupt("published content failed its encoded digest".into()));
    }
    Ok(bytes)
}
