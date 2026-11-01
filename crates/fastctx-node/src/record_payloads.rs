//! Immutable JSON projections are split into 32 KiB pieces before reaching the
//! writer. The mutable record holds only a reference. Equal prefixes share
//! pieces, and replacement manifests are bounded by the 64 MiB admission limit.
//! Staging pins survive cancellation through the writer closure; maintenance
//! never removes a live pin or a root installed after its marking snapshot.
use fastctx_storage::{
    Database, Error, ErrorKind, Priority, Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex, Weak},
};

const PIECE: usize = 32 * 1024;
const LIMIT: usize = 64 * 1024 * 1024;
const MANIFEST: usize = (LIMIT / PIECE) * 68 + 2;
pub(crate) const SCHEMA:&str="
CREATE TABLE IF NOT EXISTS record_payload_chunks(conversation TEXT NOT NULL,hash TEXT NOT NULL,body BLOB NOT NULL,PRIMARY KEY(conversation,hash)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS record_payload_manifests(conversation TEXT NOT NULL,hash TEXT NOT NULL,bytes INTEGER NOT NULL,body BLOB NOT NULL,PRIMARY KEY(conversation,hash)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS record_payload_roots(conversation TEXT NOT NULL,family TEXT NOT NULL,identity TEXT NOT NULL,hash TEXT NOT NULL,PRIMARY KEY(conversation,family,identity)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS record_payload_root_hash ON record_payload_roots(conversation,hash);
CREATE TABLE IF NOT EXISTS record_payload_leases(conversation TEXT NOT NULL,identity TEXT NOT NULL,hash TEXT NOT NULL,PRIMARY KEY(conversation,identity)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS record_payload_lease_hash ON record_payload_leases(conversation,hash);
CREATE TABLE IF NOT EXISTS record_payload_versions(conversation TEXT PRIMARY KEY,version INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS record_payload_marks(conversation TEXT NOT NULL,kind INTEGER NOT NULL,hash TEXT NOT NULL,PRIMARY KEY(conversation,kind,hash)) WITHOUT ROWID;
";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reference {
    pub hash: String,
    pub bytes: u64,
}
#[derive(Clone)]
pub(crate) struct Staged {
    pub reference: Reference,
    lease: String,
    _pin: Arc<Pin>,
}
struct Pin(String);
static LIVE: LazyLock<Mutex<HashMap<String, Weak<Pin>>>> = LazyLock::new(Default::default);
static COLLECTING: LazyLock<Mutex<HashSet<(std::path::PathBuf, String)>>> = LazyLock::new(Default::default);
struct Collecting((std::path::PathBuf, String));
impl Drop for Collecting {
    fn drop(&mut self) {
        COLLECTING.lock().expect("payload collection").remove(&self.0);
    }
}
impl Drop for Pin {
    fn drop(&mut self) {
        LIVE.lock().expect("payload pins").remove(&self.0);
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn corrupt(message: impl Into<String>) -> Error {
    Error::with_kind(ErrorKind::Corrupt, message)
}
fn encoded(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| Error::new(e.to_string()))
}
fn version(db: &Connection, scope: &str) -> Result<i64> {
    Ok(db
        .query_row("SELECT version FROM record_payload_versions WHERE conversation=?1", [scope], |row| row.get(0))
        .optional()?
        .unwrap_or(0))
}
fn changed(db: &Connection, scope: &str) -> Result<()> {
    db.execute("INSERT INTO record_payload_versions(conversation,version) VALUES(?1,1) ON CONFLICT(conversation) DO UPDATE SET version=version+1",[scope])?;
    Ok(())
}

pub(crate) fn stage(database: &Database, scope: &str, bytes: Vec<u8>, priority: Priority) -> Result<Staged> {
    if bytes.len() > LIMIT {
        return Err(Error::with_kind(ErrorKind::Invalid, "record projection exceeds the 64 MiB admission limit"));
    }
    let reference = Reference { hash: hash(&bytes), bytes: bytes.len() as u64 };
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| Error::new(e.to_string()))?;
    let lease = format!("{:x}", u128::from_be_bytes(random));
    let pin = Arc::new(Pin(lease.clone()));
    LIVE.lock().expect("payload pins").insert(lease.clone(), Arc::downgrade(&pin));
    let staged = Staged { reference, lease, _pin: pin };
    let (scope, kept) = (scope.to_string(), staged.clone());
    let registering = scope.clone();
    database.write_now_at(priority, 256, move |db| {
        let kept = kept;
        db.execute(
            "INSERT INTO record_payload_leases(conversation,identity,hash) VALUES(?1,?2,?3)",
            (&registering, &kept.lease, &kept.reference.hash),
        )?;
        changed(db, &registering)
    })?;
    let mut manifest = Vec::new();
    for group in bytes.chunks(PIECE * 4) {
        let mut missing = Vec::new();
        for piece in group.chunks(PIECE) {
            let digest = hash(piece);
            manifest.push(digest.clone());
            let prior = database.read_now(|db| {
                Ok(db
                    .query_row(
                        "SELECT CASE WHEN length(body)<=32768 THEN body ELSE NULL END FROM record_payload_chunks WHERE conversation=?1 AND hash=?2",
                        (&scope, &digest),
                        |row| row.get::<_, Option<Vec<u8>>>(0),
                    )
                    .optional()?)
            })?;
            match prior {
                Some(None) => return Err(corrupt("record projection piece exceeds its bound")),
                Some(Some(prior)) if prior != piece => {
                    return Err(corrupt("record projection piece differs from its digest"));
                }
                Some(_) => {}
                None => missing.push((digest, piece.to_vec())),
            }
        }
        if missing.is_empty() {
            continue;
        }
        let (scope, pin) = (scope.clone(), staged.clone());
        database.write_now_at(priority, missing.iter().map(|(_, body)| body.len()).sum(), move |db| {
            let _pin = pin;
            for (digest, body) in missing {
                db.execute(
                    "INSERT OR IGNORE INTO record_payload_chunks(conversation,hash,body) VALUES(?1,?2,?3)",
                    params![scope, digest, body],
                )?;
            }
            Ok(())
        })?;
    }
    let body = encoded(&manifest)?;
    if body.len() > MANIFEST {
        return Err(Error::new("projection manifest exceeded its bound"));
    }
    let (scope, pin) = (scope.clone(), staged.clone());
    database.write_now_at(priority, body.len(), move |db| {
        let pin = pin;
        db.execute(
            "INSERT OR IGNORE INTO record_payload_manifests(conversation,hash,bytes,body) VALUES(?1,?2,?3,?4)",
            params![scope, pin.reference.hash, pin.reference.bytes as i64, body],
        )?;
        let matches: bool = db.query_row(
            "SELECT bytes=?3 AND body=?4 FROM record_payload_manifests WHERE conversation=?1 AND hash=?2",
            params![scope, pin.reference.hash, pin.reference.bytes as i64, body],
            |row| row.get(0),
        )?;
        if !matches {
            return Err(corrupt("record projection manifest differs from its digest"));
        }
        Ok(())
    })?;
    Ok(staged)
}

/// Read in the same SQLite transaction that read the owning record and root.
/// A detached reference alone does not protect an older revision from collection.
pub(crate) fn read(db: &Connection, scope: &str, reference: &Reference) -> Result<Vec<u8>> {
    if db.is_autocommit() {
        return Err(Error::with_kind(ErrorKind::Invalid, "record projection requires its owner read snapshot"));
    }
    if reference.bytes > LIMIT as u64 {
        return Err(corrupt("record projection size is invalid"));
    }
    let row: Option<(i64, Option<Vec<u8>>)> = db
        .query_row(
            "SELECT bytes,CASE WHEN length(body)<=139266 THEN body ELSE NULL END FROM record_payload_manifests WHERE conversation=?1 AND hash=?2",
            (scope, &reference.hash),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((bytes, body)) = row else { return Err(corrupt("record projection manifest is missing")) };
    let body = body.ok_or_else(|| corrupt("record projection manifest exceeds its bound"))?;
    if bytes < 0 || bytes as u64 != reference.bytes {
        return Err(corrupt("record projection size changed"));
    }
    let manifest: Vec<String> = serde_json::from_slice(&body).map_err(|e| corrupt(e.to_string()))?;
    if manifest.len() != usize::try_from(reference.bytes).expect("bounded projection size").div_ceil(PIECE) {
        return Err(corrupt("record projection piece count is invalid"));
    }
    let mut result = Vec::with_capacity(reference.bytes as usize);
    for digest in manifest {
        let piece: Option<Option<Vec<u8>>> = db
            .query_row(
                "SELECT CASE WHEN length(body)<=32768 THEN body ELSE NULL END FROM record_payload_chunks WHERE conversation=?1 AND hash=?2",
                (scope, &digest),
                |row| row.get(0),
            )
            .optional()?;
        let piece = piece
            .ok_or_else(|| corrupt("record projection piece is missing"))?
            .ok_or_else(|| corrupt("record projection piece exceeds its bound"))?;
        if piece.len() > PIECE || hash(&piece) != digest {
            return Err(corrupt("record projection piece is damaged"));
        }
        result.extend_from_slice(&piece);
    }
    if result.len() as u64 != reference.bytes || hash(&result) != reference.hash {
        return Err(corrupt("record projection is damaged"));
    }
    Ok(result)
}
/// Look up a small root without loading the projection. The caller uses the same
/// transaction for its owning record and any subsequent read of the payload.
pub(crate) fn root(db: &Connection, scope: &str, family: &str, key: &str) -> Result<Option<Reference>> {
    let row:Option<(String,Option<i64>)>=db.query_row("SELECT r.hash,m.bytes FROM record_payload_roots r LEFT JOIN record_payload_manifests m ON m.conversation=r.conversation AND m.hash=r.hash WHERE r.conversation=?1 AND r.family=?2 AND r.identity=?3",(scope,family,key),|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    row.map(|(hash, bytes)| {
        let bytes = bytes
            .filter(|bytes| *bytes >= 0 && *bytes <= LIMIT as i64)
            .ok_or_else(|| corrupt("record projection root has no valid manifest"))?;
        Ok(Reference { hash, bytes: bytes as u64 })
    })
    .transpose()
}
pub(crate) fn read_root(db: &Connection, scope: &str, family: &str, key: &str) -> Result<Option<Vec<u8>>> {
    if db.is_autocommit() {
        return Err(Error::with_kind(ErrorKind::Invalid, "record projection requires its owner read snapshot"));
    }
    root(db, scope, family, key)?.map(|reference| read(db, scope, &reference)).transpose()
}
/// Install or clear a root in the same transaction as its owning record.
pub(crate) fn link(db: &Connection, scope: &str, family: &str, key: &str, reference: Option<&Reference>) -> Result<()> {
    if db.is_autocommit() {
        return Err(Error::with_kind(ErrorKind::Invalid, "record projection must commit with its owner"));
    }
    let count = if let Some(reference) = reference {
        if db
            .query_row(
                "SELECT bytes FROM record_payload_manifests WHERE conversation=?1 AND hash=?2",
                (scope, &reference.hash),
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(|value| value as u64)
            != Some(reference.bytes)
        {
            return Err(corrupt("record projection was not completely staged"));
        }
        db.execute("INSERT INTO record_payload_roots(conversation,family,identity,hash) VALUES(?1,?2,?3,?4) ON CONFLICT(conversation,family,identity) DO UPDATE SET hash=excluded.hash WHERE record_payload_roots.hash!=excluded.hash",(scope,family,key,&reference.hash))?
    } else {
        db.execute(
            "DELETE FROM record_payload_roots WHERE conversation=?1 AND family=?2 AND identity=?3",
            (scope, family, key),
        )?
    };
    if count != 0 {
        changed(db, scope)?;
    }
    Ok(())
}
pub(crate) fn finish(db: &Connection, scope: &str, staged: &Staged) -> Result<()> {
    if db.execute("DELETE FROM record_payload_leases WHERE conversation=?1 AND identity=?2", (scope, &staged.lease))?
        != 0
    {
        changed(db, scope)?;
    }
    Ok(())
}

/// Only the exclusive node owner runs this collector. A live staging handle is
/// protected even when its caller cancelled while the writer still owns it.
pub(crate) fn collect(database: &Database, scope: &str) -> Result<()> {
    let key = (database.path().to_path_buf(), scope.to_string());
    if !COLLECTING.lock().expect("payload collection").insert(key.clone()) {
        return Ok(());
    }
    let _collecting = Collecting(key);
    let mut after = String::new();
    loop {
        let leases=database.read_now(|db| {
            let mut query=db.prepare_cached("SELECT identity FROM record_payload_leases WHERE conversation=?1 AND identity>?2 ORDER BY identity LIMIT 64")?;
            Ok(query.query_map((scope,&after),|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = leases.last() else { break };
        after = last.clone();
        let abandoned: Vec<_> = {
            let live = LIVE.lock().expect("payload pins");
            leases.into_iter().filter(|id| live.get(id).is_none_or(|pin| pin.strong_count() == 0)).collect()
        };
        let scope = scope.to_string();
        database.write_now_at(Priority::Background, 4096, move |db| {
            for id in abandoned {
                if db
                    .execute("DELETE FROM record_payload_leases WHERE conversation=?1 AND identity=?2", (&scope, id))?
                    != 0
                {
                    changed(db, &scope)?;
                }
            }
            Ok(())
        })?;
    }
    clear_marks(database, scope)?;
    let expected = database.read_now(|db| version(db, scope))?;
    let mut after = String::new();
    loop {
        let roots=database.read_now(|db| {
            let mut query=db.prepare_cached("SELECT hash FROM record_payload_roots WHERE conversation=?1 AND hash>?2 UNION SELECT hash FROM record_payload_leases WHERE conversation=?1 AND hash>?2 ORDER BY hash LIMIT 32")?;
            Ok(query.query_map((scope,&after),|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = roots.last() else { break };
        after = last.clone();
        for root in roots {
            let body = database.read_now(|db| {
                Ok(db
                    .query_row(
                        "SELECT CASE WHEN length(body)<=139266 THEN body ELSE NULL END FROM record_payload_manifests WHERE conversation=?1 AND hash=?2",
                        (scope, &root),
                        |row| row.get::<_, Option<Vec<u8>>>(0),
                    )
                    .optional()?)
            })?;
            let Some(body) = body else {
                let missing_root = database.read_now(|db| {
                    let tx = db.unchecked_transaction()?;
                    if version(&tx, scope)? != expected {
                        return Ok(false);
                    }
                    Ok(tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM record_payload_roots WHERE conversation=?1 AND hash=?2)",
                        (scope, &root),
                        |row| row.get::<_, bool>(0),
                    )?)
                })?;
                if missing_root {
                    return Err(corrupt("record projection root manifest is missing"));
                }
                return Ok(());
            };
            let body = body.ok_or_else(|| corrupt("record projection manifest exceeds its bound"))?;
            let chunks: Vec<String> = serde_json::from_slice(&body).map_err(|e| corrupt(e.to_string()))?;
            let mut marks = vec![(1, root)];
            marks.extend(chunks.into_iter().map(|hash| (0, hash)));
            for batch in marks.chunks(64) {
                let (scope, batch) = (scope.to_string(), batch.to_vec());
                database.write_now_at(Priority::Background, 8192, move |db| {
                    for (kind, hash) in batch {
                        db.execute(
                            "INSERT OR IGNORE INTO record_payload_marks(conversation,kind,hash) VALUES(?1,?2,?3)",
                            (&scope, kind, hash),
                        )?;
                    }
                    Ok(())
                })?;
            }
        }
    }
    for (table, kind) in [("record_payload_chunks", 0), ("record_payload_manifests", 1)] {
        let mut after = String::new();
        loop {
            let candidates = database.read_now(|db| {
                let mut query = db.prepare(&format!(
                    "SELECT hash FROM {table} WHERE conversation=?1 AND hash>?2 ORDER BY hash LIMIT 64"
                ))?;
                Ok(query
                    .query_map((scope, &after), |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            })?;
            let Some(last) = candidates.last() else { break };
            after = last.clone();
            // Deleting a row also frees its blob pages: four pieces or one
            // manifest keep a maintenance transaction near 128 KiB of payload.
            for batch in candidates.chunks(if kind == 0 { 4 } else { 1 }) {
                let (scope, batch) = (scope.to_string(), batch.to_vec());
                let stable=database.write_now_at(Priority::Background,128*1024,move |db| {
                    if version(db,&scope)?!=expected{return Ok(false);}
                    for hash in batch {db.execute(&format!("DELETE FROM {table} WHERE conversation=?1 AND hash=?2 AND NOT EXISTS(SELECT 1 FROM record_payload_marks WHERE conversation=?1 AND kind=?3 AND hash=?2)"),(&scope,hash,kind))?;}Ok(true)
                })?;
                if !stable {
                    return Ok(());
                }
            }
        }
    }
    clear_marks(database, scope)
}
fn clear_marks(database: &Database, scope: &str) -> Result<()> {
    loop {
        let scope = scope.to_string();
        let count=database.write_now_at(Priority::Background,2048,move |db|Ok(db.execute("DELETE FROM record_payload_marks WHERE (conversation,kind,hash) IN (SELECT conversation,kind,hash FROM record_payload_marks WHERE conversation=?1 LIMIT 64)",[scope])?))?;
        if count == 0 {
            return Ok(());
        }
    }
}

/// Visit scopes in bounded pages; a damaged conversation does not suppress the
/// collection or diagnostics of the following conversations.
pub(crate) fn maintain(database: &Database, mut report: impl FnMut(&str, &Error)) -> Result<()> {
    let mut after = String::new();
    loop {
        let scopes = database.read_now(|db| {
            let mut query = db.prepare_cached(
                "SELECT conversation FROM record_payload_versions WHERE conversation>?1 ORDER BY conversation LIMIT 32",
            )?;
            Ok(query.query_map([&after], |row| row.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        let Some(last) = scopes.last() else { break };
        after = last.clone();
        for scope in scopes {
            if let Err(error) = collect(database, &scope) {
                report(&scope, &error);
            }
        }
    }
    Ok(())
}
