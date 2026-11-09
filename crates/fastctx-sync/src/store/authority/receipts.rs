//! A source receipt's key includes its scope, stream and epoch, including fanout.
use super::{SourceReceipt, decode, encode, namespace, prepare::Verified};
use crate::{identity::Digest, store::sql};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) fn get(db: &Connection, verified: &Verified) -> Result<Option<SourceReceipt>> {
    let b = &verified.batch;
    db.prepare_cached("SELECT request_hash,result FROM sync_operations WHERE hub=?1 AND actor=?2 AND operation=?3")?
        .query_row(
            params![b.scope.hub.to_string(), namespace(&b.scope, &b.stream, &b.epoch), b.operation.as_str()],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?
        .map(|(hash, result)| {
            if hash != verified.signed.proof.id().as_str() {
                return Err(sql::invalid("source operation has different original evidence"));
            }
            decode(&result)
        })
        .transpose()
}
pub(super) fn save(db: &Connection, batch: &crate::catalog::SourceBatch, receipt: &SourceReceipt) -> Result<()> {
    db.prepare_cached("INSERT INTO sync_operations(hub,actor,operation,request_hash,result) VALUES(?1,?2,?3,?4,?5)")?
        .execute(params![
        batch.scope.hub.to_string(),
        namespace(&batch.scope, &batch.stream, &batch.epoch),
        batch.operation.as_str(),
        receipt.proof.as_str(),
        encode(receipt)?
    ])?;
    Ok(())
}
/// Returns true only after durably preserving the first conflicting signed input.
/// A source collision does not manufacture an accepted alternative body version.
pub(super) fn conflict(db: &Connection, verified: &Verified) -> Result<bool> {
    let b = &verified.batch;
    let original=db.prepare_cached("SELECT proof FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND operation=?5")?
        .query_row(params![b.scope.hub.to_string(),b.scope.scope.as_str(),b.stream.as_str(),b.epoch.as_str(),b.operation.as_str()],|r|r.get::<_,String>(0)).optional()?;
    let Some(original) = original else { return Ok(false) };
    if original == verified.signed.proof.id().as_str() {
        return Ok(false);
    }
    record_conflict(
        db,
        &verified.bound.hub.object,
        b,
        &Digest::try_from(original).map_err(sql::invalid)?,
        &verified.signed,
    )?;
    Ok(true)
}
pub(super) fn record_conflict(
    db: &Connection,
    object: &crate::identity::ObjectKey,
    b: &crate::catalog::SourceBatch,
    original: &Digest,
    signed: &crate::proof::SignedSourceBatch,
) -> Result<()> {
    let current = db
        .prepare_cached("SELECT authority_epoch,revision FROM sync_objects WHERE hub=?1 AND scope=?2 AND object=?3")?
        .query_row(params![object.scope.hub.to_string(), object.scope.scope.as_str(), object.object.as_str()], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
        })
        .optional()?;
    let body = encode(&(namespace(&b.scope, &b.stream, &b.epoch), original, signed))?;
    let (epoch, revision) = current.map_or((None, None), |(e, r)| (Some(e), Some(r)));
    db.prepare_cached("INSERT INTO sync_conflicts(hub,scope,object,operation,base_authority_epoch,base_version,current_authority_epoch,current_version,alternative_authority_epoch,alternative_version,resolved_by,source_evidence) VALUES(?1,?2,?3,?4,NULL,NULL,?5,?6,NULL,NULL,NULL,?7) ON CONFLICT(hub,scope,object,operation) DO NOTHING")?
        .execute(params![b.scope.hub.to_string(),b.scope.scope.as_str(),object.object.as_str(),b.operation.as_str(),epoch,revision,body])?;
    Ok(())
}
