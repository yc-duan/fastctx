//! Transactional engine and independently versioned service schemas.

use crate::{Error, ErrorKind, Result};
use rusqlite::{Connection, OptionalExtension};

/// One forward-only schema step. Steps must be consecutive, beginning at one.
pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

pub(super) fn engine(connection: &Connection) -> Result<()> {
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > 1 {
        return Err(Error::with_kind(ErrorKind::Invalid, "the database needs a newer storage engine"));
    }
    if version == 0 {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS storage_versions (
            component TEXT PRIMARY KEY, version INTEGER NOT NULL
        ) WITHOUT ROWID;",
        )?;
        connection.pragma_update(None, "user_version", 1)?;
    }
    Ok(())
}

/// Applies pending steps inside the caller's transaction. No version is
/// published unless every statement of its migration commits with it.
pub fn apply(connection: &Connection, component: &str, steps: &[Migration]) -> Result<()> {
    if connection.is_autocommit() {
        return Err(Error::with_kind(ErrorKind::Invalid, "schema migration requires a transaction"));
    }
    if component.is_empty() || steps.iter().enumerate().any(|(i, step)| step.version as usize != i + 1) {
        return Err(Error::with_kind(ErrorKind::Invalid, "schema migrations must be consecutive"));
    }
    let current: u32 = connection
        .query_row("SELECT version FROM storage_versions WHERE component=?1", [component], |row| row.get(0))
        .optional()?
        .unwrap_or(0);
    if current as usize > steps.len() {
        return Err(Error::with_kind(ErrorKind::Invalid, format!("the {component} schema needs a newer version")));
    }
    for step in steps.iter().skip(current as usize) {
        connection.execute_batch(step.sql)?;
        connection.execute(
            "INSERT INTO storage_versions(component,version) VALUES(?1,?2)
            ON CONFLICT(component) DO UPDATE SET version=excluded.version",
            (component, step.version),
        )?;
    }
    Ok(())
}
