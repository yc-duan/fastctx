//! Incremental document journals. Updates live until an atomic checkpoint
//! includes them; document rows live with their owner and are deleted with it.
//! The caller supplies the document codec and serializes changes to one document.

use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
    namespace TEXT NOT NULL,
    owner TEXT NOT NULL,
    name TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    through INTEGER NOT NULL CHECK (through >= 0 AND through <= revision),
    checkpoint BLOB NOT NULL,
    PRIMARY KEY (namespace, owner, name)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS document_updates (
    namespace TEXT NOT NULL,
    owner TEXT NOT NULL,
    name TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    content BLOB NOT NULL,
    PRIMARY KEY (namespace, owner, name, revision),
    FOREIGN KEY (namespace, owner, name) REFERENCES documents(namespace, owner, name) ON DELETE CASCADE
) WITHOUT ROWID;
"#;

#[derive(Clone, Debug)]
pub struct Key {
    pub namespace: String,
    pub owner: String,
    pub name: String,
}
impl Key {
    pub fn bytes(&self) -> usize {
        self.namespace.len() + self.owner.len() + self.name.len()
    }
}

/// A consistent checkpoint and its complete, ordered tail. An empty checkpoint
/// denotes the codec's initial document; no row and revision zero are equivalent.
pub struct Loaded {
    pub revision: u64,
    pub checkpoint: Vec<u8>,
    pub updates: Vec<Vec<u8>>,
}
impl Loaded {
    pub fn tail_bytes(&self) -> usize {
        self.updates.iter().map(Vec::len).sum()
    }
}

/// Runs inside one read transaction because a concurrent writer may replace the
/// checkpoint and retire its old tail between the two indexed queries.
pub fn load(connection: &Connection, key: &Key) -> Result<Loaded> {
    let transaction = connection.unchecked_transaction()?;
    let loaded = load_in(&transaction, key)?;
    transaction.commit()?;
    Ok(loaded)
}

/// Reads within a caller's existing transaction, so an owner's metadata and all
/// of its documents come from the same committed snapshot.
pub fn load_in(transaction: &Connection, key: &Key) -> Result<Loaded> {
    if transaction.is_autocommit() {
        return Err(Error::new("reading a document requires a transaction"));
    }
    let row = transaction
        .query_row(
            "SELECT revision, through, checkpoint FROM documents WHERE namespace=?1 AND owner=?2 AND name=?3",
            params![key.namespace, key.owner, key.name],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, Vec<u8>>(2)?)),
        )
        .optional()?;
    let Some((revision, through, checkpoint)) = row else {
        return Ok(Loaded { revision: 0, checkpoint: Vec::new(), updates: Vec::new() });
    };
    let updates = {
        let mut statement = transaction.prepare_cached(
            "SELECT revision, content FROM document_updates WHERE namespace=?1 AND owner=?2 AND name=?3 AND revision>?4 ORDER BY revision",
        )?;
        let mut rows = statement.query(params![key.namespace, key.owner, key.name, through])?;
        let mut expected = through;
        let mut updates = Vec::new();
        while let Some(row) = rows.next()? {
            expected = expected.checked_add(1).ok_or_else(|| Error::new("the document revision overflowed"))?;
            if row.get::<_, i64>(0)? != expected {
                return Err(Error::new("the document journal has a missing update"));
            }
            updates.push(row.get(1)?);
        }
        if expected != revision {
            return Err(Error::new("the document journal does not reach its recorded revision"));
        }
        updates
    };
    let revision = u64::try_from(revision).map_err(|_| Error::new("the document revision is negative"))?;
    Ok(Loaded { revision, checkpoint, updates })
}

/// Appends to the caller's write transaction, permitting queue metadata and an
/// operation receipt to commit with the text. `checkpoint`, when supplied, must
/// include this update and every predecessor through `expected`; no older tail
/// is removed before the same transaction installs that replacement.
pub fn append(
    transaction: &Connection,
    key: &Key,
    expected: u64,
    update: &[u8],
    checkpoint: Option<&[u8]>,
) -> Result<u64> {
    if transaction.is_autocommit() {
        return Err(Error::new("document updates require a write transaction"));
    }
    let expected = i64::try_from(expected).map_err(|_| Error::new("the document revision overflowed"))?;
    let next = expected.checked_add(1).ok_or_else(|| Error::new("the document revision overflowed"))?;
    transaction.execute(
        "INSERT OR IGNORE INTO documents(namespace,owner,name,revision,through,checkpoint) VALUES (?1,?2,?3,0,0,X'')",
        params![key.namespace, key.owner, key.name],
    )?;
    let changed = if let Some(checkpoint) = checkpoint {
        transaction.execute(
            "UPDATE documents SET revision=?4,through=?4,checkpoint=?5 WHERE namespace=?1 AND owner=?2 AND name=?3 AND revision=?6",
            params![key.namespace, key.owner, key.name, next, checkpoint, expected],
        )?
    } else {
        transaction.execute(
            "UPDATE documents SET revision=?4 WHERE namespace=?1 AND owner=?2 AND name=?3 AND revision=?5",
            params![key.namespace, key.owner, key.name, next, expected],
        )?
    };
    if changed != 1 {
        return Err(Error::new("the document changed before its update could commit"));
    }
    if checkpoint.is_some() {
        transaction.execute(
            "DELETE FROM document_updates WHERE namespace=?1 AND owner=?2 AND name=?3 AND revision<=?4",
            params![key.namespace, key.owner, key.name, next],
        )?;
    } else {
        transaction.execute(
            "INSERT INTO document_updates(namespace,owner,name,revision,content) VALUES (?1,?2,?3,?4,?5)",
            params![key.namespace, key.owner, key.name, next, update],
        )?;
    }
    Ok(next as u64)
}

/// Deletes a document and its tail together, within the caller's transaction.
pub fn delete(transaction: &Connection, key: &Key) -> Result<()> {
    if transaction.is_autocommit() {
        return Err(Error::new("deleting a document requires a write transaction"));
    }
    transaction.execute(
        "DELETE FROM documents WHERE namespace=?1 AND owner=?2 AND name=?3",
        params![key.namespace, key.owner, key.name],
    )?;
    Ok(())
}

/// The owner lifecycle deletes only that owner's indexed rows, including tails.
pub fn delete_owner(transaction: &Connection, namespace: &str, owner: &str) -> Result<()> {
    if transaction.is_autocommit() {
        return Err(Error::new("deleting document records requires a write transaction"));
    }
    transaction.execute("DELETE FROM documents WHERE namespace=?1 AND owner=?2", params![namespace, owner])?;
    Ok(())
}
