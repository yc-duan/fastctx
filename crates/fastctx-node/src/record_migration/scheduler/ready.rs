//! Durable completion markers are an admission index, not an evicting cache.
use super::Key;
use fastctx_storage::{Result, rusqlite::Connection};
use std::collections::{HashMap, HashSet};

// 2026-11-01: bound corrupt or runaway family identities, not ordinary history.
const MAX_KEYS: usize = 1_048_576;
const MAX_BYTES: usize = 256 * 1024 * 1024;
#[derive(Default)]
pub(super) struct Ready {
    scopes: HashMap<String, HashSet<String>>,
    count: usize,
    bytes: usize,
}
impl Ready {
    pub(super) fn contains(&self, key: &Key) -> bool {
        self.scopes.get(&key.0).is_some_and(|families| families.contains(&key.1))
    }
    pub(super) fn insert(&mut self, key: Key) -> std::io::Result<()> {
        if self.contains(&key) {
            return Ok(());
        }
        let bytes = key.1.len() + 128 + if self.scopes.contains_key(&key.0) { 0 } else { key.0.len() + 128 };
        if self.count >= MAX_KEYS || self.bytes.saturating_add(bytes) > MAX_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "migration readiness exceeds its memory guard",
            ));
        }
        self.bytes += bytes;
        self.scopes.entry(key.0).or_default().insert(key.1);
        self.count += 1;
        fastctx_platform::readings::gauge("migration.ready_families").set(self.count as i64);
        Ok(())
    }
    pub(super) fn delete(&mut self, conversation: &str) {
        if let Some(families) = self.scopes.remove(conversation) {
            self.count -= families.len();
            self.bytes -= conversation.len() + 128 + families.iter().map(|family| family.len() + 128).sum::<usize>();
        }
        fastctx_platform::readings::gauge("migration.ready_families").set(self.count as i64);
    }
    pub(super) fn extend(&mut self, other: Self) -> std::io::Result<()> {
        for (conversation, families) in other.scopes {
            for family in families {
                self.insert((conversation.clone(), family))?;
            }
        }
        Ok(())
    }
}

pub(super) fn load(db: &Connection, excluded: &HashSet<String>, scope: Option<&str>) -> Result<Ready> {
    let mut ready = Ready::default();
    let mut query = db.prepare(
        "SELECT conversation,family FROM (
        SELECT conversation,family FROM conversation_record_imports
        UNION ALL SELECT conversation,'session-events' FROM session_heads WHERE imported=1
        UNION ALL SELECT conversation,'run-facts' FROM run_imported WHERE checked=1
        UNION ALL SELECT conversation,'gather/'||generation FROM conversation_record_gathering WHERE complete=1
    ) r WHERE (?1 IS NULL OR conversation=?1)
        AND NOT EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0)",
    )?;
    for row in query.query_map([scope], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
        let key = row?;
        if !excluded.contains(&key.0) {
            ready.insert(key).map_err(fastctx_storage::Error::from)?;
        }
    }
    Ok(ready)
}
