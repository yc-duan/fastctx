//! Where each scope of a hub's owner library stands (§6.1, §17): its
//! change number with the prefix digest there, and its parked sequence with
//! the accumulator there. The hub's checkpoints write these, and when it
//! opens it checks that each library still holds the ones it last wrote
//! outside its directory: a library put back from an older copy does not.

use super::Library;
use super::scopes::held;
use crate::error::invalid;
use crate::Result;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use sha2::Digest as _;

/// Where a scope stands; zero, with zero digests, before anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub scope: String,
    pub change: u64,
    pub prefix: [u8; 32],
    pub parked: u64,
    pub accumulated: [u8; 32],
}

fn digest(bytes: Vec<u8>) -> fastctx_storage::Result<[u8; 32]> {
    bytes.try_into().map_err(|_| invalid("a malformed digest"))
}

fn head(db: &Connection, scope: &str) -> fastctx_storage::Result<Head> {
    let mut head = Head { scope: scope.to_string(), change: 0, prefix: [0; 32], parked: 0, accumulated: [0; 32] };
    let Some((id, _, _)) = held(db, scope)? else { return Ok(head) };
    if let Some((change, prefix)) = db
        .prepare_cached("SELECT change, prefix FROM change_log WHERE scope = ?1 ORDER BY change DESC LIMIT 1")?
        .query_row([id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)))
        .optional()?
    {
        head.change = change as u64;
        head.prefix = digest(prefix)?;
    }
    if let Some((sequence, accumulated)) = db
        .prepare_cached("SELECT sequence, accumulated FROM parked_history WHERE scope = ?1 ORDER BY sequence DESC LIMIT 1")?
        .query_row([id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)))
        .optional()?
    {
        head.parked = sequence as u64;
        head.accumulated = digest(accumulated)?;
    }
    Ok(head)
}

impl Library {
    /// Where `scopes` stand here.
    pub fn heads(&self, scopes: &[String]) -> Result<Vec<Head>> {
        let scopes = scopes.to_vec();
        Ok(self.database().read_now(move |db| scopes.iter().map(|scope| head(db, scope)).collect())?)
    }

    /// Whether this library holds `head`: the change and the parked
    /// sequence it names, with the same digests.
    pub fn holds(&self, head: &Head) -> Result<bool> {
        let head = head.clone();
        Ok(self.database().read_now(move |db| {
            if head.change == 0 && head.parked == 0 {
                return Ok(true);
            }
            let Some((id, _, _)) = held(db, &head.scope)? else { return Ok(false) };
            if head.change > 0 {
                let prefix: Option<Vec<u8>> = db
                    .prepare_cached("SELECT prefix FROM change_log WHERE scope = ?1 AND change = ?2")?
                    .query_row(params![id, head.change as i64], |row| row.get(0))
                    .optional()?;
                if prefix.as_deref() != Some(&head.prefix[..]) {
                    return Ok(false);
                }
            }
            if head.parked > 0 {
                let accumulated: Option<Vec<u8>> = db
                    .prepare_cached("SELECT accumulated FROM parked_history WHERE scope = ?1 AND sequence = ?2")?
                    .query_row(params![id, head.parked as i64], |row| row.get(0))
                    .optional()?;
                if accumulated.as_deref() != Some(&head.accumulated[..]) {
                    return Ok(false);
                }
            }
            Ok(true)
        })?)
    }

    /// Whether this library holds the hub's epoch `epoch` of `stream` of
    /// `scope` as signed, its descriptor hashing to `digest`.
    pub fn holds_epoch(&self, scope: &str, stream: &str, epoch: u64, digest: &[u8; 32]) -> Result<bool> {
        let (scope, stream, digest) = (scope.to_string(), stream.to_string(), *digest);
        Ok(self.database().read_now(move |db| {
            let fact: Option<Vec<u8>> = db
                .prepare_cached(
                    "SELECT e.fact FROM hub_epochs e JOIN streams s ON s.id = e.stream JOIN scopes c ON c.id = s.scope
                     WHERE c.key = ?1 AND s.name = ?2 AND e.epoch = ?3",
                )?
                .query_row(params![scope, stream, epoch as i64], |row| row.get(0))
                .optional()?;
            Ok(fact.is_some_and(|fact| <[u8; 32]>::from(sha2::Sha256::digest(&fact)) == digest))
        })?)
    }
}
