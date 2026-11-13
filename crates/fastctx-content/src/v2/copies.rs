//! Scopes found by the end of their key (plan 3-5, design §28.30): a
//! conversation of another machine is looked up here by its resident's id,
//! the machine it was born on or the body it is in not known to the reader.

use super::Library;
use crate::Result;
use fastctx_storage::rusqlite::params;

/// A scope held here, as a reader decides whether to read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldScope {
    pub key: String,
    pub holding: i64,
    /// The holding period its first pull was done in, when it was.
    pub pulled: Option<i64>,
    /// The classes of its streams the hub lets this device hold, one
    /// prefix a line.
    pub allowed: Option<String>,
    /// The hub that described it, and its description as the hub gave it.
    pub hub: Option<String>,
    pub description: Option<Vec<u8>>,
}

impl Library {
    /// The keys of every scope this library knows, in any state, ending
    /// with `tail`.
    pub fn keys_ending(&self, tail: &str) -> Result<Vec<String>> {
        let tail = tail.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT key FROM scopes
                 WHERE length(key) > length(?1) AND substr(key, length(key) - length(?1) + 1) = ?1
                 ORDER BY key",
            )?;
            let rows = statement.query_map(params![tail], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Every live scope held here.
    pub fn held_scopes(&self) -> Result<Vec<HeldScope>> {
        self.scopes_ending("")
    }

    /// The live scopes held here whose key ends with `tail`.
    pub fn scopes_ending(&self, tail: &str) -> Result<Vec<HeldScope>> {
        let tail = tail.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT key, holding, pulled, allowed, hub, description FROM scopes
                 WHERE state = 'live' AND holding IS NOT NULL AND length(key) > length(?1)
                   AND (?1 = '' OR substr(key, length(key) - length(?1) + 1) = ?1)
                 ORDER BY key",
            )?;
            let rows = statement.query_map(params![tail], |row| {
                Ok(HeldScope {
                    key: row.get(0)?,
                    holding: row.get(1)?,
                    pulled: row.get(2)?,
                    allowed: row.get(3)?,
                    hub: row.get(4)?,
                    description: row.get(5)?,
                })
            })?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }
}
