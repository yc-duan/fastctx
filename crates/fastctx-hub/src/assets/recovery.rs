//! A hub's own tools for backups, key rotation and recovery (design §6.1,
//! §17), each run from the hub's machine while the hub is stopped.
//!
//! A backup made here says it is one (`backup-marker`), so a hub started
//! from it opens sealed whether or not anyone runs a restore. A rotation
//! makes the next service key, signed by the one in use, and exports the
//! keyring with it before it is put in use. A hub that opened sealed is
//! reopened only after its administrator saw the report and confirmed: the
//! history it was put back from is put aside as a closed branch — heads
//! past where it forked, receipts past where each scope stood — so a device
//! that brings any of it is told it is closed, forgets it and gives back
//! what the hub lacks, instead of sealing the hub again.

use super::{Assets, BACKUP_MARKER, Owner, chain, keys, witness};
use crate::store::Store;
use fastctx_storage::rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

/// What a backup's marker says.
#[derive(Debug, Serialize, Deserialize)]
pub struct Marker {
    /// When the backup was made, in milliseconds since 1970.
    pub made: i64,
    pub hub: String,
    /// The chain's number then.
    pub seq: u64,
}

/// Copies the stopped hub in `dir` to `to`, marked as a backup; what the
/// marker says.
pub fn backup(dir: &Path, to: &Path) -> anyhow::Result<Marker> {
    let store = Store::open(dir)?;
    let seq = store.shelf.0.read_now(chain::latest)?.map_or(0, |(seq, _)| seq);
    keys::copy_tree(dir, to)?;
    let marker = Marker { made: jiff::Timestamp::now().as_millisecond(), hub: store.hub.id.to_string(), seq };
    fastctx_platform::disk::write(to.join(BACKUP_MARKER), serde_json::to_vec_pretty(&marker)?)?;
    Ok(marker)
}

/// Makes the next service key of the stopped hub in `dir` and exports its
/// keyring, the next key in it, to `export`; the next key's public half.
pub fn rotate_prepare(dir: &Path, export: &Path) -> anyhow::Result<String> {
    let _store = Store::open(dir)?;
    let next = keys::prepare(&dir.join("keys"), export)?;
    Ok(witness::hex(next.as_bytes()))
}

/// Puts the next key a rotation made in use: the rotation goes into the
/// hub's chain and the witness. The old key and the new, as hex.
pub fn rotate_finish(dir: &Path) -> anyhow::Result<(String, String)> {
    let store = Store::open(dir)?;
    let (old, new) = keys::finish(&dir.join("keys"))?;
    let mut hasher = Sha256::new();
    hasher.update(old.as_bytes());
    hasher.update(new.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let (seq, head) = store.shelf.0.write_now(128, move |c| chain::append(c, "key-rotation", &digest))?;
    witness::Witness::open(dir, store.hub.id.as_str())?.advanced(seq, &head)?;
    Ok((witness::hex(old.as_bytes()), witness::hex(new.as_bytes())))
}

/// What the stopped hub in `dir` is to report before it reopens: why its
/// assets are sealed, and what devices brought while they were; `None` when
/// they are not sealed.
pub fn report(dir: &Path) -> anyhow::Result<Option<Vec<String>>> {
    let store = Store::open(dir)?;
    let assets = Assets::open(dir, store.hub.id.as_str(), store.shelf.0.clone())?;
    let mut lines = Vec::new();
    if let Some(why) = assets.sealed() {
        lines.push(format!("sealed: {why}"));
    }
    for (owner, why) in owners_behind(&assets)? {
        lines.push(format!("the library of {} cannot be served: {why}", owner.key()));
    }
    if lines.is_empty() {
        return Ok(None);
    }
    match fastctx_platform::disk::read(dir.join(BACKUP_MARKER)) {
        Ok(bytes) => {
            if let Ok(marker) = serde_json::from_slice::<Marker>(&bytes) {
                let made = jiff::Timestamp::from_millisecond(marker.made).map(|at| at.to_string()).unwrap_or_default();
                lines.push(format!("a backup made {made}, at the hub's history {}", marker.seq));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let (seq, _) = store.shelf.0.read_now(chain::latest)?.unwrap_or((0, [0; 32]));
    let kept = assets.witness.kept();
    lines.push(format!("the hub's history reaches {seq}; outside its directory it last reached {}", kept.seq));
    let brought: Vec<(String, i64, i64, i64)> = store.shelf.0.read_now(|c| {
        let mut statement = c.prepare_cached("SELECT machine, seq, receipts, at FROM recovery_seen ORDER BY machine")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    })?;
    for (machine, seen, receipts, at) in brought {
        let at = jiff::Timestamp::from_millisecond(at).map(|at| at.to_string()).unwrap_or_default();
        lines.push(format!("{machine} came at {at}, having seen the history at {seen}, with {receipts} receipt(s) not yet checkpointed"));
    }
    Ok(Some(lines))
}

/// The owner libraries that cannot be served as they are, and why.
fn owners_behind(assets: &Assets) -> anyhow::Result<Vec<(Owner, String)>> {
    let owners: Vec<String> = assets.control.read_now(|c| {
        let mut statement = c.prepare_cached("SELECT DISTINCT owner FROM asset_scopes")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    })?;
    let mut behind = Vec::new();
    for key in owners {
        let Some(owner) = Owner::from_key(&key) else { continue };
        if let Err(why) = assets.owners.library(&owner, |library| assets.behind(&owner, library))? {
            behind.push((owner, why));
        }
    }
    Ok(behind)
}

/// Reopens the stopped hub in `dir`, whose administrator saw the report and
/// confirmed: the history it was put back from is a closed branch, each
/// owner library is taken as it is, and the witness holds the chain as it
/// is now. How many scopes' places were kept.
pub fn reopen(dir: &Path) -> anyhow::Result<usize> {
    let store = Store::open(dir)?;
    let control = store.shelf.0.clone();
    let assets = Assets::open(dir, store.hub.id.as_str(), control.clone())?;
    let (fork, _) = control.read_now(chain::latest)?.unwrap_or((0, [0; 32]));
    let kept = assets.witness.kept();
    let old_head = witness::unhex(&kept.head).unwrap_or([0; 32]);
    let brought: Option<i64> = control.read_now(|c| Ok(c.query_row("SELECT MAX(seq) FROM recovery_seen", [], |row| row.get(0))?))?;
    let top = kept.seq.max(brought.unwrap_or(0).max(0) as u64).max(fork);
    // Where each scope stands now, and the hub's records of it no library
    // here holds any more.
    let scopes: Vec<(String, String)> = control.read_now(|c| {
        let mut statement = c.prepare_cached("SELECT owner, key FROM asset_scopes")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    })?;
    let mut by_owner: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (owner, scope) in scopes {
        by_owner.entry(owner).or_default().push(scope);
    }
    let mut places = Vec::new();
    let mut forgotten_checkpoints = Vec::new();
    let mut forgotten_epochs = Vec::new();
    let mut forgotten_gaps = Vec::new();
    for (key, scopes) in &by_owner {
        let Some(owner) = Owner::from_key(key) else { continue };
        let library = match assets.owners.library(&owner, |_| Ok(None))? {
            Ok(library) => library,
            Err(why) => anyhow::bail!("{why}: take it out of {} before the hub reopens", dir.join("assets").display()),
        };
        for head in library.heads(scopes).map_err(|error| anyhow::anyhow!("{error}"))? {
            places.push((head.scope.clone(), head.change, head.parked));
        }
        for head in assets.checkpointed(&owner)?.values() {
            if !library.holds(head).map_err(|error| anyhow::anyhow!("{error}"))? {
                forgotten_checkpoints.push((key.clone(), head.scope.clone()));
            }
        }
        let owner_key = key.clone();
        let epochs: Vec<(String, String, i64, Vec<u8>)> = control.read_now(move |c| {
            let mut statement = c.prepare_cached("SELECT scope, stream, epoch, digest FROM hub_epoch_facts WHERE owner = ?1")?;
            let rows = statement.query_map([&owner_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?;
        for (scope, stream, epoch, digest) in epochs {
            let digest = <[u8; 32]>::try_from(digest).unwrap_or([0; 32]);
            if !library.holds_epoch(&scope, &stream, epoch as u64, &digest).map_err(|error| anyhow::anyhow!("{error}"))? {
                forgotten_epochs.push((key.clone(), scope, stream, epoch));
            }
        }
        let owner_key = key.clone();
        let gaps: Vec<(String, String, i64, i64, Vec<u8>)> = control.read_now(move |c| {
            let mut statement =
                c.prepare_cached("SELECT scope, stream, epoch, from_sequence, digest FROM hub_gap_facts WHERE owner = ?1")?;
            let rows = statement.query_map([&owner_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?;
        for (scope, stream, epoch, from, digest) in gaps {
            let digest = <[u8; 32]>::try_from(digest).unwrap_or([0; 32]);
            if !library.holds_gap(&scope, &stream, epoch as u64, from as u64, &digest).map_err(|error| anyhow::anyhow!("{error}"))? {
                forgotten_gaps.push((key.clone(), scope, stream, epoch, from));
            }
        }
    }
    let kept_places = places.len();
    let mut hasher = Sha256::new();
    hasher.update(b"fastctx-hub-branch-closed/1");
    hasher.update(fork.to_le_bytes());
    hasher.update(top.to_le_bytes());
    hasher.update(old_head);
    let digest: [u8; 32] = hasher.finalize().into();
    let at = jiff::Timestamp::now().as_millisecond();
    let (seq, head) = control.write_now(4096 + places.len() * 96, move |c| {
        c.prepare_cached("INSERT INTO closed_branches (fork, top, head, at) VALUES (?1, ?2, ?3, ?4)")?
            .execute(params![fork as i64, top as i64, &old_head[..], at])?;
        let branch = c.last_insert_rowid();
        let mut place = c.prepare_cached("INSERT INTO closed_scopes (branch, scope, change, parked) VALUES (?1, ?2, ?3, ?4)")?;
        for (scope, change, parked) in &places {
            place.execute(params![branch, scope, *change as i64, *parked as i64])?;
        }
        // A checkpoint or an epoch the libraries no longer hold is of the
        // closed branch: the libraries are taken as they are.
        for (owner, scope) in &forgotten_checkpoints {
            c.prepare_cached("DELETE FROM hub_checkpoints WHERE owner = ?1 AND scope = ?2")?.execute(params![owner, scope])?;
        }
        for (owner, scope, stream, epoch) in &forgotten_epochs {
            c.prepare_cached("DELETE FROM hub_epoch_facts WHERE owner = ?1 AND scope = ?2 AND stream = ?3 AND epoch >= ?4")?
                .execute(params![owner, scope, stream, epoch])?;
        }
        for (owner, scope, stream, epoch, from) in &forgotten_gaps {
            c.prepare_cached(
                "DELETE FROM hub_gap_facts WHERE owner = ?1 AND scope = ?2 AND stream = ?3 AND epoch = ?4 AND from_sequence = ?5",
            )?
            .execute(params![owner, scope, stream, epoch, from])?;
        }
        c.prepare_cached("DELETE FROM recovery_seen")?.execute([])?;
        chain::append(c, "branch-closed", &digest)
    })?;
    assets.witness.reset(seq, &head)?;
    match fastctx_platform::disk::remove_file(dir.join(BACKUP_MARKER)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(kept_places)
}
