//! Remote source evidence is retained verbatim. Waiting for an epoch never
//! advances a verified cursor, and replay never touches producer checkpoints.
use super::{
    SqliteReplica,
    rows::{decode, encode},
};
use crate::{
    catalog::{SourceBatch, SourceRecord},
    cursor::{LocalSequence, StreamCursor},
    identity::{Digest, StreamId},
    proof::{ConfirmedEpoch, SignedSourceBatch},
    store::{
        Error, FutureResult,
        boundary::{self, Role},
        replica::ReplicaContext,
        sql,
    },
    timeline::{Direction, StreamPage},
};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::sync::{feed, progress::StreamProgress};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub enum StreamInstalled {
    AwaitingEpoch { proof: Digest },
    Committed { local: LocalSequence, progress: StreamProgress },
}
impl SqliteReplica {
    /// An epoch must be requested by an already installed stream head. Confirmation
    /// can arrive after its first range page, which remains a bounded durable inbox.
    pub fn install_epoch(&self, context: ReplicaContext, proof: ConfirmedEpoch) -> FutureResult<'_, ()> {
        Box::pin(async move {
            self.connection(&context)?;
            bounded(&proof)?;
            let descriptor = self.verifier.epoch(&proof, &self.hub)?;
            if descriptor.scope != context.holding.scope {
                return Err(Error::Invalid("epoch belongs to another receiving scope".into()));
            }
            let this = self.clone();
            self.blocks
                .database()
                .write(
                    encode(&proof).map_err(|e| sql::error(e, self.blocks.worker().clone()))?.len() + 16384,
                    move |db| {
                        let _role = boundary::enter_typed_write(Role::Replica);
                        let target = super::epoch_chain::target_for_epoch(
                            db,
                            &this.blocks,
                            &context,
                            &descriptor,
                            &proof.proof.id(),
                        )?;
                        if target.kind != descriptor.kind || target.schema != descriptor.schema {
                            return Err(sql::invalid("epoch differs from the advertised stream kind"));
                        }
                        super::stream_rows::install_epoch(db, &descriptor, &proof)?;
                        let (through, prefix, local) =
                            super::stream_rows::head(db, &target, &descriptor, &proof.proof.id())?;
                        super::epoch_chain::cutoff(db, &descriptor, through)?;
                        if through.0 == 0 {
                            if target.wanted.0 == 0 && target.prefix.as_ref().is_some_and(|wanted| wanted != &prefix) {
                                return Err(sql::invalid("empty stream target differs from its epoch genesis"));
                            }
                            super::stream_rows::advance(
                                db,
                                &target,
                                &descriptor,
                                &proof.proof.id(),
                                through,
                                &prefix,
                                local,
                            )?;
                        }
                        Ok(())
                    },
                )
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    /// Returns the next contiguous confirmed position, or a durable AwaitingEpoch.
    pub fn apply_stream(&self, context: ReplicaContext, page: StreamPage) -> FutureResult<'_, StreamInstalled> {
        Box::pin(self.stream(context, page))
    }
    /// Replays the one durable page waiting on this epoch. The page and its inbox
    /// entry commit together; a crash before or after the commit is harmless.
    pub fn resume_stream(
        &self,
        context: ReplicaContext,
        stream: StreamId,
        epoch_proof: Digest,
    ) -> FutureResult<'_, Option<StreamInstalled>> {
        Box::pin(async move {
            self.connection(&context)?;
            let request = context.clone();
            let blocks = self.blocks.clone();
            let page=self.blocks.database().read(move |db| {
                super::stream_rows::target(db,&blocks,&request,&stream,&epoch_proof)?;
                db.query_row("SELECT page FROM sync_replica_waiting WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch_proof=?4 AND holding=?5",params![request.holding.scope.hub.to_string(),request.holding.scope.scope.as_str(),stream.as_str(),epoch_proof.as_str(),encode(&request.holding)?],|r|r.get::<_,Vec<u8>>(0)).optional()?.map(|b|decode(&b)).transpose()
            }).await.map_err(|e|sql::error(e,self.blocks.worker().clone()))?;
            match page {
                Some(page) => self.stream(context, page).await.map(Some),
                None => Ok(None),
            }
        })
    }
    async fn stream(&self, context: ReplicaContext, page: StreamPage) -> Result<StreamInstalled, Error> {
        self.connection(&context)?;
        let bytes = bounded(&page)?;
        let StreamProgress::Confirmed { cursor, epoch_proof, prefix_digest } = page.fence.clone() else {
            return Err(Error::Invalid("a replica accepts only confirmed source positions".into()));
        };
        if page.direction != Direction::Forward
            || page.records.is_empty()
            || page.records.len() > 256
            || cursor.scope != context.holding.scope
            || page.records.windows(2).any(|r| r[0].record.sequence.0.checked_add(1) != Some(r[1].record.sequence.0))
            || page.records.last().is_some_and(|r| r.record.sequence > cursor.committed)
            || page.next.is_some_and(|n| Some(n) != page.records.last().map(|r| r.record.sequence))
        {
            return Err(Error::Invalid(
                "replica stream pages must be consecutive forward ranges within their fence".into(),
            ));
        }
        let request = context.clone();
        let stream = cursor.stream.clone();
        let id = epoch_proof.clone();
        let blocks = self.blocks.clone();
        let descriptor = self
            .blocks
            .database()
            .read(move |db| {
                super::stream_rows::target(db, &blocks, &request, &stream, &id)?;
                super::stream_rows::epoch(db, &request.holding.scope, &stream, &id)
            })
            .await
            .map_err(|e| sql::error(e, self.blocks.worker().clone()))?;
        let Some(descriptor) = descriptor else { return self.waiting(context, page, bytes).await };
        if descriptor.epoch != cursor.epoch {
            return Err(Error::Invalid("stream fence differs from its epoch description".into()));
        }
        let mut proofs = BTreeMap::<Digest, (SignedSourceBatch, SourceBatch)>::new();
        for signed in &page.proofs {
            let batch = self.verifier.source(signed, &descriptor)?;
            batch.validate_shape().map_err(Error::Invalid)?;
            if batch.scope != descriptor.scope
                || batch.stream != descriptor.stream
                || batch.epoch != descriptor.epoch
                || batch.producer != descriptor.producer
                || batch.kind != descriptor.kind
                || batch.schema != descriptor.schema
            {
                return Err(Error::Invalid("source batch differs from its confirmed producer binding".into()));
            }
            if proofs.insert(signed.proof.id(), (signed.clone(), batch)).is_some() {
                return Err(Error::Invalid("stream page repeats a batch proof".into()));
            }
        }
        let mut used = std::collections::BTreeSet::new();
        let mut references = 0;
        for record in &page.records {
            let (_, batch) = proofs
                .get(&record.batch_proof)
                .ok_or_else(|| Error::Invalid("stream record has no original proof on this page".into()))?;
            let signed: Option<&SourceRecord> = record
                .record
                .sequence
                .0
                .checked_sub(batch.after.0 + 1)
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| batch.records.get(i));
            if signed != Some(&record.record) {
                return Err(Error::Invalid("stream record differs from the exact record in its original proof".into()));
            }
            used.insert(record.batch_proof.clone());
            references += 1 + record.record.roots.len();
        }
        if proofs.len() != used.len() || references > 4096 {
            return Err(Error::Invalid("source page has unused proofs or too many references".into()));
        }
        let this = self.clone();
        self.blocks.database().write(bytes.len()*4+16384,move |db| {
            let _role=boundary::enter_typed_write(Role::Replica);
            let target=super::stream_rows::target(db,&this.blocks,&context,&cursor.stream,&epoch_proof)?;
            if cursor.committed>target.wanted || cursor.committed==target.wanted && target.prefix.as_ref().is_some_and(|p|p!=&prefix_digest) {
                return Err(sql::invalid("stream page exceeds or contradicts its authenticated catalog target"));
            }
            let persisted=super::stream_rows::epoch(db,&cursor.scope,&cursor.stream,&epoch_proof)?.ok_or_else(||sql::invalid("epoch disappeared before range commit"))?;
            if persisted!=descriptor {return Err(sql::invalid("epoch changed during source verification"));}
            let (mut through,mut prefix,mut local)=super::stream_rows::head(db,&target,&descriptor,&epoch_proof)?;
            super::epoch_chain::cutoff(db,&descriptor,page.records.last().unwrap().record.sequence)?;
            let mut appended=false;
            for proven in &page.records {
                let record=&proven.record;
                if record.sequence<=through {
                    super::stream_rows::replay(db,&descriptor,record,&proven.batch_proof)?;
                    continue;
                }
                if through.0.checked_add(1)!=Some(record.sequence.0) {return Err(sql::invalid("source page leaves a gap in the durable stream"));}
                if !appended {local=crate::store::local_head::advance(db)?;appended=true;}
                let (_,next)=feed::stream_advance(&prefix,record).map_err(sql::invalid)?;
                let (signed,batch)=&proofs[&proven.batch_proof];
                super::stream_rows::append(db,&target,&descriptor,signed,batch,record,&next)?;
                through=record.sequence;prefix=next;
                if through==cursor.committed && prefix!=prefix_digest {return Err(sql::invalid("stream prefix differs at the page fence"));}
                if through==target.wanted && target.prefix.as_ref().is_some_and(|p|p!=&prefix) {return Err(sql::invalid("stream prefix differs at the catalog boundary"));}
            }
            if cursor.committed<=through {
                let stored:String=db.query_row("SELECT prefix_digest FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence=?5",params![cursor.scope.hub.to_string(),cursor.scope.scope.as_str(),cursor.stream.as_str(),cursor.epoch.as_str(),cursor.committed.0.to_be_bytes().as_slice()],|r|r.get(0))?;
                if stored!=prefix_digest.as_str() {return Err(sql::invalid("replayed fence differs from its durable prefix"));}
            }
            if appended {super::stream_rows::advance(db,&target,&descriptor,&epoch_proof,through,&prefix,local)?;}
            db.execute("DELETE FROM sync_replica_waiting WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch_proof=?4 AND digest=?5 AND holding=?6",params![cursor.scope.hub.to_string(),cursor.scope.scope.as_str(),cursor.stream.as_str(),epoch_proof.as_str(),Digest::of(&bytes).as_str(),encode(&context.holding)?])?;
            Ok(StreamInstalled::Committed{local,progress:StreamProgress::Confirmed{cursor:StreamCursor{committed:through,..cursor},epoch_proof,prefix_digest:prefix}})
        }).await.map_err(|e|sql::error(e,self.blocks.worker().clone()))
    }
    async fn waiting(
        &self,
        context: ReplicaContext,
        page: StreamPage,
        bytes: Vec<u8>,
    ) -> Result<StreamInstalled, Error> {
        let StreamProgress::Confirmed { cursor, epoch_proof, .. } = page.fence else { unreachable!() };
        let digest = Digest::of(&bytes);
        let blocks = self.blocks.clone();
        self.blocks.database().write(bytes.len()+4096,move |db| {
            let _role=boundary::enter_typed_write(Role::Replica);
            super::stream_rows::target(db,&blocks,&context,&cursor.stream,&epoch_proof)?;
            let existing=db.query_row("SELECT digest,holding FROM sync_replica_waiting WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch_proof=?4",params![cursor.scope.hub.to_string(),cursor.scope.scope.as_str(),cursor.stream.as_str(),epoch_proof.as_str()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?))).optional()?;
            if let Some((known,holding))=existing
                && decode::<crate::cursor::HoldingPeriod>(&holding)?==context.holding {
                if known==digest.as_str() {return Ok(StreamInstalled::AwaitingEpoch{proof:epoch_proof});}
                return Err(fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Busy,"this epoch already has a waiting page"));
            }
            let (count,total):(i64,i64)=db.query_row("SELECT COUNT(*),COALESCE(SUM(length(page)),0) FROM sync_replica_waiting",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if count>=64 || total+bytes.len() as i64>4*1024*1024 {return Err(fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Busy,"awaiting-epoch inbox is full"));}
            db.execute("INSERT INTO sync_replica_waiting(hub,scope,stream,epoch_proof,holding,page,digest) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(hub,scope,stream,epoch_proof) DO UPDATE SET holding=excluded.holding,page=excluded.page,digest=excluded.digest",params![cursor.scope.hub.to_string(),cursor.scope.scope.as_str(),cursor.stream.as_str(),epoch_proof.as_str(),encode(&context.holding)?,bytes,digest.as_str()])?;
            Ok(StreamInstalled::AwaitingEpoch{proof:epoch_proof})
        }).await.map_err(|e|sql::error(e,self.blocks.worker().clone()))
    }
}
fn bounded(value: &impl serde::Serialize) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(value).map_err(|e| Error::Invalid(e.to_string()))?;
    if bytes.len() > fastctx_types::sync::PAGE_BYTES as usize {
        Err(Error::Invalid("source metadata exceeds one page".into()))
    } else {
        Ok(bytes)
    }
}
