//! SQL-only source installation, always inside the receiver's FULL transaction.
use super::rows::{self, decode, encode};
use crate::{
    blocks::{SqliteBlocks, replica_commit},
    catalog::{SourceBatch, SourceRecord},
    cursor::{LocalSequence, Sequence},
    identity::{Digest, ObjectId, ObjectKey, ScopeKey, StreamId},
    manifest::ContentRef,
    proof::{ConfirmedEpoch, EpochDescriptor, SignedSourceBatch},
    store::{access::Custody, replica::ReplicaContext, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::sync::feed;

pub(super) struct Target {
    pub custody: Custody,
    pub kind: String,
    pub schema: u16,
    pub wanted: Sequence,
    pub prefix: Option<Digest>,
}
pub(super) fn target(
    db: &Connection,
    blocks: &SqliteBlocks,
    context: &ReplicaContext,
    stream: &StreamId,
    proof: &Digest,
) -> Result<Target> {
    rows::current(db, &context.holding)?;
    let scope = &context.holding.scope;
    let (object,kind,schema,period,purpose,wanted,prefix)=db.prepare_cached("SELECT object,kind,schema,holding,purpose,wanted,prefix_digest FROM sync_replica_streams WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch_proof=?4")?
        .query_row(params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),proof.as_str()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u16>(2)?,r.get::<_,Vec<u8>>(3)?,r.get::<_,Vec<u8>>(4)?,sql::counter(r,5)?,r.get::<_,Option<String>>(6)?)))?;
    let key = ObjectKey { scope: scope.clone(), object: ObjectId::try_from(object).map_err(sql::invalid)? };
    let custody = rows::custody(context, &key, &kind)?;
    if encode(&custody.holding)? != period
        || encode(&custody.purpose)? != purpose
        || StreamId::of(&key, &kind) != *stream
    {
        return Err(sql::invalid("replica stream target belongs to another custody or object"));
    }
    replica_commit::authorize(db, blocks, &custody)?;
    Ok(Target {
        custody,
        kind,
        schema,
        wanted: Sequence(wanted),
        prefix: prefix.map(Digest::try_from).transpose().map_err(sql::invalid)?,
    })
}
pub(super) fn epoch(
    db: &Connection,
    scope: &ScopeKey,
    stream: &StreamId,
    proof: &Digest,
) -> Result<Option<EpochDescriptor>> {
    db.prepare_cached(
        "SELECT signed_bytes FROM sync_epoch_descriptors WHERE hub=?1 AND scope=?2 AND stream=?3 AND proof=?4",
    )?
    .query_row(params![scope.hub.to_string(), scope.scope.as_str(), stream.as_str(), proof.as_str()], |r| {
        r.get::<_, Vec<u8>>(0)
    })
    .optional()?
    .map(|b| decode(&b))
    .transpose()
}
pub(super) fn install_epoch(db: &Connection, d: &EpochDescriptor, signed: &ConfirmedEpoch) -> Result<()> {
    let scope = &d.scope;
    let id = signed.proof.id();
    if let Some(old) = epoch(db, scope, &d.stream, &id)? {
        if old != *d {
            return Err(sql::invalid("epoch proof identity was reused"));
        }
        return Ok(());
    }
    let existing: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_epoch_descriptors WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4)",
        params![scope.hub.to_string(), scope.scope.as_str(), d.stream.as_str(), d.epoch.as_str()],
        |r| r.get(0),
    )?;
    if existing {
        return Err(sql::invalid("producer epoch already has another confirmation"));
    }
    if d.predecessor.is_none() {
        let another:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sync_epoch_descriptors WHERE hub=?1 AND scope=?2 AND stream=?3 AND predecessor IS NULL)",params![scope.hub.to_string(),scope.scope.as_str(),d.stream.as_str()],|r|r.get(0))?;
        if another {
            return Err(sql::invalid("stream already has its initial epoch"));
        }
    }
    super::epoch_chain::install_fence(db, d)?;
    if d.predecessor.is_none() && d.closed_through.is_some() {
        return Err(sql::invalid("an initial epoch cannot close a predecessor"));
    }
    db.execute("INSERT INTO sync_epoch_descriptors(hub,scope,stream,epoch,producer,kind,schema,predecessor,closed_through,proof,signed_bytes,signer,signature,operation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",params![scope.hub.to_string(),scope.scope.as_str(),d.stream.as_str(),d.epoch.as_str(),encode(&d.producer)?,d.kind,d.schema,d.predecessor.as_ref().map(encode).transpose()?,d.closed_through.map(|n|n.0.to_be_bytes().to_vec()),id.as_str(),signed.proof.data.0,signed.proof.signer.0.as_slice(),signed.proof.signature.0.as_slice(),d.operation.as_str()])?;
    Ok(())
}
pub(super) fn head(
    db: &Connection,
    target: &Target,
    d: &EpochDescriptor,
    proof: &Digest,
) -> Result<(Sequence, Digest, LocalSequence)> {
    let found=db.query_row("SELECT object,producer,kind,schema,sequence,local,epoch_proof,prefix_digest,origin FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",params![d.scope.hub.to_string(),d.scope.scope.as_str(),d.stream.as_str(),d.epoch.as_str()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,String>(2)?,r.get::<_,u16>(3)?,sql::counter(r,4)?,sql::counter(r,5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,String>(8)?))).optional()?;
    match found {
        Some((object, producer, kind, schema, sequence, local, epoch, prefix, origin)) => {
            if object != target.custody.object.object.as_str()
                || producer != encode(&d.producer)?
                || kind != d.kind
                || schema != d.schema
                || epoch.as_deref() != Some(proof.as_str())
                || origin != "replicated"
            {
                return Err(sql::invalid("replica range cannot overwrite a local producer or another stream binding"));
            }
            Ok((
                Sequence(sequence),
                Digest::try_from(prefix.ok_or_else(|| sql::invalid("confirmed stream lacks a prefix"))?)
                    .map_err(sql::invalid)?,
                LocalSequence(local),
            ))
        }
        None => Ok((Sequence(0), feed::stream_genesis(proof), crate::store::local_head::current(db)?)),
    }
}
pub(super) fn replay(db: &Connection, d: &EpochDescriptor, record: &SourceRecord, proof: &Digest) -> Result<()> {
    let stored:String=db.query_row("SELECT batch_proof FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence=?5",params![d.scope.hub.to_string(),d.scope.scope.as_str(),d.stream.as_str(),d.epoch.as_str(),record.sequence.0.to_be_bytes().as_slice()],|r|r.get(0))?;
    if stored != proof.as_str() {
        return Err(sql::invalid("a replay must forward the retained original batch proof"));
    }
    Ok(())
}

pub(super) fn append(
    db: &Connection,
    target: &Target,
    d: &EpochDescriptor,
    signed: &SignedSourceBatch,
    batch: &SourceBatch,
    record: &SourceRecord,
    prefix: &Digest,
) -> Result<()> {
    let scope = &d.scope;
    let hub = scope.hub.to_string();
    let proof = signed.proof.id();
    let sequence = record.sequence.0.to_be_bytes();
    let existing:Option<String>=db.query_row("SELECT proof FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND operation=?5",params![hub,scope.scope.as_str(),d.stream.as_str(),d.epoch.as_str(),batch.operation.as_str()],|r|r.get(0)).optional()?;
    if existing.as_ref().is_some_and(|id| id != proof.as_str()) {
        return Err(sql::invalid("source batch operation already identifies another original proof"));
    }
    if existing.is_none() {
        db.execute("INSERT INTO sync_source_proofs(hub,scope,proof,stream,epoch,producer,operation,first_sequence,last_sequence,purpose,signer,signed_bytes,signature,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'accepted') ON CONFLICT(hub,scope,proof) DO NOTHING",params![hub,scope.scope.as_str(),proof.as_str(),d.stream.as_str(),d.epoch.as_str(),encode(&d.producer)?,batch.operation.as_str(),(batch.after.0+1).to_be_bytes().as_slice(),batch.through.0.to_be_bytes().as_slice(),signed.proof.purpose.domain(),signed.proof.signer.0.as_slice(),signed.proof.data.0,signed.proof.signature.0.as_slice()])?;
    }
    db.execute("INSERT INTO sync_stream_records(hub,scope,stream,epoch,sequence,operation,kind,schema,payload,causes,batch_proof,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![hub,scope.scope.as_str(),d.stream.as_str(),d.epoch.as_str(),sequence.as_slice(),record.operation.as_str(),record.kind,record.schema,encode(&record.payload)?,encode(&record.causes)?,proof.as_str(),prefix.as_str()])?;
    for root in std::iter::once(ContentRef::from(record.payload.clone())).chain(record.roots.iter().cloned()) {
        let (form, length) = replica_commit::shape(&root);
        db.execute("INSERT INTO sync_stream_roots(hub,scope,stream,epoch,sequence,hash,root_kind,decoded_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING",params![hub,scope.scope.as_str(),d.stream.as_str(),d.epoch.as_str(),sequence.as_slice(),root.id().digest.as_str(),form,length])?;
        replica_commit::adopt(db, &target.custody, &root)?;
    }
    Ok(())
}
pub(super) fn advance(
    db: &Connection,
    target: &Target,
    d: &EpochDescriptor,
    proof: &Digest,
    through: Sequence,
    prefix: &Digest,
    local: LocalSequence,
) -> Result<()> {
    let scope = &d.scope;
    db.execute("INSERT INTO sync_stream_heads(hub,scope,stream,object,producer,epoch,kind,schema,sequence,local,epoch_proof,prefix_digest,origin) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'replicated') ON CONFLICT(hub,scope,stream,epoch) DO UPDATE SET sequence=excluded.sequence,local=excluded.local,prefix_digest=excluded.prefix_digest",params![scope.hub.to_string(),scope.scope.as_str(),d.stream.as_str(),target.custody.object.object.as_str(),encode(&d.producer)?,d.epoch.as_str(),d.kind,d.schema,through.0.to_be_bytes().as_slice(),local.0.to_be_bytes().as_slice(),proof.as_str(),prefix.as_str()])?;
    if d.kind == "conversation.timeline" {
        crate::scheduler::projection_changed(db, "derived.timeline", &target.custody.object, local)?;
    }
    crate::scheduler::replica_changed(
        db,
        &target.custody.holding,
        format!("replica-content/{}", target.custody.object.object.as_str()),
        local,
    )?;
    Ok(())
}
