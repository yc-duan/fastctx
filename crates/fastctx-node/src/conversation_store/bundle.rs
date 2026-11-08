//! A self-contained SQLite snapshot with deterministic, typed row proofs.
use super::schema::{self, Table, quote};
use fastctx_platform::disk;
use fastctx_storage::{
    Error, ErrorKind, Result,
    rusqlite::{Connection, OptionalExtension, Row, params_from_iter, types::Value},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read},
    path::Path,
};

pub(super) const FILE: &str = "records.sqlite3";
const FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Manifest {
    pub format: u32,
    pub conversation: String,
    pub source_machine: String,
    pub tables: Vec<Proof>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Proof {
    pub name: String,
    pub columns: Vec<String>,
    pub primary: Vec<String>,
    pub rows: u64,
    pub digest: String,
}
impl Proof {
    pub(super) fn table(&self) -> Table {
        Table { name: self.name.clone(), writable: self.columns.clone(), primary: self.primary.clone() }
    }
}

pub(super) fn write(source: &Connection, directory: &Path, conversation: &str, machine: &str) -> Result<Manifest> {
    let snapshot = source.unchecked_transaction()?;
    let tables = schema::tables(&snapshot)?;
    let temporary = tempfile::Builder::new()
        .prefix(".records-")
        .suffix(".sqlite3")
        .tempfile_in(directory)
        .map_err(|error| Error::new(error.to_string()))?;
    let output = Connection::open(temporary.path())?;
    output.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; BEGIN IMMEDIATE;
        CREATE TABLE transfer_manifest (body BLOB NOT NULL);",
    )?;
    for table in &tables {
        let sql: String = snapshot.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
            [&table.name],
            |row| row.get(0),
        )?;
        output.execute_batch(&sql)?;
    }
    let mut manifest = Manifest {
        format: FORMAT,
        conversation: conversation.into(),
        source_machine: machine.into(),
        tables: Vec::new(),
    };
    let mut order = schema::children_first(&snapshot, tables)?;
    order.reverse();
    for table in &order {
        let mut insert = output.prepare(&insert_sql(table))?;
        let mut query = snapshot.prepare(&select_sql(table, false))?;
        let mut rows = query.query([conversation])?;
        let mut digest = Sha256::new();
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let values = values(row, table.writable.len())?;
            hash_row(&mut digest, &values);
            insert.execute(params_from_iter(&values))?;
            count += 1;
        }
        manifest.tables.push(Proof {
            name: table.name.clone(),
            columns: table.writable.clone(),
            primary: table.primary.clone(),
            rows: count,
            digest: format!("{:x}", digest.finalize()),
        });
    }
    manifest.tables.sort_by(|left, right| left.name.cmp(&right.name));
    output.execute(
        "INSERT INTO transfer_manifest(body) VALUES(?1)",
        [serde_json::to_vec(&manifest).map_err(|error| Error::new(error.to_string()))?],
    )?;
    output.execute_batch("COMMIT")?;
    drop(output);
    snapshot.commit()?;
    let publish = || -> io::Result<()> {
        let mut input = disk::open(temporary.path())?;
        let mut staged = fastctx_platform::atomic::Staged::create(&directory.join(FILE), Some(0o600))?;
        let mut buffer = [0; 64 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            staged.write_all(&buffer[..read])?;
        }
        staged.commit()
    };
    publish().map_err(|error| Error::new(error.to_string()))?;
    Ok(manifest)
}

/// The name an export goes by between machines: the digest of its manifest,
/// which proves every row it holds.
pub(super) fn fingerprint(manifest: &Manifest) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(manifest).map_err(|error| Error::new(error.to_string()))?)))
}

pub(super) fn manifest(source: &Connection, conversation: &str, target: &[Table]) -> Result<Manifest> {
    let bodies = source
        .prepare("SELECT body FROM transfer_manifest LIMIT 2")?
        .query_map([], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if bodies.len() != 1 {
        return Err(invalid("conversation bundle needs exactly one manifest"));
    }
    let manifest: Manifest = serde_json::from_slice(&bodies[0]).map_err(|error| invalid(error.to_string()))?;
    if manifest.format != FORMAT || manifest.conversation != conversation {
        return Err(invalid("conversation bundle format or identity differs"));
    }
    let offered: std::collections::BTreeSet<_> = manifest.tables.iter().map(|proof| proof.name.as_str()).collect();
    if offered.len() != manifest.tables.len()
        || target.iter().any(|table| !offered.contains(table.name.as_str()) && table.name != "sync_legacy_pins")
        || offered.iter().any(|name| !target.iter().any(|table| table.name == *name))
    {
        return Err(invalid("conversation bundle table set differs from this node"));
    }
    for proof in &manifest.tables {
        let table = target.iter().find(|table| table.name == proof.name).expect("checked offered table");
        if proof.name != table.name
            || proof.columns != table.writable
            || proof.primary != table.primary
            || table.primary.is_empty()
        {
            return Err(invalid(format!("conversation bundle schema differs for {}", table.name)));
        }
        let columns = source
            .prepare(&format!("PRAGMA table_xinfo({})", quote(&table.name)))?
            .query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, i64>(6)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if columns.into_iter().filter_map(|(name, hidden)| (hidden == 0).then_some(name)).collect::<Vec<_>>()
            != table.writable
        {
            return Err(invalid(format!("conversation bundle columns differ for {}", table.name)));
        }
    }
    Ok(manifest)
}

pub(super) fn verify(db: &Connection, conversation: &str, proof: &Proof) -> Result<()> {
    let table = proof.table();
    let mut query = db.prepare(&select_sql(&table, false))?;
    let mut rows = query.query([conversation])?;
    let (mut digest, mut count) = (Sha256::new(), 0u64);
    while let Some(row) = rows.next()? {
        hash_row(&mut digest, &values(row, table.writable.len())?);
        count += 1;
    }
    if count != proof.rows || format!("{:x}", digest.finalize()) != proof.digest {
        return Err(invalid(format!("conversation bundle row proof differs for {}", table.name)));
    }
    Ok(())
}

pub(super) fn page(source: &Connection, conversation: &str, table: &Table, after: &[Value]) -> Result<Vec<Vec<Value>>> {
    let mut parameters = vec![Value::Text(conversation.into())];
    parameters.extend_from_slice(after);
    let mut statement = source.prepare(&select_sql(table, !after.is_empty()))?;
    let mut rows = statement.query(params_from_iter(&parameters))?;
    let (mut result, mut bytes) = (Vec::new(), 0);
    while let Some(row) = rows.next()? {
        let values = values(row, table.writable.len())?;
        let size = values.iter().map(value_bytes).sum::<usize>();
        if !result.is_empty() && bytes + size > 128 * 1024 {
            break;
        }
        bytes += size;
        result.push(values);
        if result.len() == 128 {
            break;
        }
    }
    Ok(result)
}

pub(super) fn key(table: &Table, values: &[Value]) -> Result<Vec<Value>> {
    table
        .primary
        .iter()
        .map(|name| {
            table
                .writable
                .iter()
                .position(|column| column == name)
                .map(|index| values[index].clone())
                .ok_or_else(|| invalid("generated primary keys cannot be imported"))
        })
        .collect()
}

pub(super) fn put(db: &Connection, table: &Table, row: &[Value]) -> Result<bool> {
    let key = key(table, row)?;
    let terms = table.primary.iter().map(|column| format!("{} IS ?", quote(column))).collect::<Vec<_>>().join(" AND ");
    let sql = format!("SELECT {} FROM {} WHERE {terms}", columns(&table.writable), quote(&table.name));
    let stored = db
        .prepare_cached(&sql)?
        .query_row(params_from_iter(&key), |row| values(row, table.writable.len()))
        .optional()?;
    if let Some(stored) = stored {
        if stored != row {
            return Err(invalid(format!("conversation import conflicts in {}", table.name)));
        }
        Ok(false)
    } else {
        db.prepare_cached(&insert_sql(table))?.execute(params_from_iter(row))?;
        Ok(true)
    }
}

pub(super) fn invalid(reason: impl Into<String>) -> Error {
    Error::with_kind(ErrorKind::Invalid, reason)
}

pub(super) fn value_bytes(value: &Value) -> usize {
    match value {
        Value::Null => 1,
        Value::Integer(_) | Value::Real(_) => 8,
        Value::Text(value) => value.len(),
        Value::Blob(value) => value.len(),
    }
}
fn values(row: &Row<'_>, count: usize) -> fastctx_storage::rusqlite::Result<Vec<Value>> {
    (0..count).map(|index| row.get(index)).collect()
}
fn columns(names: &[String]) -> String {
    names.iter().map(|name| quote(name)).collect::<Vec<_>>().join(",")
}
fn insert_sql(table: &Table) -> String {
    format!(
        "INSERT INTO {} ({}) VALUES({})",
        quote(&table.name),
        columns(&table.writable),
        vec!["?"; table.writable.len()].join(",")
    )
}
fn select_sql(table: &Table, after: bool) -> String {
    let cursor = if after {
        format!(" AND ({}) > ({})", columns(&table.primary), vec!["?"; table.primary.len()].join(","))
    } else {
        String::new()
    };
    format!(
        "SELECT {} FROM {} WHERE conversation=?{cursor} ORDER BY {}",
        columns(&table.writable),
        quote(&table.name),
        columns(&table.primary)
    )
}
fn hash_row(digest: &mut Sha256, row: &[Value]) {
    digest.update((row.len() as u64).to_be_bytes());
    for value in row {
        match value {
            Value::Null => digest.update([0]),
            Value::Integer(value) => {
                digest.update([1]);
                digest.update(value.to_be_bytes());
            }
            Value::Real(value) => {
                digest.update([2]);
                digest.update(value.to_bits().to_be_bytes());
            }
            Value::Text(value) => {
                digest.update([3]);
                digest.update((value.len() as u64).to_be_bytes());
                digest.update(value.as_bytes());
            }
            Value::Blob(value) => {
                digest.update([4]);
                digest.update((value.len() as u64).to_be_bytes());
                digest.update(value);
            }
        }
    }
}
