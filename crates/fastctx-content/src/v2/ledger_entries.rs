//! A project's ledger as the hub accepted it (design §28.58): each entry its
//! author signed, by its number, in the hub's own library. Kept here alone,
//! never a stream: no device holds it, and what each reader of a project may
//! know of it goes into that reader's records as its projection
//! (`ledger/<project>/<machines>`). Written once an entry is checked, waited
//! on to the disk; an entry once kept is never written otherwise.

use super::Library;
use fastctx_storage::rusqlite::{OptionalExtension as _, params};
use fastctx_storage::{Error, ErrorKind, Priority, Result};

/// The table, made in a hub's libraries.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS ledger_entries (
    town TEXT NOT NULL,
    seq INTEGER NOT NULL,
    entry BLOB NOT NULL,
    PRIMARY KEY (town, seq)
) WITHOUT ROWID;
";

fn number(seq: u64) -> Result<i64> {
    i64::try_from(seq).map_err(|_| Error::with_kind(ErrorKind::Invalid, "an entry's number is past what is kept"))
}

impl Library {
    /// Keeps entry `seq` of the project `town`, the bytes its author signed:
    /// returned once it is on the disk or failed, never with its outcome
    /// unknown. Kept already as these bytes, nothing; as others, refused.
    pub fn ledger_keep(&self, town: &str, seq: u64, entry: Vec<u8>) -> Result<()> {
        let (town, seq) = (town.to_string(), number(seq)?);
        let bytes = 64 + town.len() + entry.len();
        self.database().write_now_settled_at(Priority::Normal, bytes, move |db| {
            let kept: Option<Vec<u8>> = db
                .prepare_cached("SELECT entry FROM ledger_entries WHERE town = ?1 AND seq = ?2")?
                .query_row(params![town, seq], |row| row.get(0))
                .optional()?;
            match kept {
                Some(kept) if kept == entry => Ok(()),
                Some(_) => Err(Error::with_kind(ErrorKind::Invalid, format!("entry {seq} of {town} is kept already as another"))),
                None => {
                    db.prepare_cached("INSERT INTO ledger_entries (town, seq, entry) VALUES (?1, ?2, ?3)")?
                        .execute(params![town, seq, entry])?;
                    Ok(())
                }
            }
        })
    }

    /// Entry `seq` of the project `town`, as its author signed it.
    pub fn ledger_entry(&self, town: &str, seq: u64) -> Result<Option<Vec<u8>>> {
        let (town, seq) = (town.to_string(), number(seq)?);
        self.database().read_now(move |db| {
            Ok(db
                .prepare_cached("SELECT entry FROM ledger_entries WHERE town = ?1 AND seq = ?2")?
                .query_row(params![town, seq], |row| row.get(0))
                .optional()?)
        })
    }

    /// The entries of the project `town` after `after`, in order, at most
    /// `most` of them.
    pub fn ledger_entries(&self, town: &str, after: u64, most: usize) -> Result<Vec<(u64, Vec<u8>)>> {
        let (town, after) = (town.to_string(), number(after)?);
        let most = i64::try_from(most).unwrap_or(i64::MAX);
        self.database().read_now(move |db| {
            Ok(db
                .prepare_cached("SELECT seq, entry FROM ledger_entries WHERE town = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3")?
                .query_map(params![town, after, most], |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// Every project with an entry kept.
    pub fn ledger_towns(&self) -> Result<Vec<String>> {
        self.database().read_now(|db| {
            Ok(db
                .prepare_cached("SELECT DISTINCT town FROM ledger_entries ORDER BY town")?
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// Lets the deleted project `town`'s entries go.
    pub fn ledger_forget(&self, town: &str) -> Result<()> {
        let town = town.to_string();
        self.database().write_now_settled_at(Priority::Normal, 64 + town.len(), move |db| {
            db.prepare_cached("DELETE FROM ledger_entries WHERE town = ?1")?.execute([town])?;
            Ok(())
        })
    }
}
