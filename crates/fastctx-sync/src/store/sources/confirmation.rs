//! A hub confirmation installs evidence first. Prefix construction resumes from a
//! durable cursor and never changes a producer's record identities or epoch.
use super::SqliteSources;
use crate::{
    cursor::{LocalSequence, Sequence},
    identity::{Producer, ProducerEpoch, ScopeKey, StreamId},
    proof::{ConfirmedEpoch, HandoverBasis},
    scheduler::{Lease, Outcome, TaskHandler, TaskKey},
    store::{
        Error, FutureResult,
        access::AccessContext,
        boundary::{self, Role},
        epochs, local_head, sql,
    },
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::sync::feed;

pub(super) const PREFIX: &str = "source-prefix/";

impl SqliteSources {
    pub(super) async fn confirm(&self, context: AccessContext, signed: ConfirmedEpoch) -> Result<(), Error> {
        if signed.proof.data.0.len() > fastctx_types::sync::PAGE_BYTES as usize {
            return Err(Error::Invalid("epoch evidence exceeds one metadata page".into()));
        }
        let (identity, verifier) = self
            .hubs
            .get(&context.scope.hub)
            .ok_or_else(|| Error::Unavailable("no trusted hub confirmation layer is installed".into()))?;
        let value = verifier.epoch(&signed, identity)?;
        if value.scope != context.scope
            || value.predecessor.is_some()
            || value.closed_through.is_some()
            || !matches!(value.basis, HandoverBasis::Initial { ref registration } if registration==&value.operation)
            || !matches!((self.blocks.holder(), &value.producer),
                (crate::cursor::Holder::Device { device: local }, Producer::Device { device }) if local==device)
        {
            return Err(Error::Denied("confirmation is not this producer's initial local epoch".into()));
        }
        let installed = self
            .authorizers
            .get(value.kind.as_str())
            .ok_or_else(|| Error::Unavailable("no source authorizer is installed for the confirmed kind".into()))?;
        let _ = installed;
        let registration = crate::registry::REGISTRY.asset(&value.kind).map_err(Error::Invalid)?;
        if registration.registration().schema != value.schema {
            return Err(Error::Invalid("confirmation schema differs from the installed kind".into()));
        }
        let op = value.operation.clone();
        self.blocks.database().write(fastctx_types::sync::PAGE_BYTES as usize*2, move |db| {
            let _boundary = boundary::enter_typed_write(Role::Source);
            let scope = &value.scope;
            let valid: bool = db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_scopes WHERE hub=?1 AND scope=?2 AND enabled=1 AND acl_revision=?3)")?
                .query_row(params![scope.hub.to_string(),scope.scope.as_str(),context.observed_acl.0.to_be_bytes().as_slice()], |r|r.get(0))?;
            if !valid { return Err(sql::invalid("confirmation's scope or ACL is no longer current")); }
            let head = db.prepare_cached("SELECT producer,kind,schema,epoch_proof FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
                .query_row(params![scope.hub.to_string(),scope.scope.as_str(),value.stream.as_str(),value.epoch.as_str()], |r|
                    Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,String>(1)?,r.get::<_,u16>(2)?,r.get::<_,Option<String>>(3)?)))
                .optional()?.ok_or_else(||sql::invalid("confirmation has no local producer head"))?;
            if head.0!=epochs::json(&value.producer)? || head.1!=value.kind || head.2!=value.schema
                || head.3.as_ref().is_some_and(|id|id!=signed.proof.id().as_str()) {
                return Err(sql::invalid("confirmation differs from the local producer binding"));
            }
            epochs::put(db,&value,&signed)?;
            if head.3.is_some() { return Ok(()); }
            let id = signed.proof.id();
            let digest = feed::stream_genesis(&id);
            db.prepare_cached("UPDATE sync_stream_heads SET epoch_proof=?5 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
                .execute(params![scope.hub.to_string(),scope.scope.as_str(),value.stream.as_str(),value.epoch.as_str(),id.as_str()])?;
            db.prepare_cached("INSERT INTO sync_stream_prefixes(hub,scope,stream,epoch,sequence,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6)")?
                .execute(params![scope.hub.to_string(),scope.scope.as_str(),value.stream.as_str(),value.epoch.as_str(),0_u64.to_be_bytes().as_slice(),digest.as_str()])?;
            let local = local_head::advance(db)?;
            dirty(db,scope,&value.stream,&value.epoch,local)
        }).await.map_err(|e|sql::error(e,op))
    }

    /// Performs one bounded prefix page. The common worker persists the outcome.
    pub fn build_prefix(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        Box::pin(async move {
            let (stream, epoch) = coordinates(&lease.task.key)?;
            let scope = lease.task.key.scope.clone();
            let read = (scope.clone(), stream.clone(), epoch.clone());
            let loaded = self
                .blocks
                .database()
                .read(move |db| {
                    let tx = db.unchecked_transaction()?;
                    let progress = progress(&tx, &read.0, &read.1, &read.2)?;
                    let Some((after, prefix, proof, head, kind)) = progress else { return Ok(None) };
                    let page = super::prefix_records::page(&tx, &read.0, &read.1, &read.2, after, &prefix)?;
                    Ok(Some((after, prefix, proof, head, kind, page)))
                })
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))?;
            let Some((after, previous, proof, _, kind, page)) = loaded else {
                return Ok(Outcome::Completed { through: lease.task.wanted });
            };
            if !self.authorizers.contains_key(kind.as_str()) {
                return Err(Error::Unavailable("prefix kind has no installed authorizer".into()));
            }
            let final_sequence = page.last().map_or(after, |(sequence, _)| *sequence);
            let final_digest = page.last().map_or(previous.clone(), |(_, digest)| digest.clone());
            let local = self.blocks.holder().clone();
            let done = self.blocks.database().write(fastctx_types::sync::PAGE_BYTES as usize, move |db| {
                let _boundary = boundary::enter_typed_write(Role::Source);
                let Some((found,digest,epoch_proof,head,_)) = progress(db,&scope,&stream,&epoch)? else { return Ok(true) };
                if found!=after || digest!=previous || epoch_proof!=proof { return Ok(false); }
                let producer: Vec<u8> = db.prepare_cached("SELECT producer FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
                    .query_row(params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),epoch.as_str()], |r|r.get(0))?;
                if !matches!(&local,crate::cursor::Holder::Device{device} if producer==epochs::json(&Producer::Device{device:device.clone()})?) {
                    return Err(sql::invalid("prefix task is not owned by this producer"));
                }
                if page.is_empty() && after<head { return Err(sql::invalid("local source records have a gap")); }
                for (sequence,digest) in &page {
                    let changed = db.prepare_cached("UPDATE sync_stream_records SET prefix_digest=?6 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence=?5 AND (prefix_digest IS NULL OR prefix_digest=?6)")?
                        .execute(params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),epoch.as_str(),sequence.0.to_be_bytes().as_slice(),digest.as_str()])?;
                    if changed!=1 { return Err(sql::invalid("stored record already has a different prefix")); }
                }
                db.prepare_cached("UPDATE sync_stream_prefixes SET sequence=?5,prefix_digest=?6 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
                    .execute(params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),epoch.as_str(),final_sequence.0.to_be_bytes().as_slice(),final_digest.as_str()])?;
                if final_sequence==head {
                    db.prepare_cached("UPDATE sync_stream_heads SET prefix_digest=?5 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
                        .execute(params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),epoch.as_str(),final_digest.as_str()])?;
                }
                Ok(final_sequence==head)
            }).await.map_err(|e|sql::error(e,self.blocks.worker().clone()))?;
            Ok(if done {
                Outcome::Completed { through: lease.task.wanted }
            } else {
                Outcome::Retry { next_due_millis: 0, reason: "more local source prefixes to build".into() }
            })
        })
    }
}
impl TaskHandler for SqliteSources {
    fn kind(&self) -> &'static str {
        "sync.projection"
    }
    fn takes(&self, key: &TaskKey) -> bool {
        key.kind == self.kind() && key.key.starts_with(PREFIX)
    }
    fn handle(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        self.build_prefix(lease)
    }
}

pub(super) fn dirty(
    db: &Connection,
    scope: &ScopeKey,
    stream: &StreamId,
    epoch: &ProducerEpoch,
    local: LocalSequence,
) -> SqlResult<()> {
    crate::scheduler::projection_work(db, scope, format!("{PREFIX}{}/{}", stream.as_str(), epoch.as_str()), local)
}
fn coordinates(key: &TaskKey) -> Result<(StreamId, ProducerEpoch), Error> {
    let rest = key.key.strip_prefix(PREFIX).ok_or_else(|| Error::Invalid("not a source prefix task".into()))?;
    let (stream, epoch) = rest.split_once('/').ok_or_else(|| Error::Invalid("invalid source prefix task".into()))?;
    if key.kind != "sync.projection" {
        return Err(Error::Invalid("not a projection task".into()));
    }
    Ok((
        StreamId::try_from(stream.to_owned()).map_err(Error::Invalid)?,
        ProducerEpoch::try_from(epoch.to_owned()).map_err(Error::Invalid)?,
    ))
}

type Progress = (Sequence, crate::identity::Digest, String, Sequence, String);
fn progress(
    db: &Connection,
    scope: &ScopeKey,
    stream: &StreamId,
    epoch: &ProducerEpoch,
) -> SqlResult<Option<Progress>> {
    db.prepare_cached("SELECT p.sequence,p.prefix_digest,h.epoch_proof,h.sequence,h.kind FROM sync_stream_prefixes p JOIN sync_stream_heads h USING(hub,scope,stream,epoch) JOIN sync_scopes s USING(hub,scope) WHERE p.hub=?1 AND p.scope=?2 AND p.stream=?3 AND p.epoch=?4 AND s.enabled=1")?
        .query_row(params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),epoch.as_str()],|r|Ok((sql::counter(r,0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,sql::counter(r,3)?,r.get::<_,String>(4)?)))
        .optional()?.map(|(sequence,prefix,proof,head,kind)|Ok((Sequence(sequence),prefix.try_into().map_err(sql::invalid)?,proof,Sequence(head),kind))).transpose()
}
