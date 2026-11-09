//! Original epoch confirmations, addressed by either identity or stream epoch.
use super::read::{Result, corrupt, unavailable};
use crate::{
    identity::{Digest, ScopeKey},
    proof::{
        Bytes, ConfirmedEpoch, EpochDescriptor, EpochLookup, ProofPurpose, Signature, SignedStatement, SigningKey,
    },
};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

pub(super) fn get(db: &Connection, scope: &ScopeKey, lookup: &EpochLookup) -> Result<ConfirmedEpoch> {
    let (predicate, first, second) = match lookup {
        EpochLookup::Proof { id } => ("proof=?3", id.as_str(), ""),
        EpochLookup::Stream { stream, epoch } => ("stream=?3 AND epoch=?4", stream.as_str(), epoch.as_str()),
    };
    let query = format!(
        "SELECT proof,CASE WHEN length(signed_bytes)<=?5 THEN signed_bytes END,signer,signature,stream,epoch,producer,kind,schema FROM sync_epoch_descriptors WHERE hub=?1 AND scope=?2 AND {predicate}"
    );
    let row = db
        .prepare_cached(&query)?
        .query_row(
            params![scope.hub.to_string(), scope.scope.as_str(), first, second, fastctx_types::sync::PAGE_BYTES],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, u16>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| unavailable("the requested epoch descriptor is absent"))?;
    let (id, data, signer, signature, stream, epoch, producer, kind, schema) = row;
    let proof = statement(
        ProofPurpose::EpochHandoverV1,
        data.ok_or_else(|| unavailable("the epoch proof exceeds a page"))?,
        signer,
        signature,
    )?;
    if proof.id().as_str() != id {
        return Err(corrupt("stored epoch proof differs from its identity"));
    }
    let decoded: EpochDescriptor = serde_json::from_slice(&proof.data.0)?;
    if decoded.scope != *scope
        || decoded.stream.as_str() != stream
        || decoded.epoch.as_str() != epoch
        || decoded.producer != serde_json::from_slice(&producer)?
        || decoded.kind != kind
        || decoded.schema != schema
    {
        return Err(corrupt("stored epoch descriptor differs from its index binding"));
    }
    Ok(ConfirmedEpoch { proof })
}
pub(super) fn statement(
    purpose: ProofPurpose,
    data: Vec<u8>,
    signer: Vec<u8>,
    signature: Vec<u8>,
) -> Result<SignedStatement> {
    Ok(SignedStatement {
        purpose,
        data: Bytes(data),
        signer: SigningKey(signer.try_into().map_err(|_| corrupt("invalid stored signing key"))?),
        signature: Signature(signature.try_into().map_err(|_| corrupt("invalid stored signature"))?),
    })
}
pub(super) fn digest(text: String) -> Result<Digest> {
    Digest::try_from(text).map_err(corrupt)
}
