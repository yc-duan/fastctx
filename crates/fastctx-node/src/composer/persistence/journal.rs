//! Composer checkpoint blobs are immutable between compactions. The generic
//! document row stays small when its revision changes for each keystroke.

use super::*;
pub(super) const ALIASES: &str = "document_aliases";
const CHECKPOINT: &str = "document_checkpoint";

pub(in crate::composer) fn record(
    db: &Connection,
    owner: &str,
    kind: &str,
    name: &str,
) -> StoredResult<Option<Vec<u8>>> {
    Ok(db
        .prepare_cached("SELECT content FROM composer_records WHERE owner=?1 AND kind=?2 AND name=?3")?
        .query_row(params![owner, kind, name], |row| row.get(0))
        .optional()?)
}

pub(in crate::composer) fn load(db: &Connection, key: &documents::Key) -> StoredResult<(documents::Loaded, bool)> {
    let mut loaded = documents::load_in(db, key)?;
    let checkpoint = record(db, &key.owner, CHECKPOINT, &key.name)?;
    let external = checkpoint.is_some();
    if let Some(checkpoint) = checkpoint {
        if !loaded.checkpoint.is_empty() {
            return Err(error("the composer journal has two checkpoint authorities"));
        }
        loaded.checkpoint = checkpoint;
    }
    if loaded.checkpoint.is_empty() && loaded.revision > loaded.updates.len() as u64 {
        return Err(error("the composer journal is missing its checkpoint"));
    }
    Ok((loaded, external))
}

pub(in crate::composer) fn checkpoint(db: &Connection, key: &documents::Key, bytes: &[u8]) -> StoredResult<()> {
    db.prepare_cached("INSERT INTO composer_records(owner,kind,name,content,expires) VALUES(?1,?2,?3,?4,NULL) ON CONFLICT(owner,kind,name) DO UPDATE SET content=excluded.content,expires=NULL")?
        .execute(params![key.owner,CHECKPOINT,key.name,bytes])?;
    db.prepare_cached("UPDATE documents SET checkpoint=X'' WHERE namespace=?1 AND owner=?2 AND name=?3")?
        .execute(params![key.namespace, key.owner, key.name])?;
    Ok(())
}

pub(super) fn delete(db: &Connection, key: &documents::Key) -> StoredResult<()> {
    documents::delete(db, key)?;
    db.prepare_cached("DELETE FROM composer_records WHERE owner=?1 AND name=?2 AND kind IN (?3,?4)")?
        .execute(params![key.owner, key.name, ALIASES, CHECKPOINT])?;
    Ok(())
}
