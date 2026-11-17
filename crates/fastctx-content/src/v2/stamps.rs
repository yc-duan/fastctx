//! A few named values a library keeps about itself, for whoever opened it:
//! a hub stamps each owner library with its own incarnation, so a library
//! put back from another hub, or from another time, is told apart (§17,
//! item 3).

use super::Library;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};

impl Library {
    /// The value kept under `name`, if any.
    pub fn stamped(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let name = name.to_string();
        Ok(self.database().read_now(move |db| {
            Ok(db.prepare_cached("SELECT value FROM stamps WHERE name = ?1")?.query_row([&name], |row| row.get(0)).optional()?)
        })?)
    }

    /// Keeps `value` under `name` when nothing is kept there yet; what is
    /// kept there after.
    pub fn stamp(&self, name: &str, value: Vec<u8>) -> Result<Vec<u8>> {
        let name = name.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 128 + value.len(), move |db| {
            db.prepare_cached("INSERT OR IGNORE INTO stamps (name, value) VALUES (?1, ?2)")?.execute(params![name, value])?;
            Ok(db.prepare_cached("SELECT value FROM stamps WHERE name = ?1")?.query_row([&name], |row| row.get(0))?)
        })?)
    }
}
