//! Where each producer on this machine stands in its sources (design §4.1,
//! §10): kept beside what it wrote, read back to go on from there.

use super::Library;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};

impl Library {
    /// What the producer `name` last kept of where it stands.
    pub fn checkpoint(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let name = name.to_string();
        Ok(self.database().read_now(move |db| {
            Ok(db.prepare_cached("SELECT state FROM checkpoints WHERE name = ?1")?.query_row([&name], |row| row.get(0)).optional()?)
        })?)
    }

    /// Keeps where the producer `name` stands: written after what it wrote
    /// from there, so a stop between the two makes it write nothing new.
    pub fn keep_checkpoint(&self, name: &str, state: Vec<u8>) -> Result<()> {
        let name = name.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 64 + state.len(), move |db| {
            db.prepare_cached("INSERT INTO checkpoints (name, state) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET state = excluded.state")?
                .execute(params![name, state])?;
            Ok(())
        })?)
    }

    /// Forgets where the producer `name` stood.
    pub fn drop_checkpoint(&self, name: &str) -> Result<()> {
        let name = name.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 64, move |db| {
            db.prepare_cached("DELETE FROM checkpoints WHERE name = ?1")?.execute([&name])?;
            Ok(())
        })?)
    }
}
