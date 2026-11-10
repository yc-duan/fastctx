//! Small encoded representations share SQLite's publication transaction.

use super::rows::Stored;
use fastctx_storage::{
    Result,
    rusqlite::{Connection, params},
};

/// Maximum encoded representation size stored in SQLite instead of a content file.
pub const INLINE_BYTES: u32 = 32 * 1024;

pub(super) fn put(db: &Connection, stored: &Stored, bytes: &[u8]) -> Result<()> {
    let representation = &stored.representation;
    let id = &representation.content.id;
    db.execute(
        "INSERT INTO sync_asset_inline(hub,scope,hash,representation,bytes) VALUES(?1,?2,?3,?4,?5)",
        params![
            id.domain.hub.to_string(),
            id.domain.scope.as_str(),
            id.digest.as_str(),
            representation.id().as_str(),
            bytes
        ],
    )?;
    Ok(())
}
