//! The upload unit is the exact durable proof, never a reconstructed source step.
use super::*;
use crate::{
    catalog::SourceBatch,
    cursor::Sequence,
    identity::Producer,
    proof::{
        Bytes, EpochDescriptor, HandoverBasis, ProofPurpose, Signature, SignedSourceBatch, SignedStatement, SigningKey,
    },
};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::sync::source_transfer::SourceTransfer;

pub(crate) struct UploadBatch {
    pub signed: SignedSourceBatch,
    pub batch: SourceBatch,
    pub transfer: SourceTransfer,
    pub account: crate::identity::AccountId,
    pub _permit: crate::admission::BytePermit,
}
impl SourceUploads {
    pub(crate) async fn batch(&self, state: &SourceUploadState) -> Result<Option<UploadBatch>, Error> {
        self.check(state).await?;
        let expected = state.clone();
        let length=self.sources.blocks.database().read(move|db| {
            let k=&expected.key;
            db.query_row("SELECT length(signed_bytes) FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND ((?5 IS NOT NULL AND proof=?5) OR (?5 IS NULL AND first_sequence>?6)) ORDER BY first_sequence LIMIT 1",
                params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),expected.proof.as_ref().map(crate::identity::Digest::as_str),expected.through.0.to_be_bytes().as_slice()],|r|r.get::<_,u32>(0)).optional().map_err(Into::into)
        }).await.map_err(|e|sql::error(e,self.sources.blocks.worker().clone()))?;
        let length = length.unwrap_or(0) as usize;
        if length > fastctx_types::sync::PAGE_BYTES as usize {
            return Err(Error::Corrupt("stored source proof exceeds its byte bound".into()));
        }
        let permit = self.budget().try_acquire(length * 8 + 65536)?;
        let store = self.clone();
        let expected = state.clone();
        let found=self.sources.blocks.database().read(move|db|{
            let tx=db.unchecked_transaction()?;
            let route=store.route(&tx,&expected.key)?.ok_or_else(||sql::invalid("source scope is not promoted"))?;
            let key=&expected.key;
            let found=tx.query_row("SELECT proof,purpose,signer,CASE WHEN length(signed_bytes)<=?7 THEN signed_bytes END,signature,first_sequence,last_sequence FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND ((?5 IS NOT NULL AND proof=?5) OR (?5 IS NULL AND first_sequence>?6)) ORDER BY first_sequence LIMIT 1",
                params![key.scope.hub.to_string(),key.scope.scope.as_str(),key.stream.as_str(),key.epoch.as_str(),expected.proof.as_ref().map(crate::identity::Digest::as_str),expected.through.0.to_be_bytes().as_slice(),fastctx_types::sync::PAGE_BYTES],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Vec<u8>>(2)?,r.get::<_,Option<Vec<u8>>>(3)?,r.get::<_,Vec<u8>>(4)?,sql::counter(r,5)?,sql::counter(r,6)?))).optional()?;
            tx.commit()?;Ok((route,found))
        }).await.map_err(|e|sql::error(e,self.sources.blocks.worker().clone()))?;
        let (route, found) = found;
        let Some((id, purpose, signer, data, signature, first, last)) = found else {
            if state.proof.is_some() || route.through > state.through {
                return Err(Error::Corrupt("source proof is missing before the local stream head".into()));
            }
            return Ok(None);
        };
        let proof = SignedStatement {
            purpose: ProofPurpose::from_domain(&purpose)
                .ok_or_else(|| Error::Corrupt("source proof has an unknown purpose".into()))?,
            signer: SigningKey(
                signer.try_into().map_err(|_| Error::Corrupt("invalid source signing key length".into()))?,
            ),
            data: Bytes(data.ok_or_else(|| Error::Corrupt("stored source proof exceeds its byte bound".into()))?),
            signature: Signature(
                signature.try_into().map_err(|_| Error::Corrupt("invalid source signature length".into()))?,
            ),
        };
        let claim: SourceBatch = serde_json::from_slice(&proof.data.0).map_err(|e| Error::Corrupt(e.to_string()))?;
        claim.validate_shape().map_err(Error::Corrupt)?;
        if proof.id().as_str() != id
            || proof.purpose != ProofPurpose::SourceBatchV1
            || claim.scope != state.key.scope
            || claim.stream != state.key.stream
            || claim.epoch != state.key.epoch
            || claim.producer != (Producer::Device { device: self.device.clone() })
            || claim.kind != route.kind.kind
            || claim.schema != route.kind.schema
            || claim.after != state.through
            || claim.after.0.checked_add(1) != Some(first)
            || claim.through.0 != last
        {
            return Err(Error::Corrupt("source proof identity or range differs from the durable upload cursor".into()));
        }
        let epoch = EpochDescriptor {
            scope: claim.scope.clone(),
            stream: claim.stream.clone(),
            kind: claim.kind.clone(),
            schema: claim.schema,
            epoch: claim.epoch.clone(),
            producer: claim.producer.clone(),
            predecessor: None,
            closed_through: None,
            operation: claim.operation.clone(),
            control_sequence: Sequence(0),
            basis: HandoverBasis::Initial { registration: claim.operation.clone() },
        };
        let signed = SignedSourceBatch { proof };
        if self
            .verifier
            .source(&signed, &epoch)
            .map_err(|e| Error::Corrupt(format!("stored source proof did not verify: {e}")))?
            != claim
        {
            return Err(Error::Corrupt("source verifier changed the original batch".into()));
        }
        Ok(Some(UploadBatch {
            transfer: SourceTransfer {
                holding: state.holding.clone(),
                object: route.object,
                kind: route.kind,
                epoch: claim.epoch.clone(),
                operation: claim.operation.clone(),
            },
            signed,
            batch: claim,
            account: route.account,
            _permit: permit,
        }))
    }
    pub(crate) async fn start(&self, state: &SourceUploadState, batch: &UploadBatch) -> Result<(), Error> {
        let mut next = state.clone();
        next.proof = Some(batch.signed.proof.id());

        next.phase = SourceUploadPhase::Content;
        let roots: Vec<_> = batch
            .batch
            .records
            .iter()
            .flat_map(|r| {
                std::iter::once(crate::manifest::ContentRef::from(r.payload.clone())).chain(r.roots.iter().cloned())
            })
            .collect();
        self.change(state, fastctx_types::sync::PAGE_BYTES as usize * 4, move |db| {
            if rows::batch(db, &next, next.proof.as_ref().expect("new batch proof"))?.is_some() {
                return Ok(());
            }
            rows::save(db, &next)?;
            for root in roots {
                super::nodes::insert(db, &next, &root)?;
            }
            Ok(())
        })
        .await
    }
}
