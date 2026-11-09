//! Original hub epoch evidence shared by the authority and producer confirmation.
use crate::identity::{ProducerEpoch, ScopeKey, StreamId};
use crate::{
    proof::{Bytes, ConfirmedEpoch, EpochDescriptor, ProofPurpose, Signature, SignedStatement, SigningKey},
    store::sql,
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(crate) fn get(
    db: &Connection,
    scope: &ScopeKey,
    stream: &StreamId,
    epoch: &ProducerEpoch,
) -> Result<Option<ConfirmedEpoch>> {
    db.prepare_cached("SELECT signed_bytes,signer,signature FROM sync_epoch_descriptors WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
        .query_row(params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),epoch.as_str()], |row| {
            Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,Vec<u8>>(1)?,row.get::<_,Vec<u8>>(2)?))
        }).optional()?.map(|(data,key,signature)| Ok(ConfirmedEpoch { proof: SignedStatement {
            purpose: ProofPurpose::EpochHandoverV1, data: Bytes(data),
            signer: SigningKey(key.try_into().map_err(|_|sql::invalid("invalid stored epoch signer"))?),
            signature: Signature(signature.try_into().map_err(|_|sql::invalid("invalid stored epoch signature"))?),
        }})).transpose()
}

pub(crate) fn put(db: &Connection, value: &EpochDescriptor, signed: &ConfirmedEpoch) -> Result<()> {
    if let Some(previous) = get(db, &value.scope, &value.stream, &value.epoch)? {
        return if previous == *signed {
            Ok(())
        } else {
            Err(sql::invalid("the producer epoch already has different evidence"))
        };
    }
    db.prepare_cached("INSERT INTO sync_epoch_descriptors(hub,scope,stream,epoch,producer,kind,schema,predecessor,closed_through,proof,signed_bytes,signer,signature,operation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)")?
        .execute(params![value.scope.hub.to_string(),value.scope.scope.as_str(),value.stream.as_str(),value.epoch.as_str(),
            json(&value.producer)?,value.kind,value.schema,value.predecessor.as_ref().map(json).transpose()?,
            value.closed_through.map(|v|v.0.to_be_bytes().to_vec()),signed.proof.id().as_str(),signed.proof.data.0,
            signed.proof.signer.0.as_slice(),signed.proof.signature.0.as_slice(),value.operation.as_str()])?;
    Ok(())
}
pub(crate) fn json(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| sql::invalid(e.to_string()))
}
