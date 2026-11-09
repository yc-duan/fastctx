//! The transport runner reads durable requirements, never infers them from a task key.
use super::{SqliteReplica, stream_rows};
use crate::{
    cursor::Sequence,
    identity::{Digest, ObjectKey, StreamId},
    proof::{Bytes, ConfirmedEpoch, ProofPurpose, Signature, SignedStatement, SigningKey},
    store::{FutureResult, replica::ReplicaContext, sql},
};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::sync::progress::StreamProgress;

pub struct StreamTarget {
    pub object: ObjectKey,
    pub kind: String,
    pub schema: u16,
    pub epoch_proof: Digest,
    pub through: Sequence,
    /// Ancestors discovered from a signed handover have a closed sequence but
    /// no catalog anchor; their own signed records establish the prefix.
    pub prefix_digest: Option<Digest>,
    pub received: Option<StreamProgress>,
}
impl SqliteReplica {
    pub fn stream_target(
        &self,
        context: ReplicaContext,
        stream: StreamId,
        proof: Digest,
    ) -> FutureResult<'_, StreamTarget> {
        Box::pin(async move {
            self.connection(&context)?;
            let blocks = self.blocks.clone();
            self.blocks
                .database()
                .read(move |db| {
                    let target = stream_rows::target(db, &blocks, &context, &stream, &proof)?;
                    let descriptor = stream_rows::epoch(db, &context.holding.scope, &stream, &proof)?;
                    let received = descriptor
                        .map(|descriptor| {
                            let (through, prefix, _) = stream_rows::head(db, &target, &descriptor, &proof)?;
                            Ok::<_, fastctx_storage::Error>(StreamProgress::Confirmed {
                                cursor: crate::cursor::StreamCursor {
                                    scope: descriptor.scope,
                                    stream: descriptor.stream,
                                    epoch: descriptor.epoch,
                                    committed: through,
                                },
                                epoch_proof: proof.clone(),
                                prefix_digest: prefix,
                            })
                        })
                        .transpose()?;
                    Ok(StreamTarget {
                        object: target.custody.object,
                        kind: target.kind,
                        schema: target.schema,
                        epoch_proof: proof,
                        through: target.wanted,
                        prefix_digest: target.prefix,
                        received,
                    })
                })
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    /// Returns the original statement, including its exact signed bytes. It is
    /// suitable for verification and forwarding, not a reconstructed signature.
    pub fn epoch(
        &self,
        context: ReplicaContext,
        stream: StreamId,
        proof: Digest,
    ) -> FutureResult<'_, Option<ConfirmedEpoch>> {
        Box::pin(async move {
            self.connection(&context)?;
            let blocks = self.blocks.clone();
            self.blocks.database().read(move |db| {
                stream_rows::target(db,&blocks,&context,&stream,&proof)?;
                let scope=&context.holding.scope;
                let row=db.query_row("SELECT signed_bytes,signer,signature FROM sync_epoch_descriptors WHERE hub=?1 AND scope=?2 AND stream=?3 AND proof=?4",params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),proof.as_str()],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,Vec<u8>>(2)?))).optional()?;
                row.map(|(data,key,signature)| {
                    let statement=SignedStatement{purpose:ProofPurpose::EpochHandoverV1,data:Bytes(data),signer:SigningKey(key.try_into().map_err(|_|sql::invalid("invalid stored hub key"))?),signature:Signature(signature.try_into().map_err(|_|sql::invalid("invalid stored epoch signature"))?)};
                    if statement.id()!=proof {return Err(sql::invalid("stored epoch differs from its proof identity"));}
                    Ok(ConfirmedEpoch{proof:statement})
                }).transpose()
            }).await.map_err(|e|sql::error(e,self.blocks.worker().clone()))
        })
    }
}
