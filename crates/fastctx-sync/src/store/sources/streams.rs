//! A destination's signed batch joins its stream: the proof, the records, the
//! roots they retain and the head move together, and sequence numbers never gap.
//! A producer's first epoch here commits before the hub confirms it.
use super::commit::encode;
use crate::{
    blocks::source_commit,
    catalog::SourceBatch,
    cursor::{LocalSequence, StreamCursor},
    manifest::ContentRef,
    store::{
        ingest::{SourceTarget, StreamProgress},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::sync::proof::SignedSourceBatch;

struct Head {
    object: String,
    producer: Vec<u8>,
    kind: String,
    schema: u16,
    sequence: u64,
    confirmed: bool,
}

fn head(db: &Connection, target: &SourceTarget) -> Result<Option<Head>> {
    let scope = &target.custody.object.scope;
    db.prepare_cached("SELECT object,producer,kind,schema,sequence,epoch_proof FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
        .query_row(
            params![scope.hub.to_string(), scope.scope.as_str(), target.stream.as_str(), target.epoch.as_str()],
            |row| {
                Ok(Head {
                    object: row.get(0)?,
                    producer: row.get(1)?,
                    kind: row.get(2)?,
                    schema: row.get(3)?,
                    sequence: sql::counter(row, 4)?,
                    confirmed: row.get::<_, Option<String>>(5)?.is_some(),
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

/// Refuses a batch that would not continue its stream exactly.
pub(super) fn check(db: &Connection, target: &SourceTarget, batch: &SourceBatch) -> Result<()> {
    let scope = &target.custody.object.scope;
    match head(db, target)? {
        None => {
            if batch.after.0 != 0 {
                return Err(sql::invalid("a new stream epoch starts at sequence zero"));
            }
            let others: bool = db
                .prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch!=?4)")?
                .query_row(
                    params![scope.hub.to_string(), scope.scope.as_str(), target.stream.as_str(), target.epoch.as_str()],
                    |row| row.get(0),
                )?;
            if others {
                return Err(sql::invalid("a later producer epoch needs its confirmed handover first"));
            }
        }
        Some(head) => {
            if head.object != target.custody.object.object.as_str()
                || head.producer != encode(&batch.producer)?
                || head.kind != batch.kind
                || head.schema != batch.schema
            {
                return Err(sql::invalid("the stream epoch belongs to another object, producer or kind"));
            }
            if head.confirmed {
                return Err(sql::invalid("a confirmed epoch continues through the hub's acceptance path"));
            }
            if head.sequence != batch.after.0 {
                return Err(sql::invalid(format!(
                    "the stream head is at {}, not at the batch's start {}",
                    head.sequence, batch.after.0
                )));
            }
        }
    }
    let repeated: bool = db
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND operation=?5)")?
        .query_row(
            params![
                scope.hub.to_string(),
                scope.scope.as_str(),
                target.stream.as_str(),
                target.epoch.as_str(),
                batch.operation.as_str()
            ],
            |row| row.get(0),
        )?;
    if repeated {
        return Err(sql::invalid("this stream epoch already holds the batch's operation"));
    }
    Ok(())
}

/// Writes the checked batch. Every record retains its payload and declared roots,
/// each adopted from the step that supplied it.
pub(super) fn append(
    db: &Connection,
    content: &source_commit::ContentCommit<'_>,
    target: &SourceTarget,
    batch: &SourceBatch,
    signed: &SignedSourceBatch,
    local: LocalSequence,
) -> Result<StreamProgress> {
    let scope = &target.custody.object.scope;
    let hub = scope.hub.to_string();
    let (stream, epoch) = (target.stream.as_str(), target.epoch.as_str());
    let through = batch.through.0.to_be_bytes();
    let proof = signed.proof.id();
    let local = local.0.to_be_bytes();
    let changed = db
        .prepare_cached(
            "UPDATE sync_stream_heads SET sequence=?5,local=?6 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",
        )?
        .execute(params![hub, scope.scope.as_str(), stream, epoch, through.as_slice(), local.as_slice()])?;
    if changed == 0 {
        db.prepare_cached("INSERT INTO sync_stream_heads(hub,scope,stream,object,producer,epoch,kind,schema,sequence,local,epoch_proof,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,NULL,NULL)")?
            .execute(params![
                hub,
                scope.scope.as_str(),
                stream,
                target.custody.object.object.as_str(),
                encode(&batch.producer)?,
                epoch,
                batch.kind,
                batch.schema,
                through.as_slice(),
                local.as_slice()
            ])?;
    }
    db.prepare_cached("INSERT INTO sync_source_proofs(hub,scope,proof,stream,epoch,producer,operation,first_sequence,last_sequence,purpose,signer,signed_bytes,signature,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'local')")?
        .execute(params![
            hub,
            scope.scope.as_str(),
            proof.as_str(),
            stream,
            epoch,
            encode(&batch.producer)?,
            batch.operation.as_str(),
            (batch.after.0 + 1).to_be_bytes().as_slice(),
            through.as_slice(),
            signed.proof.purpose.domain(),
            signed.proof.signer.0.as_slice(),
            signed.proof.data.0,
            signed.proof.signature.0.as_slice()
        ])?;
    for record in &batch.records {
        let sequence = record.sequence.0.to_be_bytes();
        db.prepare_cached("INSERT INTO sync_stream_records(hub,scope,stream,epoch,sequence,operation,kind,schema,payload,causes,batch_proof,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,NULL)")?
            .execute(params![
                hub,
                scope.scope.as_str(),
                stream,
                epoch,
                sequence.as_slice(),
                record.operation.as_str(),
                record.kind,
                record.schema,
                encode(&record.payload)?,
                encode(&record.causes)?,
                proof.as_str()
            ])?;
        for root in std::iter::once(ContentRef::from(record.payload.clone())).chain(record.roots.iter().cloned()) {
            content.adopt(target, &root)?;
            let (form, length) = match &root {
                ContentRef::Single { decoded_bytes, .. } => ("block", Some(*decoded_bytes)),
                ContentRef::Manifest { .. } => ("manifest", None),
            };
            db.prepare_cached("INSERT INTO sync_stream_roots(hub,scope,stream,epoch,sequence,hash,root_kind,decoded_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING")?
                .execute(params![
                    hub,
                    scope.scope.as_str(),
                    stream,
                    epoch,
                    sequence.as_slice(),
                    root.id().digest.as_str(),
                    form,
                    length
                ])?;
        }
    }
    Ok(StreamProgress::LocalPending {
        cursor: StreamCursor {
            scope: scope.clone(),
            stream: target.stream.clone(),
            epoch: target.epoch.clone(),
            committed: batch.through,
        },
    })
}
