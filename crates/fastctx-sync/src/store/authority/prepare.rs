//! Signature, shape, custody, payload decoding and supply checks precede the writer.
use super::{
    SqliteAuthority,
    access::{self, Bound},
};
use crate::blocks::authority_supply::{self, AuthoritySupplies};
use crate::{
    catalog::SourceBatch,
    cursor::{HoldingPeriod, Sequence},
    identity::{Producer, StreamId},
    manifest::ContentRef,
    proof::{EpochDescriptor, HandoverBasis, SignedSourceBatch},
    registry::{AuthorityRule, ConflictRule, REGISTRY},
    store::{
        Error,
        access::{AccessContext, Decision},
        epochs,
        ingest::PAYLOAD_BYTES,
        source_authorize::{ReceivedSourceBatch, SourcePayload},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{OptionalExtension, params},
};

pub(super) struct Verified {
    pub batch: SourceBatch,
    pub signed: SignedSourceBatch,
    pub bound: Bound,
}

pub(super) struct Prepared {
    pub batch: SourceBatch,
    pub signed: SignedSourceBatch,
    pub bound: Bound,
    pub payloads: Vec<Vec<u8>>,
    pub supplies: AuthoritySupplies,
    pub ready: Vec<ContentRef>,
    pub complete: bool,
    pub _permits: Vec<crate::admission::BytePermit>,
}
pub(super) fn verify(
    authority: &SqliteAuthority,
    context: AccessContext,
    holding: HoldingPeriod,
    signed: SignedSourceBatch,
) -> std::result::Result<Verified, Error> {
    if signed.proof.data.0.len() > fastctx_types::sync::PAGE_BYTES as usize {
        return Err(Error::Invalid("source proof exceeds its metadata budget".into()));
    }
    let claim: SourceBatch = serde_json::from_slice(&signed.proof.data.0).map_err(|e| Error::Invalid(e.to_string()))?;
    claim.validate_shape().map_err(Error::Invalid)?;
    if claim.records.len() > 256 || claim.scope != context.scope {
        return Err(Error::Invalid("source batch differs from the authenticated scope or record budget".into()));
    }
    let crate::store::access::Principal::Account { device, .. } = &context.principal else {
        return Err(Error::Denied("source connection has no authenticated account and device".into()));
    };
    if claim.producer != (Producer::Device { device: device.clone() }) {
        return Err(Error::Denied("source producer is not the authenticated device".into()));
    }
    let kind = REGISTRY.asset(&claim.kind).map_err(Error::Invalid)?.registered();
    let registration = kind.registration();
    if registration.schema != claim.schema
        || registration.conflict != ConflictRule::SingleWriter
        || !matches!(registration.authority, AuthorityRule::SourceDevice | AuthorityRule::LoginDevice)
    {
        return Err(Error::Invalid("batch differs from the installed device-source kind".into()));
    }
    if !authority.authorizers.contains_key(claim.kind.as_str()) {
        return Err(Error::Unavailable("no authority source authorizer is installed for this kind".into()));
    }
    let batch = authority.verifier.source(&signed, &initial(&claim, Sequence(0)))?;
    if batch != claim {
        return Err(Error::Invalid("verifier returned a different source batch".into()));
    }
    let op = claim.operation.clone();
    let (bound, existing) = authority
        .blocks
        .database()
        .read_now(|db| {
            let tx = db.unchecked_transaction()?;
            let bound = access::bind(authority, &tx, &context, &holding, kind, &claim.epoch)?;
            let existing = epochs::get(&tx, &claim.scope, &claim.stream, &claim.epoch)?;
            Ok((bound, existing))
        })
        .map_err(|e| sql::error(e, op.clone()))?;
    if StreamId::of(&bound.hub.object, registration.id) != claim.stream {
        return Err(Error::Invalid("source stream does not derive from the described object and kind".into()));
    }
    if let Some(signed) = existing {
        let descriptor = authority.verifier.epoch(&signed, &authority.identity)?;
        if descriptor.scope != batch.scope
            || descriptor.stream != batch.stream
            || descriptor.epoch != batch.epoch
            || descriptor.producer != batch.producer
            || descriptor.kind != batch.kind
            || descriptor.schema != batch.schema
        {
            return Err(Error::Denied("source batch differs from the hub's confirmed epoch".into()));
        }
    }
    let bytes = batch.records.iter().map(|r| u64::from(r.payload.decoded_bytes)).sum::<u64>();
    if bytes > PAYLOAD_BYTES || batch.records.iter().map(|r| r.roots.len() + 1).sum::<usize>() > 4097 {
        return Err(Error::Invalid("source batch exceeds the payload or reference budget".into()));
    }
    Ok(Verified { batch, signed, bound })
}
pub(super) async fn prepare(authority: &SqliteAuthority, verified: Verified) -> std::result::Result<Prepared, Error> {
    let Verified { batch, signed, bound } = verified;
    let op = batch.operation.clone();
    let accepted:bool=authority.blocks.database().read_now(|db| db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND proof=?3 AND state='accepted')")?
        .query_row(params![batch.scope.hub.to_string(),batch.scope.scope.as_str(),signed.proof.id().as_str()],|r|r.get(0)).map_err(Into::into))
        .map_err(|e|sql::error(e,op.clone()))?;
    let mut present_roots = std::collections::BTreeSet::new();
    let mut roots = std::collections::BTreeSet::new();
    let mut ready = Vec::new();
    let mut complete = true;
    for root in batch
        .records
        .iter()
        .flat_map(|r| std::iter::once(ContentRef::from(r.payload.clone())).chain(r.roots.iter().cloned()))
    {
        if !roots.insert(serde_json::to_vec(&root).map_err(|e| Error::Invalid(e.to_string()))?) {
            continue;
        }
        let present =
            authority.blocks.database().read_now(|db| present(db, &root)).map_err(|e| sql::error(e, op.clone()))?;
        if present {
            present_roots.insert(serde_json::to_vec(&root).map_err(|e| Error::Invalid(e.to_string()))?);
            let retained = accepted
                && authority
                    .blocks
                    .database()
                    .read_now(|db| granted(db, &bound.hub, &root))
                    .map_err(|e| sql::error(e, op.clone()))?;
            if !retained {
                ready.push(root);
            }
        } else {
            complete = false;
        }
    }
    if batch.records.iter().any(|r| {
        !present_roots
            .contains(&serde_json::to_vec(&ContentRef::from(r.payload.clone())).expect("content references serialize"))
    }) {
        return Err(Error::Unavailable("source record payload has not arrived for semantic authorization".into()));
    }
    let requests: Vec<_> = ready.iter().map(|root| (bound.hub.clone(), root.clone())).collect();
    let supplies = authority_supply::prepare(&authority.blocks, &op, &batch.stream, &requests).await?;
    let mut payloads = Vec::with_capacity(batch.records.len());
    let mut permits = Vec::new();
    for record in &batch.records {
        let (bytes, permit) = authority
            .blocks
            .stored_block(&record.payload)?
            .ok_or_else(|| Error::Unavailable("prepared source payload is no longer local".into()))?;
        payloads.push(bytes);
        permits.push(permit);
    }
    let result = Prepared { batch, signed, bound, payloads, supplies, ready, complete, _permits: permits };
    authorize(authority, &result)?;
    Ok(result)
}
pub(super) fn initial(batch: &SourceBatch, control_sequence: Sequence) -> EpochDescriptor {
    EpochDescriptor {
        scope: batch.scope.clone(),
        stream: batch.stream.clone(),
        kind: batch.kind.clone(),
        schema: batch.schema,
        epoch: batch.epoch.clone(),
        producer: batch.producer.clone(),
        predecessor: None,
        closed_through: None,
        operation: batch.operation.clone(),
        control_sequence,
        basis: HandoverBasis::Initial { registration: batch.operation.clone() },
    }
}
pub(super) fn authorize(authority: &SqliteAuthority, prepared: &Prepared) -> std::result::Result<(), Error> {
    let payloads: Vec<_> = prepared
        .batch
        .records
        .iter()
        .zip(&prepared.payloads)
        .map(|(record, bytes)| SourcePayload { record, bytes })
        .collect();
    let authorizer = &authority.authorizers[prepared.batch.kind.as_str()];
    match authorizer.authorize_received(ReceivedSourceBatch {
        custody: &prepared.bound.source,
        batch: &prepared.batch,
        payloads: &payloads,
    }) {
        Decision::Allowed { revision } if revision == prepared.bound.source.context.observed_acl => Ok(()),
        Decision::Unavailable => Err(Error::Unavailable("source authorizer has no receiving policy".into())),
        Decision::Denied { reason, .. } => Err(Error::Denied(reason)),
        _ => Err(Error::Busy),
    }
}
fn present(db: &fastctx_storage::rusqlite::Connection, root: &ContentRef) -> Result<bool> {
    let id = root.id();
    match root {
        ContentRef::Single{decoded_bytes,..}=>db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_asset_blocks WHERE hub=?1 AND scope=?2 AND hash=?3 AND decoded_bytes=?4 AND verified=1)")?
            .query_row(params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str(),decoded_bytes],|r|r.get(0)).map_err(Into::into),
        ContentRef::Manifest{..}=>db.prepare_cached("SELECT complete FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3")?
            .query_row(params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str()],|r|r.get(0)).optional().map(|v|v.unwrap_or(false)).map_err(Into::into),
    }
}

/// An exact accepted replay already owns these reads. It must not reopen its
/// terminal upload just to return the original durable receipt.
fn granted(
    db: &fastctx_storage::rusqlite::Connection,
    custody: &crate::store::access::Custody,
    root: &ContentRef,
) -> Result<bool> {
    let (form, length) = match root {
        ContentRef::Single { decoded_bytes, .. } => ("block", Some(*decoded_bytes)),
        _ => ("manifest", None),
    };
    let object = &custody.object;
    db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_asset_access WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind=?6 AND decoded_bytes IS ?7)")?
        .query_row(params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),custody.kind.registration().id,root.id().digest.as_str(),form,length],|r|r.get(0)).map_err(Into::into)
}
