//! Where each producer on this machine stands in its sources (design §4.1,
//! §10): kept beside what it wrote, read back to go on from there. With it,
//! what a producer holds in its window, too many to keep in the checkpoint
//! itself (design §28.51 ⑤-7): kept under a generation its checkpoint names,
//! and an epoch that moves whenever what was kept is let go from beside the
//! producer, so that one that read before cannot keep after.

use super::Library;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::types::Value;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params, params_from_iter};

/// An item a producer holds in its window: its time (seconds, nanoseconds)
/// and order key.
pub type HeldItem = (i64, i32, u64);

impl Library {
    /// What the producer `name` last kept of where it stands.
    pub fn checkpoint(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let name = name.to_string();
        Ok(self.database().read_now(move |db| {
            Ok(db.prepare_cached("SELECT state FROM checkpoints WHERE name = ?1")?.query_row([&name], |row| row.get(0)).optional()?)
        })?)
    }

    /// As `checkpoint`, with the epoch it was read in — both in one read
    /// transaction, so that what was let go between them is not read as
    /// standing: kept again only in that epoch (`keep_checkpoint_in`).
    pub fn checkpoint_in_epoch(&self, name: &str) -> Result<(Option<Vec<u8>>, u64)> {
        let name = name.to_string();
        Ok(self.database().read_now(move |connection| {
            let db = connection.unchecked_transaction()?;
            let state = db.prepare_cached("SELECT state FROM checkpoints WHERE name = ?1")?.query_row([&name], |row| row.get(0)).optional()?;
            let epoch = epoch(&db, &name)?;
            Ok((state, epoch))
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

    /// As `keep_checkpoint`, returning only once the write did commit or
    /// fail — never with its outcome unknown while it may still commit — so
    /// that a producer that holds its turn while it keeps holds it until
    /// then: what a later turn writes beside the checkpoint never meets a
    /// checkpoint of this one landing late (design §28.51 ⑤-7).
    pub fn keep_checkpoint_settled(&self, name: &str, state: Vec<u8>) -> Result<()> {
        let name = name.to_string();
        Ok(self.database().write_now_settled_at(Priority::Normal, 64 + state.len(), move |db| {
            db.prepare_cached("INSERT INTO checkpoints (name, state) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET state = excluded.state")?
                .execute(params![name, state])?;
            Ok(())
        })?)
    }

    /// As `keep_checkpoint_settled`, only while the epoch is still `epoch` —
    /// what it stands on not let go meanwhile — in the transaction that
    /// checks it: whether it was kept.
    pub fn keep_checkpoint_in(&self, name: &str, state: Vec<u8>, epoch: u64) -> Result<bool> {
        let name = name.to_string();
        Ok(self.database().write_now_settled_at(Priority::Normal, 64 + state.len(), move |db| {
            if self::epoch(db, &name)? != epoch {
                return Ok(false);
            }
            db.prepare_cached("INSERT INTO checkpoints (name, state) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET state = excluded.state")?
                .execute(params![name, state])?;
            Ok(true)
        })?)
    }

    /// Forgets where the producer `name` stood, and what it held: the epoch
    /// moves on with the checkpoint's going, what it held goes after, a
    /// batch at a time.
    pub fn drop_checkpoint(&self, name: &str) -> Result<()> {
        let dropping = name.to_string();
        self.database().write_now_at(Priority::Normal, 64, move |db| {
            db.prepare_cached("DELETE FROM checkpoints WHERE name = ?1")?.execute([&dropping])?;
            let_go(db, &dropping)?;
            Ok(())
        })?;
        self.sweep_held(name, None)?;
        self.sweep_digests(name, None)
    }

    /// Keeps `items` among those the producer `name` holds under
    /// `generation`, one batch of them; `fresh`, the first batch: what an
    /// attempt that never finished kept under it goes first. None is read
    /// until a checkpoint names the generation. Settled before it returns,
    /// as `keep_checkpoint_settled` is.
    pub fn stage_held(&self, name: &str, generation: u64, fresh: bool, items: Vec<HeldItem>) -> Result<()> {
        if fresh {
            self.delete_rows(Kept::Held, name, Generations::Only(generation))?;
        }
        let name = name.to_string();
        Ok(self.database().write_now_settled_at(Priority::Normal, 64 + items.len() * 40, move |db| {
            let mut insert = db.prepare_cached("INSERT OR IGNORE INTO checkpoint_held (name, generation, second, nanos, key) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            for (second, nanos, key) in items {
                insert.execute(params![name, generation as i64, second, nanos, key.to_be_bytes().as_slice()])?;
            }
            Ok(())
        })?)
    }

    /// Puts `kept` in place of `gone` among the items the producer `name`
    /// holds under `generation`, in one write, the rest left as they are:
    /// what a window written on from a place inside it changes of what it
    /// holds, under the generation its checkpoint names already. Settled
    /// before it returns, as `stage_held` is.
    pub fn restage_held(&self, name: &str, generation: u64, gone: Vec<HeldItem>, kept: Vec<HeldItem>) -> Result<()> {
        let name = name.to_string();
        Ok(self.database().write_now_settled_at(Priority::Normal, 64 + (gone.len() + kept.len()) * 40, move |db| {
            let mut delete = db.prepare_cached("DELETE FROM checkpoint_held WHERE name = ?1 AND generation = ?2 AND second = ?3 AND nanos = ?4 AND key = ?5")?;
            for (second, nanos, key) in gone {
                delete.execute(params![name, generation as i64, second, nanos, key.to_be_bytes().as_slice()])?;
            }
            let mut insert = db.prepare_cached("INSERT OR IGNORE INTO checkpoint_held (name, generation, second, nanos, key) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            for (second, nanos, key) in kept {
                insert.execute(params![name, generation as i64, second, nanos, key.to_be_bytes().as_slice()])?;
            }
            Ok(())
        })?)
    }

    /// The highest order key of the items the producer `name` holds under
    /// `generation` from before `second` and `nanos`; none where none is.
    pub fn held_before(&self, name: &str, generation: u64, second: i64, nanos: i32) -> Result<Option<u64>> {
        let name = name.to_string();
        Ok(self.database().read_now(move |db| {
            let key: Option<Vec<u8>> = db
                .prepare_cached("SELECT MAX(key) FROM checkpoint_held WHERE name = ?1 AND generation = ?2 AND (second, nanos) < (?3, ?4)")?
                .query_row(params![name, generation as i64, second, nanos], |row| row.get(0))?;
            Ok(key.and_then(|key| <[u8; 8]>::try_from(key.as_slice()).ok()).map(u64::from_be_bytes))
        })?)
    }

    /// The items the producer `name` holds under `generation`, in their time's
    /// order: for comparing what two readings left, and looking into it.
    pub fn held_items(&self, name: &str, generation: u64) -> Result<Vec<HeldItem>> {
        let name = name.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement =
                db.prepare_cached("SELECT second, nanos, key FROM checkpoint_held WHERE name = ?1 AND generation = ?2 ORDER BY second, nanos, key")?;
            let rows = statement.query_map(params![name, generation as i64], |row| {
                let key: Vec<u8> = row.get(2)?;
                Ok((row.get(0)?, row.get(1)?, <[u8; 8]>::try_from(key.as_slice()).map(u64::from_be_bytes).unwrap_or_default()))
            })?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?)
    }

    /// Lets go what the producer `name` held under any generation but
    /// `keep` (all of it for none): those its checkpoint no longer names.
    pub fn sweep_held(&self, name: &str, keep: Option<u64>) -> Result<()> {
        self.delete_rows(Kept::Held, name, keep.map_or(Generations::All, Generations::AllBut))
    }

    /// Keeps `digests` among those the producer `name` keeps under
    /// `generation`, one batch of them; `fresh`, the first batch of a new
    /// generation: what an attempt that never finished kept under it goes
    /// first. A digest kept twice is kept once.
    pub fn stage_digests(&self, name: &str, generation: u64, fresh: bool, digests: Vec<u64>) -> Result<()> {
        if fresh {
            self.delete_rows(Kept::Digests, name, Generations::Only(generation))?;
        }
        let name = name.to_string();
        Ok(self.database().write_now_settled_at(Priority::Normal, 64 + digests.len() * 24, move |db| {
            let mut insert = db.prepare_cached("INSERT OR IGNORE INTO checkpoint_digests (name, generation, digest) VALUES (?1, ?2, ?3)")?;
            for digest in digests {
                insert.execute(params![name, generation as i64, digest.to_be_bytes().as_slice()])?;
            }
            Ok(())
        })?)
    }

    /// Of `digests`, those the producer `name` keeps under `generation`,
    /// asked in one read transaction.
    pub fn digests_among(&self, name: &str, generation: u64, digests: &[u64]) -> Result<std::collections::HashSet<u64>> {
        let (name, digests) = (name.to_string(), digests.to_vec());
        Ok(self.database().read_now(move |connection| {
            let db = connection.unchecked_transaction()?;
            let mut found = std::collections::HashSet::new();
            let mut query = db.prepare_cached("SELECT 1 FROM checkpoint_digests WHERE name = ?1 AND generation = ?2 AND digest = ?3")?;
            for digest in digests {
                if query.query_row(params![name, generation as i64, digest.to_be_bytes().as_slice()], |_| Ok(())).optional()?.is_some() {
                    found.insert(digest);
                }
            }
            Ok(found)
        })?)
    }

    /// The digests the producer `name` keeps under `generation`, rising:
    /// for comparing, and for a follower that holds them whole.
    pub fn digests_of(&self, name: &str, generation: u64) -> Result<Vec<u64>> {
        let name = name.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached("SELECT digest FROM checkpoint_digests WHERE name = ?1 AND generation = ?2 ORDER BY digest")?;
            let rows = statement.query_map(params![name, generation as i64], |row| {
                let digest: Vec<u8> = row.get(0)?;
                Ok(<[u8; 8]>::try_from(digest.as_slice()).map(u64::from_be_bytes).unwrap_or_default())
            })?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?)
    }

    /// Lets go the digests the producer `name` keeps under any generation
    /// but `keep` (all of them for none).
    pub fn sweep_digests(&self, name: &str, keep: Option<u64>) -> Result<()> {
        self.delete_rows(Kept::Digests, name, keep.map_or(Generations::All, Generations::AllBut))
    }

    /// Deletes what the producer `name` keeps beside its checkpoint under
    /// `generations` in `kept`, a batch of rows to a transaction: a window
    /// as long as a record is let go without holding the library's writer
    /// for all of it at once. Each batch settled before the next, as
    /// `keep_checkpoint_settled` is.
    fn delete_rows(&self, kept: Kept, name: &str, generations: Generations) -> Result<()> {
        let (which, generation) = match generations {
            Generations::Only(generation) => ("AND generation = ?2", Some(generation)),
            Generations::AllBut(generation) => ("AND generation <> ?2", Some(generation)),
            Generations::All => ("", None),
        };
        let (table, key) = match kept {
            Kept::Held => ("checkpoint_held", "name, generation, second, nanos, key"),
            Kept::Digests => ("checkpoint_digests", "name, generation, digest"),
        };
        let query = format!("DELETE FROM {table} WHERE ({key}) IN (SELECT {key} FROM {table} WHERE name = ?1 {which} LIMIT {DELETED_AT_ONCE})");
        loop {
            let (name, query) = (name.to_string(), query.clone());
            let deleted = self.database().write_now_settled_at(Priority::Normal, DELETED_AT_ONCE * 40, move |db| {
                let mut values = vec![Value::Text(name)];
                values.extend(generation.map(|generation| Value::Integer(generation as i64)));
                Ok(db.prepare_cached(&query)?.execute(params_from_iter(values))?)
            })?;
            if deleted < DELETED_AT_ONCE {
                return Ok(());
            }
        }
    }
}

/// How many rows of what a producer held go in one transaction.
const DELETED_AT_ONCE: usize = 4096;

/// What a producer keeps beside its checkpoint (`delete_rows`).
enum Kept {
    Held,
    Digests,
}

/// Which generations of what a producer keeps (`delete_rows`).
enum Generations {
    Only(u64),
    AllBut(u64),
    All,
}

/// The epoch of the producer `name`'s checkpoint: how often what it kept
/// was let go from beside it.
fn epoch(db: &Connection, name: &str) -> fastctx_storage::Result<u64> {
    let epoch: Option<i64> = db.prepare_cached("SELECT epoch FROM checkpoint_epochs WHERE name = ?1")?.query_row([name], |row| row.get(0)).optional()?;
    Ok(epoch.unwrap_or(0) as u64)
}

/// The epoch of the producer `name` moves on: a producer that read before
/// keeps nothing after (`keep_checkpoint_in`). In the transaction that lets
/// its checkpoint go; what it held goes after (`sweep_held`).
pub(super) fn let_go(db: &Connection, name: &str) -> fastctx_storage::Result<()> {
    db.prepare_cached("INSERT INTO checkpoint_epochs (name, epoch) VALUES (?1, 1) ON CONFLICT (name) DO UPDATE SET epoch = epoch + 1")?.execute([name])?;
    Ok(())
}
