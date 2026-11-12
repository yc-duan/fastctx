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
    // On the disk, with its place in the copy, before the backup is done: a
    // hub started from the copy opens sealed.
    fastctx_platform::publish(&to.join(BACKUP_MARKER), &serde_json::to_vec_pretty(&marker)?, fastctx_platform::Permissions::Private)?;
    keys::sync_directory(to)?;
    Ok(marker)
}

/// Makes the next service key of the stopped hub in `dir` and exports its
/// keyring, the next key in it, to `export`; the next key's public half.
pub fn rotate_prepare(dir: &Path, export: &Path) -> anyhow::Result<String> {
    let _store = Store::open(dir)?;
    let next = keys::prepare(&dir.join("keys"), export)?;
    Ok(witness::hex(next.as_bytes()))
}

/// Whether a rotation of the stopped hub in `dir` was put in use and not
/// yet written into its chain: `rotate_finish` takes it up.
pub fn rotation_unfinished(dir: &Path) -> anyhow::Result<bool> {
    Ok(fastctx_platform::disk::exists(dir.join("keys").join(FINISHING))?)
}

/// Where a rotation being put in use says which keys it is between, until
/// it is in the hub's chain and the witness.
const FINISHING: &str = "finishing";

/// Puts the next key a rotation made in use: the rotation goes into the
/// hub's chain and the witness. A rotation cut short is taken up where it
/// stopped. The old key and the new, as hex.
pub fn rotate_finish(dir: &Path) -> anyhow::Result<(String, String)> {
    let store = Store::open(dir)?;
    let keyring = dir.join("keys");
    let marker = keyring.join(FINISHING);
    let (old, new) = if fastctx_platform::disk::exists(&marker)? {
        let text = String::from_utf8(fastctx_platform::disk::read(&marker)?)?;
        let (old, new) = text.split_once(' ').ok_or_else(|| anyhow::anyhow!("{} is not two keys", marker.display()))?;
        (old.to_string(), new.trim().to_string())
    } else {
        let next = fastctx_net::sign::SecretKey::load(&keyring.join("next.key"))
            .map_err(|_| anyhow::anyhow!("no rotation is prepared: run it with --export first"))?;
        let current = fastctx_net::sign::SecretKey::load(&keyring.join("current.key"))?;
        let pair = (witness::hex(current.public().as_bytes()), witness::hex(next.public().as_bytes()));
        fastctx_platform::publish(&marker, format!("{} {}", pair.0, pair.1).as_bytes(), fastctx_platform::Permissions::Private)?;
        pair
    };
    if fastctx_platform::disk::exists(keyring.join("next.key"))? {
        keys::finish(&keyring)?;
    }
    let mut hasher = Sha256::new();
    hasher.update(old.as_bytes());
    hasher.update(new.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let (seq, head) = store.shelf.0.write_now(128, move |c| chain::append(c, "key-rotation", &digest))?;
    witness::Witness::open(dir, store.hub.id.as_str())?.advanced(seq, &head)?;
    fastctx_platform::disk::remove_file(&marker)?;
    Ok((old, new))
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
    let brought: Vec<(String, i64, i64)> = store.shelf.0.read_now(|c| {
        let mut statement = c.prepare_cached(
            "SELECT machine, MAX(seq), COUNT(*) FROM recovery_heads GROUP BY machine
             UNION ALL SELECT machine, -1, COUNT(*) FROM recovery_receipts GROUP BY machine ORDER BY 1",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    })?;
    for (machine, seq, count) in brought {
        if seq >= 0 {
            lines.push(format!("{machine} came having seen the hub's history at {seq}, past what the hub holds"));
        } else {
            lines.push(format!("{machine} came with {count} receipt(s) of the hub's that the hub no longer holds"));
        }
    }
    lines.push(
        "what is set aside: exactly the heads and receipts above, and the head outside the hub's directory; a device that saw \
         another part of the set-aside history and has not come yet seals the hub again when it does"
            .into(),
    );
    lines.push(
        "not known here: access granted or taken away after the backup. Check the accounts, shares and projects before \
         reopening: a share or a project withdrawn after the backup is in force again until it is withdrawn again"
            .into(),
    );
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
/// confirmed: what devices showed of the history it was put back from is
/// set aside, each owner library is taken as it is, and the witness holds
/// the chain as it is now. How many of the hub's records of the libraries
/// went.
pub fn reopen(dir: &Path) -> anyhow::Result<usize> {
    let store = Store::open(dir)?;
    let control = store.shelf.0.clone();
    let assets = Assets::open(dir, store.hub.id.as_str(), control.clone())?;
    let (fork, _) = control.read_now(chain::latest)?.unwrap_or((0, [0; 32]));
    let kept = assets.witness.kept();
    let old_head = witness::unhex(&kept.head).unwrap_or([0; 32]);
    let scopes: Vec<(String, String)> = control.read_now(|c| {
        let mut statement = c.prepare_cached("SELECT owner, key FROM asset_scopes")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    })?;
    let mut by_owner: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (owner, scope) in scopes {
        by_owner.entry(owner).or_default().push(scope);
    }
    let mut forgotten_checkpoints = Vec::new();
    let mut forgotten_epochs = Vec::new();
    let mut forgotten_gaps = Vec::new();
    for (key, scopes) in &by_owner {
        let Some(owner) = Owner::from_key(key) else { continue };
        let library = match assets.owners.library(&owner, |_| Ok(None))? {
            Ok(library) => library,
            Err(why) => anyhow::bail!("{why}: take it out of {} before the hub reopens", dir.join("assets").display()),
        };
        let _ = scopes;
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
    let forgotten = forgotten_checkpoints.len() + forgotten_epochs.len() + forgotten_gaps.len();
    // A checkpoint or an epoch the libraries no longer hold is of the
    // history set aside: the libraries are taken as they are, a few
    // thousand records to a commit.
    for batch in forgotten_checkpoints.chunks(4096) {
        let batch = batch.to_vec();
        control.write_now(256 + batch.len() * 128, move |c| {
            for (owner, scope) in &batch {
                c.prepare_cached("DELETE FROM hub_checkpoints WHERE owner = ?1 AND scope = ?2")?.execute(params![owner, scope])?;
            }
            Ok(())
        })?;
    }
    for batch in forgotten_epochs.chunks(4096) {
        let batch = batch.to_vec();
        control.write_now(256 + batch.len() * 160, move |c| {
            for (owner, scope, stream, epoch) in &batch {
                c.prepare_cached("DELETE FROM hub_epoch_facts WHERE owner = ?1 AND scope = ?2 AND stream = ?3 AND epoch >= ?4")?
                    .execute(params![owner, scope, stream, epoch])?;
            }
            Ok(())
        })?;
    }
    for batch in forgotten_gaps.chunks(4096) {
        let batch = batch.to_vec();
        control.write_now(256 + batch.len() * 160, move |c| {
            for (owner, scope, stream, epoch, from) in &batch {
                c.prepare_cached(
                    "DELETE FROM hub_gap_facts WHERE owner = ?1 AND scope = ?2 AND stream = ?3 AND epoch = ?4 AND from_sequence = ?5",
                )?
                .execute(params![owner, scope, stream, epoch, from])?;
            }
            Ok(())
        })?;
    }
    // Exactly what was shown under the hub's word, and the head outside
    // its directory, is the history set aside.
    let mut hasher = Sha256::new();
    hasher.update(b"fastctx-hub-branch-closed/2");
    hasher.update(fork.to_le_bytes());
    hasher.update(kept.seq.to_le_bytes());
    hasher.update(old_head);
    let digest: [u8; 32] = hasher.finalize().into();
    let old_seq = kept.seq;
    let (seq, head) = control.write_now(4096, move |c| {
        if old_seq > 0 {
            c.prepare_cached("INSERT OR IGNORE INTO closed_heads (seq, head) VALUES (?1, ?2)")?.execute(params![old_seq as i64, &old_head[..]])?;
        }
        c.prepare_cached("INSERT OR IGNORE INTO closed_heads (seq, head) SELECT seq, head FROM recovery_heads")?.execute([])?;
        c.prepare_cached(
            "INSERT OR IGNORE INTO closed_receipts (scope, change, prefix, parked, accumulated)
             SELECT scope, change, prefix, parked, accumulated FROM recovery_receipts",
        )?
        .execute([])?;
        c.prepare_cached("DELETE FROM recovery_heads")?.execute([])?;
        c.prepare_cached("DELETE FROM recovery_receipts")?.execute([])?;
        chain::append(c, "branch-closed", &digest)
    })?;
    assets.witness.reset(seq, &head)?;
    match fastctx_platform::disk::remove_file(dir.join(BACKUP_MARKER)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(forgotten)
}
