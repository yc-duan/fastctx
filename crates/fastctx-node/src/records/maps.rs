//! Referenced map snapshots share unchanged scalar rows. A complete data revision
//! becomes the next diff base before its owner is published; a live pin protects
//! that revision until the owner's metadata and root commit together.
use super::Rows;
use crate::leases;
use fastctx_storage::{
    Database, Error, ErrorKind, Priority, Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex, Weak},
};

pub(crate) type Values = BTreeMap<String, Vec<u8>>;
const VALUE_BYTES: usize = 32 * 1024;
const KEY_BYTES: usize = 16 * 1024;
const BATCH_BYTES: usize = 128 * 1024;
pub(crate) const SCHEMA:&str="
CREATE TABLE IF NOT EXISTS record_map_heads(conversation TEXT NOT NULL,name TEXT NOT NULL,allocated INTEGER NOT NULL,published INTEGER NOT NULL,PRIMARY KEY(conversation,name)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS record_map_revisions(conversation TEXT NOT NULL,name TEXT NOT NULL,revision INTEGER NOT NULL,state INTEGER NOT NULL,expected INTEGER NOT NULL,written INTEGER NOT NULL,hash TEXT NOT NULL,PRIMARY KEY(conversation,name,revision)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS record_map_published ON record_map_revisions(conversation,name,revision) WHERE state=2;
CREATE TABLE IF NOT EXISTS record_map_keys(conversation TEXT NOT NULL,name TEXT NOT NULL,identity TEXT NOT NULL,PRIMARY KEY(conversation,name,identity)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS record_map_entries(conversation TEXT NOT NULL,name TEXT NOT NULL,identity TEXT NOT NULL,revision INTEGER NOT NULL,body BLOB,PRIMARY KEY(conversation,name,identity,revision)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS record_map_entry_revision ON record_map_entries(conversation,name,revision,identity);
CREATE INDEX IF NOT EXISTS record_map_values ON record_map_entries(conversation,name,identity,revision) WHERE body IS NOT NULL;
CREATE TABLE IF NOT EXISTS record_map_roots(conversation TEXT NOT NULL,name TEXT NOT NULL,identity TEXT NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(conversation,name,identity)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS record_map_root_revision ON record_map_roots(conversation,name,revision);
CREATE TABLE IF NOT EXISTS record_map_pins(conversation TEXT NOT NULL,name TEXT NOT NULL,identity TEXT NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(conversation,name,identity)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS record_map_pin_revision ON record_map_pins(conversation,name,revision);
";
type Key = (PathBuf, String, String);
type Gate = Arc<tokio::sync::Mutex<()>>;
static GATES: LazyLock<leases::Registry<Key, Gate>> = LazyLock::new(Default::default);
static COLLECTING: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(Default::default);
struct Collecting(Key);
impl Drop for Collecting {
    fn drop(&mut self) {
        COLLECTING.lock().expect("map collection").remove(&self.0);
    }
}
static LIVE: LazyLock<Mutex<HashMap<String, Weak<Pin>>>> = LazyLock::new(Default::default);
struct Pin(String);
impl Drop for Pin {
    fn drop(&mut self) {
        LIVE.lock().expect("map pins").remove(&self.0);
    }
}
struct Order {
    _lease: leases::Lease<Key, Gate>,
    _permit: tokio::sync::OwnedMutexGuard<()>,
}
#[derive(Clone)]
pub(crate) struct Staged {
    conversation: String,
    name: String,
    revision: i64,
    token: String,
    _pin: Arc<Pin>,
}
fn corrupt(message: impl Into<String>) -> Error {
    Error::with_kind(ErrorKind::Corrupt, message)
}
fn invalid(message: impl Into<String>) -> Error {
    Error::with_kind(ErrorKind::Invalid, message)
}
fn digest(values: &Values) -> String {
    let mut hash = Sha256::new();
    for (key, value) in values {
        hash.update((key.len() as u64).to_be_bytes());
        hash.update(key.as_bytes());
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value);
    }
    format!("{:x}", hash.finalize())
}
fn commit<T: Send + 'static>(
    database: &Database,
    priority: Priority,
    bytes: usize,
    order: Arc<Order>,
    pin: Arc<Pin>,
    action: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
) -> Result<T> {
    // Ownership lasts through the actual commit, including a timed-out caller.
    database.write_now_then_at(priority, bytes, action, move |_| {
        let _order = order;
        let _pin = pin;
    })
}
/// Describe the prior data snapshot on the worker, then publish a complete
/// revision. The installation closure can return a Staged clone for later
/// publication, or install its owner root and finish the pin in this commit.
pub(crate) async fn stage<T: Send + 'static, R: Send + 'static>(
    database: Database,
    conversation: String,
    name: String,
    values: Values,
    priority: Priority,
    describe: impl FnOnce(Option<&Values>) -> Result<R> + Send + 'static,
    install: impl FnOnce(&Rows<'_>, &Staged) -> Result<T> + Send + 'static,
) -> Result<(T, R)> {
    let lease = GATES.get_or_insert_with(&(database.path().to_path_buf(), conversation.clone(), name.clone()), || {
        Arc::new(tokio::sync::Mutex::new(()))
    });
    let permit = Arc::clone(&lease).lock_owned().await;
    let order = Arc::new(Order { _lease: lease, _permit: permit });
    tokio::task::spawn_blocking(move || {
        stage_now(&database, conversation, name, values, priority, order, describe, install)
    })
    .await
    .map_err(|error| Error::new(error.to_string()))?
}
#[allow(clippy::too_many_arguments)]
fn stage_now<T: Send + 'static, R: Send + 'static>(
    database: &Database,
    conversation: String,
    name: String,
    values: Values,
    priority: Priority,
    order: Arc<Order>,
    describe: impl FnOnce(Option<&Values>) -> Result<R>,
    install: impl FnOnce(&Rows<'_>, &Staged) -> Result<T> + Send + 'static,
) -> Result<(T, R)> {
    for (key, value) in &values {
        if key.len() > KEY_BYTES || value.len() > VALUE_BYTES {
            return Err(invalid("map scalars exceed their row admission bound"));
        }
    }
    let hash = digest(&values);
    let (base, previous) = database.read_background_now(|db| {
        let tx = db.unchecked_transaction()?;
        let base = tx
            .query_row(
                "SELECT published FROM record_map_heads WHERE conversation=?1 AND name=?2",
                (&conversation, &name),
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let previous = if base == 0 { Values::new() } else { read_revision(&tx, &conversation, &name, base)? };
        tx.commit()?;
        Ok((base, previous))
    })?;
    let description = describe((base != 0).then_some(&previous))?;
    let mut changes: Vec<_> =
        previous.keys().filter(|key| !values.contains_key(*key)).map(|key| (key.clone(), None)).collect();
    changes.extend(
        values
            .into_iter()
            .filter(|(key, value)| previous.get(key) != Some(value))
            .map(|(key, value)| (key, Some(value))),
    );
    drop(previous);
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| Error::new(error.to_string()))?;
    let token = format!("{:x}", u128::from_be_bytes(random));
    let pin = Arc::new(Pin(token.clone()));
    LIVE.lock().expect("map pins").insert(token.clone(), Arc::downgrade(&pin));
    let (scope, map, lease, expected) = (conversation.clone(), name.clone(), token.clone(), changes.len() as i64);
    let revision = commit(database, priority, 512, order.clone(), pin.clone(), move |db| {
        db.prepare_cached(
            "INSERT OR IGNORE INTO record_map_heads(conversation,name,allocated,published) VALUES(?1,?2,0,0)",
        )?
        .execute((&scope, &map))?;
        let revision=db.prepare_cached("UPDATE record_map_heads SET allocated=allocated+1 WHERE conversation=?1 AND name=?2 AND allocated<9223372036854775807 RETURNING allocated")?.query_row((&scope,&map),|row|row.get::<_,i64>(0)).optional()?.ok_or_else(||invalid("map revision counter exhausted"))?;
        db.prepare_cached("INSERT INTO record_map_revisions(conversation,name,revision,state,expected,written,hash) VALUES(?1,?2,?3,0,?4,0,?5)")?.execute((&scope,&map,revision,expected,hash))?;
        db.prepare_cached("INSERT INTO record_map_pins(conversation,name,identity,revision) VALUES(?1,?2,?3,?4)")?
            .execute((&scope, &map, lease, revision))?;
        Ok(revision)
    })?;
    let staged = Staged { conversation, name, revision, token, _pin: pin.clone() };
    let mut changes = changes.into_iter().peekable();
    while changes.peek().is_some() {
        let mut batch = Vec::new();
        let mut bytes = 0;
        while let Some((key, value)) = changes.peek() {
            let size = key.len() + value.as_ref().map_or(0, Vec::len) + 128;
            if !batch.is_empty() && (batch.len() == 64 || bytes + size > BATCH_BYTES) {
                break;
            }
            bytes += size;
            batch.push(changes.next().expect("peeked map row"));
        }
        let kept = staged.clone();
        commit(database, priority, bytes, order.clone(), pin.clone(), move |db| {
            let count = batch.len() as i64;
            for (key, body) in batch {
                db.prepare_cached(
                    "INSERT OR IGNORE INTO record_map_keys(conversation,name,identity) VALUES(?1,?2,?3)",
                )?
                .execute((&kept.conversation, &kept.name, &key))?;
                db.prepare_cached(
                    "INSERT INTO record_map_entries(conversation,name,identity,revision,body) VALUES(?1,?2,?3,?4,?5)",
                )?
                .execute(params![kept.conversation, kept.name, key, kept.revision, body])?;
            }
            if db.prepare_cached("UPDATE record_map_revisions SET written=written+?4 WHERE conversation=?1 AND name=?2 AND revision=?3 AND state=0")?.execute((&kept.conversation,&kept.name,kept.revision,count))?!=1{return Err(invalid("map staging revision disappeared"));}
            Ok(())
        })?;
    }
    commit(database, priority, 512, order, pin, move |db| {
        if db.prepare_cached("UPDATE record_map_revisions SET state=2 WHERE conversation=?1 AND name=?2 AND revision=?3 AND state=0 AND written=expected AND EXISTS(SELECT 1 FROM record_map_pins WHERE conversation=?1 AND name=?2 AND identity=?4 AND revision=?3)")?.execute((&staged.conversation,&staged.name,staged.revision,&staged.token))?!=1{return Err(invalid("map staging is incomplete or was collected"));}
        if db
            .prepare_cached(
                "UPDATE record_map_heads SET published=?3 WHERE conversation=?1 AND name=?2 AND published=?4",
            )?
            .execute((&staged.conversation, &staged.name, staged.revision, base))?
            != 1
        {
            return Err(invalid("map diff base changed during staging"));
        }
        let rows = Rows { connection: db, conversation: &staged.conversation };
        Ok((install(&rows, &staged)?, description))
    })
}
/// Install only a still-pinned revision, in the owning record's transaction.
pub(crate) fn link(rows: &Rows<'_>, owner: &str, staged: &Staged) -> Result<()> {
    if rows.conversation() != staged.conversation {
        return Err(invalid("map snapshot belongs to another conversation"));
    }
    let ready:bool=rows.connection().prepare_cached("SELECT EXISTS(SELECT 1 FROM record_map_pins p JOIN record_map_revisions r ON r.conversation=p.conversation AND r.name=p.name AND r.revision=p.revision WHERE p.conversation=?1 AND p.name=?2 AND p.identity=?3 AND p.revision=?4 AND r.state=2)")?.query_row((&staged.conversation,&staged.name,&staged.token,staged.revision),|row|row.get(0))?;
    if !ready {
        return Err(invalid("map snapshot no longer has its staging pin"));
    }
    rows.connection().prepare_cached("INSERT INTO record_map_roots(conversation,name,identity,revision) VALUES(?1,?2,?3,?4) ON CONFLICT(conversation,name,identity) DO UPDATE SET revision=excluded.revision")?.execute((&staged.conversation,&staged.name,owner,staged.revision))?;
    Ok(())
}
pub(crate) fn finish(rows: &Rows<'_>, staged: &Staged) -> Result<()> {
    if rows.conversation() != staged.conversation {
        return Err(invalid("map snapshot belongs to another conversation"));
    }
    rows.connection()
        .prepare_cached("DELETE FROM record_map_pins WHERE conversation=?1 AND name=?2 AND identity=?3")?
        .execute((&staged.conversation, &staged.name, &staged.token))?;
    Ok(())
}
pub(crate) fn read(rows: &Rows<'_>, name: &str, owner: &str) -> Result<Option<Values>> {
    if rows.connection().is_autocommit() {
        return Err(invalid("map reads require the owning record's snapshot"));
    }
    let revision = rows
        .connection()
        .query_row(
            "SELECT revision FROM record_map_roots WHERE conversation=?1 AND name=?2 AND identity=?3",
            (rows.conversation(), name, owner),
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    revision.map(|revision| read_revision(rows.connection(), rows.conversation(), name, revision)).transpose()
}
/// Load an entire private snapshot on a worker without occupying the reader
/// reserved for ordinary pages and point lookups.
pub(crate) fn load(
    directory: &fastctx_types::layout::ConversationDir,
    name: &str,
    owner: &str,
) -> std::io::Result<Option<Values>> {
    let (database, conversation) = super::locate(directory)?;
    database
        .read_background_now(|db| {
            let tx = db.unchecked_transaction()?;
            let result = read(&Rows { connection: &tx, conversation: &conversation }, name, owner)?;
            tx.commit()?;
            Ok(result)
        })
        .map_err(std::io::Error::other)
}

fn read_revision(db: &Connection, scope: &str, name: &str, revision: i64) -> Result<Values> {
    let hash: Option<String> = db
        .query_row(
            "SELECT hash FROM record_map_revisions WHERE conversation=?1 AND name=?2 AND revision=?3 AND state=2",
            (scope, name, revision),
            |row| row.get(0),
        )
        .optional()?;
    let hash = hash.ok_or_else(|| corrupt("map snapshot revision is missing"))?;
    let mut result = Values::new();
    let mut after = String::new();
    let mut first = true;
    loop {
        let comparison = if first { ">=" } else { ">" };
        first = false;
        let mut query=db.prepare_cached(&format!("SELECT k.identity,length(e.body),CASE WHEN length(e.body)<=32768 THEN e.body ELSE NULL END FROM record_map_keys k LEFT JOIN record_map_entries e ON e.conversation=k.conversation AND e.name=k.name AND e.identity=k.identity AND e.revision=(SELECT v.revision FROM record_map_entries v JOIN record_map_revisions r ON r.conversation=v.conversation AND r.name=v.name AND r.revision=v.revision WHERE v.conversation=k.conversation AND v.name=k.name AND v.identity=k.identity AND v.revision<=?4 AND r.state=2 ORDER BY v.revision DESC LIMIT 1) WHERE k.conversation=?1 AND k.name=?2 AND k.identity {comparison} ?3 ORDER BY k.identity LIMIT 64"))?;
        let values = query
            .query_map((scope, name, &after, revision), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<Vec<u8>>>(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let Some(last) = values.last() else { break };
        after = last.0.clone();
        for (key, bytes, value) in values {
            if key.len() > KEY_BYTES {
                return Err(corrupt("map key exceeds its row bound"));
            }
            if let Some(bytes) = bytes {
                let value = value.ok_or_else(|| corrupt("map value exceeds its row bound"))?;
                if bytes as usize != value.len() {
                    return Err(corrupt("map value length changed"));
                }
                result.insert(key, value);
            }
        }
    }
    if digest(&result) != hash {
        return Err(corrupt("map snapshot content is incomplete or damaged"));
    }
    Ok(result)
}

pub(crate) fn delete_root(rows: &Rows<'_>, name: &str, owner: &str) -> Result<()> {
    rows.connection()
        .prepare_cached("DELETE FROM record_map_roots WHERE conversation=?1 AND name=?2 AND identity=?3")?
        .execute((rows.conversation(), name, owner))?;
    Ok(())
}
fn referenced(db: &Connection, scope: &str, name: &str, revision: i64, next: Option<i64>) -> Result<bool> {
    let within = |value: i64| value >= revision && next.is_none_or(|end| value < end);
    let head = db
        .prepare_cached("SELECT published FROM record_map_heads WHERE conversation=?1 AND name=?2")?
        .query_row((scope, name), |row| row.get::<_, i64>(0))
        .optional()?
        .unwrap_or(0);
    if within(head) {
        return Ok(true);
    }
    for table in ["record_map_roots", "record_map_pins"] {
        let first=db.prepare_cached(&format!("SELECT revision FROM {table} WHERE conversation=?1 AND name=?2 AND revision>=?3 ORDER BY revision LIMIT 1"))?.query_row((scope,name,revision),|row|row.get::<_,i64>(0)).optional()?;
        if first.is_some_and(within) {
            return Ok(true);
        }
    }
    Ok(false)
}
/// The exclusive node owner collects only unreferenced intervals. Every deletion
/// rechecks references in its own write transaction, so publication can continue
/// between batches without invalidating a distant marking pass.
pub(crate) fn collect(database: &Database, scope: &str, name: &str) -> Result<()> {
    let key = (database.path().to_path_buf(), scope.to_string(), name.to_string());
    if !COLLECTING.lock().expect("map collection").insert(key.clone()) {
        return Ok(());
    }
    let _collecting = Collecting(key);
    let mut after = String::new();
    loop {
        let pins=database.read_now(|db|{
            let mut query=db.prepare_cached("SELECT identity FROM record_map_pins WHERE conversation=?1 AND name=?2 AND identity>?3 ORDER BY identity LIMIT 64")?;
            Ok(query.query_map((scope,name,&after),|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = pins.last() else { break };
        after = last.clone();
        let abandoned: Vec<_> = {
            let live = LIVE.lock().expect("map pins");
            pins.into_iter().filter(|token| live.get(token).is_none_or(|pin| pin.strong_count() == 0)).collect()
        };
        let (scope, name) = (scope.to_string(), name.to_string());
        database.write_now_at(Priority::Background, 4096, move |db| {
            for token in abandoned {
                db.prepare_cached("DELETE FROM record_map_pins WHERE conversation=?1 AND name=?2 AND identity=?3")?
                    .execute((&scope, &name, token))?;
            }
            Ok(())
        })?;
    }
    let mut after = 0;
    loop {
        let revisions=database.read_now(|db|{
            let mut query=db.prepare_cached("SELECT revision FROM record_map_revisions WHERE conversation=?1 AND name=?2 AND revision>?3 AND state IN (0,1) ORDER BY revision LIMIT 16")?;
            Ok(query.query_map((scope,name,after),|row|row.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = revisions.last() else { break };
        after = *last;
        for revision in revisions {
            collect_abandoned(database, scope, name, revision)?;
        }
    }
    let mut cursor = (String::new(), 0);
    loop {
        let candidates=database.read_now(|db|{
            let mut query=db.prepare_cached("SELECT identity,revision,coalesce(length(body),0) FROM record_map_entries WHERE conversation=?1 AND name=?2 AND (identity,revision)>(?3,?4) ORDER BY identity,revision LIMIT 16")?;
            let mut result=Vec::new();let mut bytes=0;
            for row in query.query_map((scope,name,&cursor.0,cursor.1),|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?)))?{
                let (key,revision,size)=row?;if size>VALUE_BYTES as i64{return Err(corrupt("map value exceeds its row bound"));}
                if !result.is_empty()&&bytes+size as usize+key.len()+128>BATCH_BYTES{break;}
                bytes+=size as usize+key.len()+128;result.push((key,revision));
            }Ok(result)
        })?;
        let Some(last) = candidates.last() else { break };
        cursor = last.clone();
        let (scope, name) = (scope.to_string(), name.to_string());
        database.write_now_at(Priority::Background,BATCH_BYTES,move|db|{
            for (key,revision) in candidates{
                let row:Option<(bool,i64)>=db.prepare_cached("SELECT e.body IS NULL,r.state FROM record_map_entries e JOIN record_map_revisions r ON r.conversation=e.conversation AND r.name=e.name AND r.revision=e.revision WHERE e.conversation=?1 AND e.name=?2 AND e.identity=?3 AND e.revision=?4")?.query_row((&scope,&name,&key,revision),|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
                let Some((tombstone,state))=row else{continue};
                if state!=2{continue;}
                let next=db.prepare_cached("SELECT e.revision FROM record_map_entries e JOIN record_map_revisions r ON r.conversation=e.conversation AND r.name=e.name AND r.revision=e.revision WHERE e.conversation=?1 AND e.name=?2 AND e.identity=?3 AND e.revision>?4 AND r.state=2 ORDER BY e.revision LIMIT 1")?.query_row((&scope,&name,&key,revision),|row|row.get::<_,i64>(0)).optional()?;
                let discard=!referenced(db,&scope,&name,revision,next)?
                    || (tombstone && !db.prepare_cached("SELECT EXISTS(SELECT 1 FROM record_map_entries WHERE conversation=?1 AND name=?2 AND identity=?3 AND body IS NOT NULL)")?.query_row((&scope,&name,&key),|row|row.get::<_,bool>(0))?);
                if discard{
                    db.prepare_cached("DELETE FROM record_map_entries WHERE conversation=?1 AND name=?2 AND identity=?3 AND revision=?4")?.execute((&scope,&name,&key,revision))?;
                    db.prepare_cached("DELETE FROM record_map_keys WHERE conversation=?1 AND name=?2 AND identity=?3 AND NOT EXISTS(SELECT 1 FROM record_map_entries WHERE conversation=?1 AND name=?2 AND identity=?3)")?.execute((&scope,&name,&key))?;
                }
            }Ok(())
        })?;
    }
    collect_revisions(database, scope, name)
}
fn collect_abandoned(database: &Database, scope: &str, name: &str, revision: i64) -> Result<()> {
    let (marking, map) = (scope.to_string(), name.to_string());
    let garbage=database.write_now_at(Priority::Background,256,move|db|{
        if db.prepare_cached("SELECT EXISTS(SELECT 1 FROM record_map_pins WHERE conversation=?1 AND name=?2 AND revision=?3)")?.query_row((&marking,&map,revision),|row|row.get::<_,bool>(0))?{return Ok(false);}
        let owned:bool=db.prepare_cached("SELECT EXISTS(SELECT 1 FROM record_map_roots WHERE conversation=?1 AND name=?2 AND revision=?3) OR EXISTS(SELECT 1 FROM record_map_heads WHERE conversation=?1 AND name=?2 AND published=?3)")?.query_row((&marking,&map,revision),|row|row.get(0))?;
        if owned{return Err(corrupt("an unfinished map revision has a published owner"));}
        Ok(db.prepare_cached("UPDATE record_map_revisions SET state=1 WHERE conversation=?1 AND name=?2 AND revision=?3 AND state IN (0,1)")?.execute((&marking,&map,revision))?!=0)
    })?;
    if !garbage {
        return Ok(());
    }
    loop {
        let keys=database.read_now(|db|{
            let mut query=db.prepare_cached("SELECT identity,coalesce(length(body),0) FROM record_map_entries WHERE conversation=?1 AND name=?2 AND revision=?3 ORDER BY identity LIMIT 64")?;
            let mut keys=Vec::new();let mut bytes=0;
            for row in query.query_map((scope,name,revision),|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?)))?{
                let (key,size)=row?;if size>VALUE_BYTES as i64{return Err(corrupt("map value exceeds its row bound"));}
                if !keys.is_empty()&&bytes+size as usize+key.len()+128>BATCH_BYTES{break;}
                bytes+=size as usize+key.len()+128;keys.push(key);
            }Ok(keys)
        })?;
        if keys.is_empty() {
            break;
        }
        let (scope, name) = (scope.to_string(), name.to_string());
        database.write_now_at(Priority::Background,BATCH_BYTES,move|db|{
            let garbage:bool=db.prepare_cached("SELECT EXISTS(SELECT 1 FROM record_map_revisions WHERE conversation=?1 AND name=?2 AND revision=?3 AND state=1) AND NOT EXISTS(SELECT 1 FROM record_map_pins WHERE conversation=?1 AND name=?2 AND revision=?3)")?.query_row((&scope,&name,revision),|row|row.get(0))?;
            if !garbage{return Err(invalid("map collection ownership changed"));}
            for key in keys{
                db.prepare_cached("DELETE FROM record_map_entries WHERE conversation=?1 AND name=?2 AND identity=?3 AND revision=?4")?.execute((&scope,&name,&key,revision))?;
                db.prepare_cached("DELETE FROM record_map_keys WHERE conversation=?1 AND name=?2 AND identity=?3 AND NOT EXISTS(SELECT 1 FROM record_map_entries WHERE conversation=?1 AND name=?2 AND identity=?3)")?.execute((&scope,&name,key))?;
            }Ok(())
        })?;
    }
    let (scope, name) = (scope.to_string(), name.to_string());
    database.write_now_at(Priority::Background,256,move|db|{
        db.prepare_cached("DELETE FROM record_map_revisions WHERE conversation=?1 AND name=?2 AND revision=?3 AND state=1 AND NOT EXISTS(SELECT 1 FROM record_map_entries WHERE conversation=?1 AND name=?2 AND revision=?3) AND NOT EXISTS(SELECT 1 FROM record_map_pins WHERE conversation=?1 AND name=?2 AND revision=?3)")?.execute((&scope,&name,revision))?;Ok(())
    })
}
fn collect_revisions(database: &Database, scope: &str, name: &str) -> Result<()> {
    let mut after = 0;
    loop {
        let revisions=database.read_now(|db|{
            let mut query=db.prepare_cached("SELECT revision FROM record_map_revisions WHERE conversation=?1 AND name=?2 AND revision>?3 ORDER BY revision LIMIT 64")?;
            Ok(query.query_map((scope,name,after),|row|row.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = revisions.last() else { return Ok(()) };
        after = *last;
        let (scope, name) = (scope.to_string(), name.to_string());
        database.write_now_at(Priority::Background,8192,move|db|{
            for revision in revisions{
                db.prepare_cached("DELETE FROM record_map_revisions WHERE conversation=?1 AND name=?2 AND revision=?3 AND state=2 AND NOT EXISTS(SELECT 1 FROM record_map_entries WHERE conversation=?1 AND name=?2 AND revision=?3) AND NOT EXISTS(SELECT 1 FROM record_map_roots WHERE conversation=?1 AND name=?2 AND revision=?3) AND NOT EXISTS(SELECT 1 FROM record_map_pins WHERE conversation=?1 AND name=?2 AND revision=?3) AND NOT EXISTS(SELECT 1 FROM record_map_heads WHERE conversation=?1 AND name=?2 AND published=?3)")?.execute((&scope,&name,revision))?;
            }Ok(())
        })?;
    }
}

/// Check the root without materializing its complete contents on a writer.
pub(crate) fn exists(rows: &Rows<'_>, name: &str, owner: &str) -> Result<bool> {
    Ok(rows
        .connection()
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM record_map_roots WHERE conversation=?1 AND name=?2 AND identity=?3)",
        )?
        .query_row((rows.conversation(), name, owner), |row| row.get(0))?)
}

/// One damaged map does not stop maintenance of other conversations.
pub(crate) fn maintain(database: &Database, mut report: impl FnMut(&str, &Error)) -> Result<()> {
    let mut after = (String::new(), String::new());
    loop {
        let maps = database.read_now(|db| {
            let mut query = db.prepare_cached("SELECT conversation,name FROM record_map_heads WHERE (conversation,name)>(?1,?2) ORDER BY conversation,name LIMIT 32")?;
            Ok(query.query_map((&after.0,&after.1), |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = maps.last() else {
            return Ok(());
        };
        after = last.clone();
        for (scope, name) in maps {
            if let Err(error) = collect(database, &scope, &name) {
                report(&scope, &error);
            }
        }
    }
}
