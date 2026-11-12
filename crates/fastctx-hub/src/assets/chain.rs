//! The hub's own history as a chain (design §6.1): each control fact it
//! commits, and each second's checkpoint of its owner libraries, takes the
//! next `hub_seq`, and `hub_head` chains it to the one before, in the same
//! transaction. One sequence number can be overtaken by another history;
//! its head cannot.

use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

/// The chain and the checkpoints, in the hub's control database.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS hub_chain (
    seq INTEGER PRIMARY KEY, head BLOB NOT NULL, kind TEXT NOT NULL, digest BLOB NOT NULL, at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS hub_checkpoints (
    owner TEXT NOT NULL, scope TEXT NOT NULL, seq INTEGER NOT NULL, change INTEGER NOT NULL, parked INTEGER NOT NULL,
    prefix BLOB NOT NULL, accumulated BLOB NOT NULL,
    PRIMARY KEY (owner, scope, seq)
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

/// The head at `seq`, if the chain has it.
pub(crate) fn head_at(c: &Connection, seq: u64) -> fastctx_storage::Result<Option<[u8; 32]>> {
    let found: Option<Vec<u8>> = c
        .prepare_cached("SELECT head FROM hub_chain WHERE seq = ?1")?
        .query_row([seq as i64], |row| row.get(0))
        .optional()?;
    found.map(digest_of).transpose()
}
