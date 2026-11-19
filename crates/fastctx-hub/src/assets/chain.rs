//! The hub's own history as a chain (design §6.1): each control fact it
//! commits, and each second's checkpoint of its owner libraries, takes the
//! next `hub_seq`, and `hub_head` chains it to the one before, in the same
//! transaction. One sequence number can be overtaken by another history;
//! its head cannot.

use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

/// The chain, the checkpoints, and the epochs the hub opened as control
/// facts, in the hub's control database.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS hub_chain (
    seq INTEGER PRIMARY KEY, head BLOB NOT NULL, kind TEXT NOT NULL, digest BLOB NOT NULL, at INTEGER NOT NULL
);
-- Each scope's latest checkpoint: where it stood at which link of the chain.
CREATE TABLE IF NOT EXISTS hub_checkpoints (
    owner TEXT NOT NULL, scope TEXT NOT NULL, seq INTEGER NOT NULL, change INTEGER NOT NULL, parked INTEGER NOT NULL,
    prefix BLOB NOT NULL, accumulated BLOB NOT NULL,
    PRIMARY KEY (owner, scope)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS asset_machines (machine TEXT PRIMARY KEY) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS hub_gap_facts (
    owner TEXT NOT NULL, scope TEXT NOT NULL, stream TEXT NOT NULL, epoch INTEGER NOT NULL, from_sequence INTEGER NOT NULL,
    digest BLOB NOT NULL, seq INTEGER NOT NULL,
    PRIMARY KEY (owner, scope, stream, epoch, from_sequence)
) WITHOUT ROWID;
-- What machines brought, under the hub's word, while every asset was
-- sealed (design §17): the heads they saw and the receipts they held. When
-- the hub reopens, exactly these, and the head outside its directory, are
-- the history it set aside; nothing else is taken for it.
CREATE TABLE IF NOT EXISTS recovery_heads (
    seq INTEGER NOT NULL, head BLOB NOT NULL, machine TEXT NOT NULL, at INTEGER NOT NULL,
    judged INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (seq, head)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS recovery_receipts (
    scope TEXT NOT NULL, change INTEGER NOT NULL, prefix BLOB NOT NULL, parked INTEGER NOT NULL, accumulated BLOB NOT NULL,
    machine TEXT NOT NULL, at INTEGER NOT NULL, judged INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (scope, change, prefix, parked, accumulated)
) WITHOUT ROWID;
-- Why every asset is sealed for a recovery, once the hub found it was put
-- back: kept, so a plain restart seals it again; gone only as the hub is
-- reopened (design §17).
CREATE TABLE IF NOT EXISTS asset_seal (
    id INTEGER PRIMARY KEY CHECK (id = 1), why TEXT NOT NULL, at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS closed_heads (seq INTEGER NOT NULL, head BLOB NOT NULL, PRIMARY KEY (seq, head)) WITHOUT ROWID;
-- A missing signed head at a sequence retired by an operator after recovery.
-- The slot applies only while the chain contains that exact recovery anchor.
CREATE TABLE IF NOT EXISTS closed_head_slots (
    seq INTEGER NOT NULL CHECK (seq > 0),
    anchor_seq INTEGER NOT NULL CHECK (anchor_seq > seq),
    anchor_head BLOB NOT NULL CHECK (length(anchor_head) = 32),
    PRIMARY KEY (seq, anchor_seq, anchor_head)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS closed_receipts (
    scope TEXT NOT NULL, change INTEGER NOT NULL, prefix BLOB NOT NULL, parked INTEGER NOT NULL, accumulated BLOB NOT NULL,
    PRIMARY KEY (scope, change, prefix, parked, accumulated)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS hub_epoch_facts (
    owner TEXT NOT NULL, scope TEXT NOT NULL, stream TEXT NOT NULL, epoch INTEGER NOT NULL, digest BLOB NOT NULL,
    seq INTEGER NOT NULL,
    PRIMARY KEY (owner, scope, stream, epoch)
) WITHOUT ROWID;
";

fn digest_of(bytes: Vec<u8>) -> fastctx_storage::Result<[u8; 32]> {
    bytes.try_into().map_err(|_| fastctx_storage::Error::new("a malformed chain head"))
}

/// Adds `kind` with `digest` to the chain; its number and head.
pub(crate) fn append(c: &Connection, kind: &str, digest: &[u8; 32]) -> fastctx_storage::Result<(u64, [u8; 32])> {
    let (seq, previous) = latest(c)?.unwrap_or((0, [0; 32]));
    let seq = seq + 1;
    let mut hasher = Sha256::new();
    hasher.update(b"fastctx-hub-chain/1");
    hasher.update(previous);
    hasher.update(seq.to_le_bytes());
    hasher.update(digest);
    let head: [u8; 32] = hasher.finalize().into();
    let at = jiff::Timestamp::now().as_millisecond();
    c.prepare_cached("INSERT INTO hub_chain (seq, head, kind, digest, at) VALUES (?1, ?2, ?3, ?4, ?5)")?
        .execute(params![seq as i64, &head[..], kind, &digest[..], at])?;
    Ok((seq, head))
}

/// The chain's last number and head.
pub(crate) fn latest(c: &Connection) -> fastctx_storage::Result<Option<(u64, [u8; 32])>> {
    let found: Option<(i64, Vec<u8>)> = c
        .prepare_cached("SELECT seq, head FROM hub_chain ORDER BY seq DESC LIMIT 1")?
        .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    found.map(|(seq, head)| Ok((seq as u64, digest_of(head)?))).transpose()
}

/// The head at `seq`, if the chain has it: before its first fact, the
/// chain's start, which every hub has.
pub(crate) fn head_at(c: &Connection, seq: u64) -> fastctx_storage::Result<Option<[u8; 32]>> {
    if seq == 0 {
        return Ok(Some([0; 32]));
    }
    let found: Option<Vec<u8>> = c
        .prepare_cached("SELECT head FROM hub_chain WHERE seq = ?1")?
        .query_row([seq as i64], |row| row.get(0))
        .optional()?;
    found.map(digest_of).transpose()
}

/// Whether a verified head belongs to history that the hub has closed.
/// If the directory is ever put back before the recovery anchor, a slot
/// must not hide that rollback. A slot never changes the canonical chain.
pub(crate) fn closed_head(c: &Connection, seq: u64, head: &[u8; 32]) -> fastctx_storage::Result<bool> {
    let Ok(seq) = i64::try_from(seq) else { return Ok(false) };
    Ok(c.prepare_cached(
        "SELECT 1 FROM closed_heads WHERE seq = ?1 AND head = ?2
         UNION ALL
         SELECT 1 FROM closed_head_slots AS slot
         JOIN hub_chain AS anchor ON anchor.seq = slot.anchor_seq AND anchor.head = slot.anchor_head
         WHERE slot.seq = ?1 AND anchor.kind = 'branch-closed'
         LIMIT 1",
    )?
    .exists(params![seq, &head[..]])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_slot_requires_its_exact_recovery_anchor() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        let (first, first_head) = append(&db, "checkpoint", &[1; 32]).unwrap();
        let (second, _) = append(&db, "checkpoint", &[2; 32]).unwrap();
        let (anchor, anchor_head) = append(&db, "branch-closed", &[3; 32]).unwrap();
        db.execute(
            "INSERT INTO closed_head_slots VALUES (?1, ?2, ?3)",
            params![first as i64, anchor as i64, &anchor_head[..]],
        )
        .unwrap();
        db.execute("INSERT INTO closed_heads VALUES (?1, ?2)", params![first as i64, &first_head[..]]).unwrap();

        assert!(closed_head(&db, first, &[4; 32]).unwrap());
        assert!(!closed_head(&db, second, &[4; 32]).unwrap());
        db.execute("UPDATE hub_chain SET head = ?1 WHERE seq = ?2", params![&[5_u8; 32][..], anchor as i64])
            .unwrap();
        assert!(!closed_head(&db, first, &[4; 32]).unwrap());
        db.execute("DELETE FROM hub_chain WHERE seq = ?1", [anchor as i64]).unwrap();
        assert!(!closed_head(&db, first, &[4; 32]).unwrap());
        assert!(closed_head(&db, first, &first_head).unwrap());
    }
}
