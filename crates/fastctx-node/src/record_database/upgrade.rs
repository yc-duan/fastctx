//! Forward schema steps also accept databases made by earlier development builds.
//! All copies, cursor watermarks and version publications share the open transaction.
mod payloads;
mod positions;

use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension},
};

pub(super) fn payloads(db: &Connection) -> Result<()> {
    payloads::apply(db)
}
pub(super) fn records(db: &Connection) -> Result<()> {
    positions::records(db)
}
pub(super) fn runs(db: &Connection) -> Result<()> {
    positions::runs(db)
}
fn exists(db: &Connection, name: &str) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)", [name], |row| {
        row.get(0)
    })?)
}
fn column(db: &Connection, table: &str, name: &str) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo(?1) WHERE name=?2)", (table, name), |row| {
        row.get(0)
    })?)
}
fn primary(db: &Connection, table: &str) -> Result<Vec<String>> {
    Ok(db
        .prepare("SELECT name FROM pragma_table_xinfo(?1) WHERE pk>0 ORDER BY pk")?
        .query_map([table], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}
fn sequence(db: &Connection, table: &str) -> Result<i64> {
    if !exists(db, "sqlite_sequence")? {
        return Ok(0);
    }
    Ok(db
        .query_row("SELECT seq FROM sqlite_sequence WHERE name=?1", [table], |row| row.get(0))
        .optional()?
        .unwrap_or(0))
}
