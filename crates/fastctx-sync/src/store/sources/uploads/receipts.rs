//! A checked remote receipt is durable before local epoch confirmation resumes.
use super::*;
use crate::{
    catalog::{Authority, HeadEvidence},
    cursor::Sequence,
    proof::{AcceptedActor, EpochDescriptor},
    store::{
        access::{AccessContext, Principal},
        authority::SourceReceipt,
    },
};
use fastctx_storage::rusqlite::params;
use fastctx_types::sync::feed;

impl SourceUploads {
    fn verify_receipt(
        &self,
        state: &SourceUploadState,
        batch: &UploadBatch,
        receipt: &SourceReceipt,
    ) -> Result<EpochDescriptor, Error> {
        let epoch = self.verifier.epoch(&receipt.epoch, &self.identity)?;
        let epoch_id = receipt.epoch.proof.id();
        let mut prefix = state.prefix_digest.clone().unwrap_or_else(|| feed::stream_genesis(&epoch_id));
        for record in &batch.batch.records {
            prefix = feed::stream_advance(&prefix, record).map_err(Error::Corrupt)?.1;
        }
        // Later receipts may arrive first. Their prefix is checked again against
        // the contiguous durable anchor before the acknowledgement cursor moves.
        if state.through.0 > 0 && state.prefix_digest.is_none() {
            prefix = receipt.prefix_digest.clone();
        }
        let expected_actor = AcceptedActor::Account { account: batch.account.clone(), device: self.device.clone() };
        let expected_authority = Authority::Device { device: self.device.clone(), epoch: batch.batch.epoch.clone() };
        if epoch.scope != batch.batch.scope
            || epoch.stream != batch.batch.stream
            || epoch.epoch != batch.batch.epoch
            || epoch.producer != batch.batch.producer
            || epoch.kind != batch.batch.kind
            || epoch.schema != batch.batch.schema
            || receipt.operation != batch.batch.operation
            || receipt.proof != batch.signed.proof.id()
            || receipt.actor != expected_actor
            || receipt.stream.scope != batch.batch.scope
            || receipt.stream.stream != batch.batch.stream
            || receipt.stream.epoch != batch.batch.epoch
            || receipt.stream.committed != batch.batch.through
            || receipt.prefix_digest != prefix
            || receipt.head.key != batch.transfer.object
            || receipt.head.kind != batch.batch.kind
            || receipt.head.schema != batch.batch.schema
            || receipt.head.acl_revision != state.holding.acl_revision
            || receipt.head.authority != expected_authority
            || receipt.head.evidence
                != (HeadEvidence::Stream { epoch: epoch_id, through: batch.batch.through, prefix_digest: prefix })
            || receipt.scope.scope != batch.batch.scope
            || receipt.scope.acl_revision != state.holding.acl_revision
        {
            return Err(Error::Invalid(
                "hub source receipt differs from the original batch, owner or stream prefix".into(),
            ));
        }
        Ok(epoch)
    }
    pub(crate) async fn acknowledge(
        &self,
        state: &SourceUploadState,
        batch: &UploadBatch,
        receipt: SourceReceipt,
        complete: bool,
    ) -> Result<(), Error> {
        self.verify_receipt(state, batch, &receipt)?;
        let mut next = state.clone();
        next.receipt = Some(receipt);
        if complete {
            next.phase = SourceUploadPhase::Confirm;
        } else {
            next.phase = SourceUploadPhase::Waiting;
        }
        self.change(state, fastctx_types::sync::PAGE_BYTES as usize * 2, move |db| rows::save(db, &next)).await
    }
    pub(crate) async fn waiting(
        &self,
        state: &SourceUploadState,
        proof: &crate::identity::Digest,
    ) -> Result<(), Error> {
        if state.proof.as_ref() != Some(proof) {
            return Err(Error::Invalid("hub awaited another source proof".into()));
        }
        let mut next = state.clone();
        next.phase = SourceUploadPhase::Waiting;
        self.change(state, 8192, move |db| rows::save(db, &next)).await
    }
    pub(crate) async fn finish(&self, state: &SourceUploadState, batch: &UploadBatch) -> Result<Sequence, Error> {
        self.check(state).await?;
        if state.through.0 > 0 && state.prefix_digest.is_none() {
            return Err(Error::Corrupt("contiguous source acknowledgement has no prefix anchor".into()));
        }
        let receipt =
            state.receipt.clone().ok_or_else(|| Error::Corrupt("confirmed source upload has no receipt".into()))?;
        self.verify_receipt(state, batch, &receipt)?;
        let key = state.key.clone();
        let proof = receipt.epoch.proof.id();
        let installed = self
            .sources
            .blocks
            .database()
            .read(move |db| {
                db.query_row(
                    "SELECT epoch_proof FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",
                    params![
                        key.scope.hub.to_string(),
                        key.scope.scope.as_str(),
                        key.stream.as_str(),
                        key.epoch.as_str()
                    ],
                    |r| r.get::<_, Option<String>>(0),
                )
                .map_err(Into::into)
            })
            .await
            .map_err(|e| sql::error(e, self.sources.blocks.worker().clone()))?;
        if installed.as_deref() != Some(proof.as_str()) {
            self.sources
                .confirm(
                    AccessContext {
                        principal: Principal::LocalService,
                        scope: state.key.scope.clone(),
                        observed_acl: state.holding.acl_revision,
                    },
                    receipt.epoch.clone(),
                )
                .await?;
        }
        let mut next = state.clone();
        next.through = receipt.stream.committed;
        next.prefix_digest = Some(receipt.prefix_digest);
        next.proof = None;

        next.phase = SourceUploadPhase::Idle;
        let through = next.through;
        let expected = state.clone();
        self.change(state, 16384, move |db| {
            let current = rows::load(db, &expected.key)?.ok_or_else(|| sql::invalid("upload cursor disappeared"))?;
            if current != expected {
                return Err(fastctx_storage::Error::with_kind(
                    fastctx_storage::ErrorKind::Busy,
                    "an earlier source receipt must commit first",
                ));
            }
            rows::save(db, &next)
        })
        .await?;
        Ok(through)
    }
}
