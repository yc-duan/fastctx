//! Import writes preserve stored revisions without replaying derived triggers.
//! The connection setting is restored before the writer can accept another job,
//! including while unwinding a failed batch.
use fastctx_storage::{
    Result,
    rusqlite::{Connection, config::DbConfig},
};

pub(crate) struct SuspendedTriggers<'a> {
    connection: &'a Connection,
    enabled: bool,
}

impl<'a> SuspendedTriggers<'a> {
    pub(crate) fn new(connection: &'a Connection) -> Result<Self> {
        let enabled = connection.db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)?;
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)?;
        Ok(Self { connection, enabled })
    }
}

impl Drop for SuspendedTriggers<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.connection.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, self.enabled) {
            // 2026-11-01: Continuing on this connection would silently bypass
            // deletion fences; unwinding alone is caught by the writer.
            eprintln!("restoring SQLite triggers failed: {error}");
            std::process::abort();
        }
    }
}
