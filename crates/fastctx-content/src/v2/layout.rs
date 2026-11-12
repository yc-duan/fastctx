//! The layout of a machine's asset library: its tables as this build makes
//! them (`schema::LAYOUT`, stamped as the tables are made). Until the layout
//! is settled (plan 3-3), a library of an earlier one is not carried over: it
//! is refused, named, and made again from what the hub and the machine's own
//! sources hold (GG01). From the settled layout on, a later one comes with
//! what carries the earlier forward.

use super::schema::LAYOUT;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use fastctx_storage::{Error, ErrorKind, Result};
use std::path::Path;

/// Makes the tables `schema` names in `db`, the library at `path`: a new
/// library's, stamped with this build's layout in the same transaction; an
/// existing one's only when its stamp is this build's, refused before
/// anything is made otherwise. Nothing is written for a library made
/// already, so opening it waits on no other writer. In the transaction that
/// opens it, after `schema::PRELUDE`.
pub(super) fn made(db: &Connection, path: &Path, schema: &str) -> Result<()> {
    let there = db.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'scopes'")?.query_row([], |_| Ok(())).optional()?.is_some();
    if !there {
        db.execute_batch(schema)?;
        db.execute("INSERT OR REPLACE INTO stamps (name, value) VALUES ('layout', ?1)", params![LAYOUT.as_bytes()])?;
        return Ok(());
    }
    let layout: Option<Vec<u8>> =
        db.prepare("SELECT value FROM stamps WHERE name = 'layout'")?.query_row([], |row| row.get(0)).optional()?;
    if layout.as_deref() != Some(LAYOUT.as_bytes()) {
        let was = layout.map_or_else(|| "none".to_string(), |layout| String::from_utf8_lossy(&layout).into_owned());
        return Err(Error::with_kind(
            ErrorKind::Invalid,
            format!(
                "the asset library at {} is of an earlier layout ({was}) than this build's ({LAYOUT}), made before the layout was \
                 settled; it is not carried over: move it aside and it is made again",
                path.display()
            ),
        ));
    }
    db.execute_batch(schema)?;
    Ok(())
}
