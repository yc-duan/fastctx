//! Durable completion markers, mirrored so admission needs no database read.
//!
//! A conversation's gathered layout and its import are asked for on every
//! read of its records, without a read of their markers: those stay mirrored
//! while the node runs, one or two per conversation. Every other family is
//! checked in the database by its blocking readers first, so its mirror only
//! holds the latest found and is let go whole when it fills. Past its guards
//! the mirror stops growing; it never refuses a key or fails its caller.
use super::Key;
use fastctx_storage::{Result, rusqlite::Connection};
use std::collections::{HashMap, HashSet};

/// Guards memory against corrupt or runaway family identities, not ordinary history.
const MAX_KEYS: usize = 1_048_576;
const MAX_BYTES: usize = 256 * 1024 * 1024;
/// How many families checked by their readers are kept at once.
const CACHED: usize = 65_536;

/// Families asked for without a read of their marker.
fn asked_unread(family: &str) -> bool {
    family.starts_with("gather/") || family == "layout-metadata" || family == "conversation-bundle"
}

#[derive(Default)]
struct Scopes {
    scopes: HashMap<String, HashSet<String>>,
    count: usize,
    bytes: usize,
}
impl Scopes {
    fn contains(&self, key: &Key) -> bool {
        self.has(&key.0, &key.1)
    }
    fn has(&self, conversation: &str, family: &str) -> bool {
        self.scopes.get(conversation).is_some_and(|families| families.contains(family))
    }
    fn insert(&mut self, key: Key) -> bool {
        let bytes = key.1.len() + 128 + if self.scopes.contains_key(&key.0) { 0 } else { key.0.len() + 128 };
        if self.count >= MAX_KEYS || self.bytes.saturating_add(bytes) > MAX_BYTES {
            fastctx_platform::readings::counter("migration.ready_unmirrored").add(1);
            return false;
        }
        if self.scopes.entry(key.0).or_default().insert(key.1) {
            self.bytes += bytes;
            self.count += 1;
        }
        true
    }
    fn delete(&mut self, conversation: &str) {
        if let Some(families) = self.scopes.remove(conversation) {
            self.count -= families.len();
            self.bytes -= conversation.len() + 128 + families.iter().map(|family| family.len() + 128).sum::<usize>();
        }
    }
    fn forget(&mut self, conversation: &str, prefix: &str) {
        let Some(families) = self.scopes.get_mut(conversation) else { return };
        let before = families.len();
        let mut freed = 0;
        families.retain(|family| {
            let kept = !family.starts_with(prefix);
            if !kept {
                freed += family.len() + 128;
            }
            kept
        });
        self.count -= before - families.len();
        self.bytes -= freed;
        if families.is_empty() {
            self.scopes.remove(conversation);
            self.bytes -= conversation.len() + 128;
        }
    }
}

#[derive(Default)]
pub(super) struct Ready {
    unread: Scopes,
    cached: Scopes,
}
impl Ready {
    pub(super) fn contains(&self, key: &Key) -> bool {
        self.unread.contains(key) || self.cached.contains(key)
    }
    pub(super) fn has(&self, conversation: &str, family: &str) -> bool {
        self.unread.has(conversation, family) || self.cached.has(conversation, family)
    }
    pub(super) fn insert(&mut self, key: Key) {
        if self.contains(&key) {
            return;
        }
        if asked_unread(&key.1) {
            self.unread.insert(key);
        } else {
            if self.cached.count >= CACHED {
                self.cached = Scopes::default();
            }
            self.cached.insert(key);
        }
        self.told();
    }
    pub(super) fn delete(&mut self, conversation: &str) {
        self.unread.delete(conversation);
        self.cached.delete(conversation);
        self.told();
    }
    /// Lets go of the families under `prefix`, retired with what they held.
    pub(super) fn forget(&mut self, conversation: &str, prefix: &str) {
        self.unread.forget(conversation, prefix);
        self.cached.forget(conversation, prefix);
        self.told();
    }
    pub(super) fn extend(&mut self, other: Self) {
        for scopes in [other.unread, other.cached] {
            for (conversation, families) in scopes.scopes {
                for family in families {
                    self.insert((conversation.clone(), family));
                }
            }
        }
    }
    fn told(&self) {
        fastctx_platform::readings::gauge("migration.ready_families")
            .set((self.unread.count + self.cached.count) as i64);
    }
}

/// The gathered layouts complete when the node opens, or those of `scope`.
pub(super) fn load(db: &Connection, excluded: &HashSet<String>, scope: Option<&str>) -> Result<Ready> {
    let mut ready = Ready::default();
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='conversation_record_gathering')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(ready);
    }
    let fence = {
        let transfers: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='node_conversation_transfers')",
            [],
            |row| row.get(0),
        )?;
        if transfers {
            " AND NOT EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0)"
        } else {
            ""
        }
    };
    let sql = format!(
        "SELECT conversation,'gather/'||generation FROM conversation_record_gathering r
         WHERE complete=1 AND (?1 IS NULL OR conversation=?1){fence}"
    );
    let mut query = db.prepare(&sql)?;
    for row in query.query_map([scope], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
        let key = row?;
        if !excluded.contains(&key.0) {
            ready.insert(key);
        }
    }
    Ok(ready)
}
