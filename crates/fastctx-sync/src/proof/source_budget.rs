//! Source proofs repeat their records in stream pages; both copies need admission.
use crate::{
    catalog::SourceBatch,
    cursor::{Holder, HoldingPeriod, Revision, Sequence, StreamCursor},
    identity::{DeviceId, Digest, OperationId, Producer, ProducerEpoch, ScopeKey, StreamId},
    proof::{Bytes, ProofPurpose, Signature, SignedSourceBatch, SignedStatement, SigningKey},
    store::{Error, ingest::StreamProgress},
    timeline::{Direction, ProvenRecord, StreamPage},
    transfer::Message,
};
use fastctx_types::sync::{PAGE_BYTES, transport_codec::bounded_json};

pub(crate) const SOURCE_RECORDS: usize = 256;

/// Encodes a source batch only if its original proof and every record fit one
/// complete stream response. The future reader's fence, tenure and continuation
/// use their longest encodings; key, signature and digest values have fixed widths.
/// Call before signing, and split a refused multi-record batch first.
pub fn encode_source_batch(batch: &SourceBatch) -> Result<Vec<u8>, Error> {
    if batch.records.is_empty() || batch.records.len() > SOURCE_RECORDS {
        return Err(Error::Invalid("source batch exceeds its record count".into()));
    }
    let exceeds = || {
        Error::Invalid(if batch.records.len() == 1 {
            "a single source record exceeds the stream page budget".into()
        } else {
            "source batch exceeds the stream page budget; split it before signing".into()
        })
    };
    let data = bounded_json(batch, PAGE_BYTES as usize).map_err(|_| exceeds())?;
    let digest = Digest::from_bytes([0; 32]);
    let page = StreamPage {
        fence: StreamProgress::Confirmed {
            cursor: StreamCursor {
                scope: batch.scope.clone(),
                stream: batch.stream.clone(),
                epoch: batch.epoch.clone(),
                committed: Sequence(u64::MAX),
            },
            epoch_proof: digest.clone(),
            prefix_digest: digest.clone(),
        },
        direction: Direction::Backward,
        records: batch
            .records
            .iter()
            .map(|record| ProvenRecord { record: record.clone(), batch_proof: digest.clone() })
            .collect(),
        proofs: vec![SignedSourceBatch {
            proof: SignedStatement {
                purpose: match batch.producer {
                    Producer::Hub => ProofPurpose::HubSourceBatchV1,
                    Producer::Device { .. } => ProofPurpose::SourceBatchV1,
                },
                signer: SigningKey([0; 32]),
                data: Bytes(data),
                signature: Signature([0; 64]),
            },
        }],
        next: Some(Sequence(u64::MAX)),
    };
    let response = Message::StreamPage {
        holding: HoldingPeriod {
            holder: Holder::Device { device: DeviceId::from_bytes([0; 32]) },
            scope: batch.scope.clone(),
            acl_revision: Revision(u64::MAX),
            publication_epoch: Revision(u64::MAX),
        },
        page,
    };
    bounded_json(&response, PAGE_BYTES as usize).map_err(|_| exceeds())?;
    let Message::StreamPage { mut page, .. } = response else { unreachable!() };
    Ok(page.proofs.remove(0).proof.data.0.into_boxed_slice().into_vec())
}

/// Subsequent parts have stable identities distinct from the step and from each
/// other. The first part keeps the step identity, including epoch registration.
pub(crate) fn part_operation(
    step: &OperationId,
    scope: &ScopeKey,
    stream: &StreamId,
    epoch: &ProducerEpoch,
    first: Sequence,
) -> OperationId {
    let seed = format!(
        "fastctx-sync/source-batch-part/v1\0{}\0{}\0{}\0{}\0{}\0{}",
        step.as_str(),
        scope.hub,
        scope.scope.as_str(),
        stream.as_str(),
        epoch.as_str(),
        first.0
    );
    OperationId::from_bytes(Digest::of(seed.as_bytes()).bytes())
}

pub(crate) fn split_source_batch(batch: &SourceBatch) -> Result<Vec<SourceBatch>, Error> {
    batch.validate_shape().map_err(Error::Invalid)?;
    if batch.records.len() > SOURCE_RECORDS {
        return Err(Error::Invalid("source batch exceeds its record count".into()));
    }
    fn split(batch: SourceBatch, step: &OperationId, output: &mut Vec<SourceBatch>) -> Result<(), Error> {
        match encode_source_batch(&batch) {
            Ok(_) => output.push(batch),
            Err(error) if batch.records.len() == 1 => return Err(error),
            Err(_) => {
                let middle = batch.records.len() / 2;
                let mut left = batch.clone();
                let mut right = batch;
                left.records.truncate(middle);
                right.records.drain(..middle);
                left.through = left.records.last().expect("nonempty left half").sequence;
                right.after = left.through;
                right.operation =
                    part_operation(step, &right.scope, &right.stream, &right.epoch, Sequence(right.after.0 + 1));
                split(left, step, output)?;
                split(right, step, output)?;
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    split(batch.clone(), &batch.operation, &mut output)?;
    Ok(output)
}
