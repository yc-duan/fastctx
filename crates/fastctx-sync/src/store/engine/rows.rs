//! Indexed head reads share the writer's exact unsigned and JSON encodings.
use crate::{
    catalog::{Lifecycle, ObjectHead},
    cursor::{LocalSequence, VersionRef},
    identity::{AuthorityEpoch, Digest, ObjectKey},
    manifest::{BlobId, DigestAlgorithm, ManifestRoot},
    store::sql,
};
use fastctx_storage::{
    Result,
    rusqlite::{self, Connection, OptionalExtension, Row, params, types::Type},
};
use serde::{Serialize, de::DeserializeOwned};

pub(super) fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| sql::invalid(e.to_string()))
}

fn json<T: DeserializeOwned>(row: &Row<'_>, column: usize) -> rusqlite::Result<T> {
    let bytes: Vec<u8> = row.get(column)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(column, Type::Blob, Box::new(e)))
}
fn version(row: &Row<'_>, epoch: usize, revision: usize) -> rusqlite::Result<VersionRef> {
    let bytes: Vec<u8> = row.get(epoch)?;
    let bytes: [u8; 16] = bytes
        .try_into()
        .map_err(|_| rusqlite::Error::InvalidColumnType(epoch, "authority epoch".into(), Type::Blob))?;
    Ok(VersionRef {
        authority_epoch: AuthorityEpoch::from_bytes(bytes),
        revision: crate::cursor::Revision(sql::counter(row, revision)?),
    })
}
pub(super) fn get(db: &Connection, key: &ObjectKey) -> Result<Option<ObjectHead>> {
    db.prepare_cached(
        "SELECT o.kind,o.schema,s.acl_revision,o.authority,o.evidence,o.authority_epoch,o.revision,o.current_authority_epoch,o.current_version,o.content,o.history,o.state,o.completeness FROM sync_objects o JOIN sync_scopes s USING(hub,scope) WHERE o.hub=?1 AND o.scope=?2 AND o.object=?3")?.query_row(
        params![key.scope.hub.to_string(),key.scope.scope.as_str(),key.object.as_str()],
        |row| {
            let content = row.get::<_,Option<Vec<u8>>>(9)?.map(|bytes|serde_json::from_slice(&bytes)).transpose().map_err(|e|rusqlite::Error::FromSqlConversionFailure(9,Type::Blob,Box::new(e)))?;
            let history = row.get::<_,Option<String>>(10)?.map(|hash| Digest::try_from(hash).map(|digest|ManifestRoot(BlobId {domain:key.scope.clone(),algorithm:DigestAlgorithm::Sha256,digest}))).transpose().map_err(|_|rusqlite::Error::InvalidColumnType(10,"history root".into(),Type::Text))?;
            let lifecycle=match row.get::<_,String>(11)?.as_str() { "live"=>Lifecycle::Live,"removed"=>Lifecycle::Removed,"deleted"=>Lifecycle::Deleted,_=>return Err(rusqlite::Error::InvalidColumnType(11,"object lifecycle".into(),Type::Text)) };
            Ok(ObjectHead { key:key.clone(),kind:row.get(0)?,schema:row.get(1)?,acl_revision:crate::cursor::Revision(sql::counter(row,2)?),authority:json(row,3)?,evidence:json(row,4)?,revision:version(row,5,6)?,current_version:version(row,7,8)?,content,history,lifecycle,completeness:json(row,12)? })
        },
    ).optional().map_err(Into::into)
}
pub(super) fn local(db: &Connection) -> Result<LocalSequence> {
    db.prepare_cached("SELECT sequence FROM sync_local_head WHERE singleton=1")?
        .query_row([], |r| sql::counter(r, 0))
        .map(LocalSequence)
        .map_err(Into::into)
}
pub(super) fn advance(db: &Connection) -> Result<LocalSequence> {
    let next = local(db)?.0.checked_add(1).ok_or_else(|| sql::invalid("local change sequence exhausted"))?;
    db.prepare_cached("UPDATE sync_local_head SET sequence=?1 WHERE singleton=1")?
        .execute([next.to_be_bytes().as_slice()])?;
    Ok(LocalSequence(next))
}
