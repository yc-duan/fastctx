//! Hub-confirmed predecessor fences survive reordered descriptor delivery.
use super::{
    rows::{decode, encode},
    stream_rows::Target,
};
use crate::{
    blocks::SqliteBlocks,
    cursor::Sequence,
    identity::{Digest, ProducerEpoch, StreamId},
    proof::EpochDescriptor,
    store::{replica::ReplicaContext, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

type TargetRow = (String, String, u16, Vec<u8>, Vec<u8>);

pub(super) fn target_for_epoch(
    db: &Connection,
    blocks: &SqliteBlocks,
    context: &ReplicaContext,
    d: &EpochDescriptor,
    proof: &Digest,
) -> Result<Target> {
    let scope = &d.scope;
    let direct: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_replica_streams WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch_proof=?4)",
        params![scope.hub.to_string(), scope.scope.as_str(), d.stream.as_str(), proof.as_str()],
        |r| r.get(0),
    )?;
    if !direct {
        // The catalog head's signed predecessor chain is the only way an older
        // description acquires a target; unrelated epochs cannot borrow a stream.
        let cutoff: Vec<u8> = db.query_row(
            "SELECT closed_through FROM sync_replica_epoch_fences WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",
            params![scope.hub.to_string(), scope.scope.as_str(), d.stream.as_str(), d.epoch.as_str()],
            |r| r.get(0),
        )?;
        let origin:Option<TargetRow>=db.query_row("SELECT object,kind,schema,holding,purpose FROM sync_replica_streams WHERE hub=?1 AND scope=?2 AND stream=?3 AND holding=?4 LIMIT 1",params![scope.hub.to_string(),scope.scope.as_str(),d.stream.as_str(),encode(&context.holding)?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let (object, kind, schema, holding, purpose) =
            origin.ok_or_else(|| sql::invalid("predecessor has no current authenticated stream target"))?;
        if kind != d.kind || schema != d.schema {
            return Err(sql::invalid("predecessor changes the stream's kind or schema"));
        }
        let local = crate::store::local_head::current(db)?;
        crate::scheduler::replica_changed(
            db,
            &context.holding,
            format!("replica-stream/{}/{}", d.stream.as_str(), proof.as_str()),
            local,
        )?;
        db.execute("INSERT INTO sync_replica_streams(hub,scope,stream,epoch_proof,object,kind,schema,holding,purpose,wanted,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,NULL)",params![scope.hub.to_string(),scope.scope.as_str(),d.stream.as_str(),proof.as_str(),object,kind,schema,holding,purpose,cutoff])?;
    }
    let successor:Option<u64>=db.query_row("SELECT control_sequence FROM sync_replica_epoch_fences WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",params![scope.hub.to_string(),scope.scope.as_str(),d.stream.as_str(),d.epoch.as_str()],|r|sql::counter(r,0)).optional()?;
    if successor.is_some_and(|sequence| sequence <= d.control_sequence.0) {
        return Err(sql::invalid("a predecessor cannot follow its successor's control sequence"));
    }
    super::stream_rows::target(db, blocks, context, &d.stream, proof)
}
pub(super) fn install_fence(db: &Connection, d: &EpochDescriptor) -> Result<()> {
    let Some(previous) = &d.predecessor else { return Ok(()) };
    if previous.scope != d.scope
        || previous.stream != d.stream
        || previous.epoch == d.epoch
        || d.closed_through != Some(previous.committed)
    {
        return Err(sql::invalid("inconsistent epoch predecessor fence"));
    }
    let scope = &d.scope;
    let old:Option<(u64,String)>=db.query_row("SELECT closed_through,successor FROM sync_replica_epoch_fences WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",params![scope.hub.to_string(),scope.scope.as_str(),d.stream.as_str(),previous.epoch.as_str()],|r|Ok((sql::counter(r,0)?,r.get(1)?))).optional()?;
    if old.as_ref().is_some_and(|(cutoff, next)| *cutoff != previous.committed.0 || next != d.epoch.as_str()) {
        return Err(sql::invalid("epoch already has a different permanent handover"));
    }
    let installed: Option<u64> = db
        .query_row(
            "SELECT sequence FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",
            params![scope.hub.to_string(), scope.scope.as_str(), d.stream.as_str(), previous.epoch.as_str()],
            |r| sql::counter(r, 0),
        )
        .optional()?;
    if installed.is_some_and(|n| n > previous.committed.0) {
        return Err(sql::invalid("handover cutoff contradicts already accepted source records"));
    }
    let predecessor: Option<Vec<u8>> = db
        .query_row(
            "SELECT signed_bytes FROM sync_epoch_descriptors WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",
            params![scope.hub.to_string(), scope.scope.as_str(), d.stream.as_str(), previous.epoch.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(bytes) = predecessor {
        let prior: EpochDescriptor = decode(&bytes)?;
        if prior.control_sequence >= d.control_sequence {
            return Err(sql::invalid("epoch chain control sequence does not increase"));
        }
    }
    db.execute("INSERT INTO sync_replica_epoch_fences(hub,scope,stream,epoch,closed_through,successor,control_sequence) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT DO NOTHING",params![scope.hub.to_string(),scope.scope.as_str(),d.stream.as_str(),previous.epoch.as_str(),previous.committed.0.to_be_bytes().as_slice(),d.epoch.as_str(),d.control_sequence.0.to_be_bytes().as_slice()])?;
    Ok(())
}
pub(super) fn cutoff(db: &Connection, d: &EpochDescriptor, through: Sequence) -> Result<()> {
    let scope = &d.scope;
    let closed: Option<u64> = db
        .query_row(
            "SELECT closed_through FROM sync_replica_epoch_fences WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",
            params![scope.hub.to_string(), scope.scope.as_str(), d.stream.as_str(), d.epoch.as_str()],
            |r| sql::counter(r, 0),
        )
        .optional()?;
    if closed.is_some_and(|cutoff| through.0 > cutoff) {
        return Err(sql::invalid("source records exceed their permanent producer cutoff"));
    }
    Ok(())
}
impl super::SqliteReplica {
    /// Older descriptions are discovered through the current head's authenticated
    /// predecessor chain; fetch them with EpochLookup::Stream, then install them.
    pub fn missing_epochs(
        &self,
        context: ReplicaContext,
        stream: StreamId,
    ) -> crate::store::FutureResult<'_, Vec<ProducerEpoch>> {
        Box::pin(async move {
            self.connection(&context)?;
            self.blocks.database().read(move |db| {
                super::rows::current(db,&context.holding)?;
                let mut q=db.prepare_cached("SELECT f.epoch FROM sync_replica_epoch_fences f WHERE f.hub=?1 AND f.scope=?2 AND f.stream=?3 AND NOT EXISTS(SELECT 1 FROM sync_epoch_descriptors e WHERE e.hub=f.hub AND e.scope=f.scope AND e.stream=f.stream AND e.epoch=f.epoch) ORDER BY f.epoch LIMIT 64")?;
                q.query_map(params![context.holding.scope.hub.to_string(),context.holding.scope.scope.as_str(),stream.as_str()],|r|r.get::<_,String>(0))?.map(|r|ProducerEpoch::try_from(r?).map_err(sql::invalid)).collect()
            }).await.map_err(|e|sql::error(e,self.blocks.worker().clone()))
        })
    }
}
