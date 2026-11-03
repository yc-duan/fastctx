//! A small per-row proof ties a reread source ordinal to the exact stored row,
//! including deduplication winners and counted occurrences, across restarts.
use super::{Context, LegacyRow};
use fastctx_storage::{
    Error, Result,
    rusqlite::{Connection, OptionalExtension},
};
use sha2::{Digest, Sha256};
use std::io;

/// A proof for each legacy row brought over, gone once its file is archived
/// (`maintenance::files::collect`) or its conversation goes.
pub(crate) const SCHEMA:&str="
CREATE TABLE IF NOT EXISTS migration_row_proofs (
    conversation TEXT NOT NULL, family TEXT NOT NULL, path TEXT NOT NULL, ordinal INTEGER NOT NULL,
    source_digest BLOB NOT NULL, identity TEXT, body_digest BLOB, content_hash TEXT,
    PRIMARY KEY(family,path,ordinal)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS migration_row_proofs_owner ON migration_row_proofs(conversation,family,path,ordinal);
CREATE INDEX IF NOT EXISTS migration_row_proofs_content ON migration_row_proofs(conversation,family,content_hash) WHERE identity IS NOT NULL;
";

pub(super) fn bytes(row: &LegacyRow) -> usize {
    row.body.len()
        + row.key.len()
        + row.conversation.as_ref().map_or(0, String::len)
        + row.arrival.as_ref().map_or(0, Vec::len)
        + row.category.as_ref().map_or(0, String::len)
        + row.file.as_ref().map_or(0, |file| file.path.len() + file.incarnation.len() + file.signature.digest.len())
        + 384
}

/// What a row's source said: its body and the fields kept with it. The
/// time its file was last written is not among them: it is not what the
/// file says, and a copy or a restore changes it between an import and a
/// later run's reread of the same bytes.
pub(super) fn digest(row: &LegacyRow) -> Result<Vec<u8>> {
    let mut hash = Sha256::new();
    hash.update((row.body.len() as u64).to_be_bytes());
    hash.update(&row.body);
    hash.update(
        serde_json::to_vec(&(
            &row.key,
            &row.conversation,
            &row.arrival,
            &row.category,
            row.expires,
            row.discard,
            row.replace,
            &row.content_key,
            row.counted,
        ))
        .map_err(|error| Error::new(error.to_string()))?,
    );
    Ok(hash.finalize().to_vec())
}

pub(super) fn keep(
    db: &Connection,
    owner: &str,
    family: &str,
    row: &LegacyRow,
    source_digest: Vec<u8>,
    repeated: bool,
) -> Result<()> {
    let file = row.file.as_ref().ok_or_else(|| Error::new("a migration proof needs its source ordinal"))?;
    let (identity, body_digest) = if row.discard {
        (None, None)
    } else if repeated {
        let hash = &row.content_key.as_ref().expect("repeated content").0;
        let winner=db.query_row("SELECT identity,body_digest FROM migration_row_proofs WHERE conversation=?1 AND family=?2 AND content_hash=?3 AND identity IS NOT NULL LIMIT 1",(owner,family,hash),|row|Ok((row.get::<_,String>(0)?,row.get::<_,Vec<u8>>(1)?))).optional()?;
        let (identity, digest) = match winner {
            Some(winner) => winner,
            None => {
                // An older development checkpoint did not retain ordinal proofs.
                let identity=db.query_row("SELECT identity FROM conversation_records WHERE conversation=?1 AND family=?2 AND body=?3 ORDER BY position LIMIT 1",(owner,family,&row.body),|row|row.get::<_,String>(0))?;
                (identity, Sha256::digest(&row.body).to_vec())
            }
        };
        (Some(identity), Some(digest))
    } else {
        (Some(row.key.clone()), Some(Sha256::digest(&row.body).to_vec()))
    };
    db.execute("INSERT INTO migration_row_proofs(conversation,family,path,ordinal,source_digest,identity,body_digest,content_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
        ON CONFLICT(family,path,ordinal) DO UPDATE SET conversation=excluded.conversation,source_digest=excluded.source_digest,identity=excluded.identity,body_digest=excluded.body_digest,content_hash=excluded.content_hash",
        (owner,family,&file.path,file.ordinal,source_digest,identity,body_digest,row.content_key.as_ref().map(|key|&key.0)))?;
    Ok(())
}

pub(super) fn verify(context: &Context, rows: Vec<LegacyRow>) -> io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let changed=context.database.read_background_now(|db| {
        let snapshot=db.unchecked_transaction()?;
        for row in rows {
            let file=row.file.as_ref().ok_or_else(||Error::new("a reread row needs its source ordinal"))?;
            let owner=row.conversation.as_deref().unwrap_or(&context.conversation);
            if crate::conversation_store::deleted(&snapshot,owner)? {continue;}
            let proof=snapshot.query_row("SELECT conversation,source_digest,identity,body_digest FROM migration_row_proofs WHERE family=?1 AND path=?2 AND ordinal=?3",
                (&context.family,&file.path,file.ordinal),|row|Ok((row.get::<_,String>(0)?,row.get::<_,Vec<u8>>(1)?,row.get::<_,Option<String>>(2)?,row.get::<_,Option<Vec<u8>>>(3)?))).optional()?;
            let Some((stored_owner,expected,identity,body_digest))=proof else {
                return Err(Error::new("the legacy ordinal has no committed verification proof"));
            };
            if stored_owner!=owner || expected!=digest(&row)? {
                // Said apart, for whoever reads why the original was set aside.
                let what = if stored_owner != owner {
                    "the conversation it belongs to"
                } else if body_digest.as_deref().is_some_and(|stored| stored != Sha256::digest(&row.body).as_slice()) {
                    "its body"
                } else {
                    "the fields kept with it"
                };
                return Ok(Some(format!("legacy source row {} differs when reread, in {what}",file.ordinal)));
            }
            if let Some(identity)=identity {
                let body:Vec<u8>=snapshot.query_row("SELECT body FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",(owner,&context.family,&identity),|row|row.get(0))?;
                let current=Sha256::digest(body).to_vec();
                // A replacing row may have been superseded by a later legacy row
                // of the same identity, whose own proof then accounts for the body.
                let superseded=row.replace && current.as_slice()!=body_digest.as_deref().unwrap_or_default()
                    && snapshot.query_row("SELECT EXISTS(SELECT 1 FROM migration_row_proofs WHERE family=?1 AND conversation=?2 AND identity=?3 AND body_digest=?4 AND NOT (path=?5 AND ordinal=?6))",
                        (&context.family,owner,&identity,&current,&file.path,file.ordinal),|row|row.get::<_,bool>(0))?;
                if !superseded && Some(current.as_slice())!=body_digest.as_deref() {
                    return Err(Error::new("a committed legacy row differs from its reread proof"));
                }
            }
        }
        snapshot.commit()?;
        Ok(None)
    }).map_err(io::Error::other)?;
    match changed {
        Some(reason) => Err(crate::record_migration::errors::invalid(reason)),
        None => Ok(()),
    }
}
