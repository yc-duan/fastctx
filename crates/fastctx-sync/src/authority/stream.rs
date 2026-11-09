//! Keyset stream reads carry original batches once and retain their signed bytes.
use super::{
    epoch,
    read::{Result, corrupt, invalid, size, unavailable},
};
use crate::{
    catalog::SourceBatch,
    cursor::{HoldingPeriod, Sequence, StreamCursor},
    identity::Digest,
    proof::{EpochDescriptor, EpochLookup, ProofPurpose, SignedSourceBatch},
    store::{ingest::StreamProgress, sql},
    timeline::{Direction, ProvenRecord, StreamPage, StreamPosition, StreamRead},
    transfer::Message,
};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use fastctx_types::sync::feed;
use std::collections::BTreeMap;

pub(super) fn page(db: &Connection, holding: HoldingPeriod, request: StreamRead, max: usize) -> Result<Message> {
    let cursor = &request.stream;
    let scope = &cursor.scope;
    let row = db.prepare_cached("SELECT sequence,epoch_proof,prefix_digest,producer,kind,schema FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), cursor.stream.as_str(), cursor.epoch.as_str()], |row| {
            Ok((sql::counter(row, 0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Vec<u8>>(3)?, row.get::<_, String>(4)?, row.get::<_, u16>(5)?))
        }).optional()?.ok_or_else(|| unavailable("the requested stream epoch is absent"))?;
    let (head, proof, head_prefix, producer, kind, schema) = row;
    let proof = epoch::digest(proof.ok_or_else(|| unavailable("the stream epoch is not confirmed"))?)?;
    let head_prefix =
        epoch::digest(head_prefix.ok_or_else(|| unavailable("the confirmed stream prefix is not ready"))?)?;
    let confirmed = epoch::get(db, scope, &EpochLookup::Proof { id: proof.clone() })?;
    let descriptor: EpochDescriptor = serde_json::from_slice(&confirmed.proof.data.0)?;
    if descriptor.stream != cursor.stream
        || descriptor.epoch != cursor.epoch
        || descriptor.producer != serde_json::from_slice(&producer)?
        || descriptor.kind != kind
        || descriptor.schema != schema
    {
        return Err(corrupt("stream head differs from its confirmed epoch"));
    }
    let through = request.through.unwrap_or(Sequence(head));
    if through.0 > head {
        return Err(unavailable("the requested stream fence is not present"));
    }
    let prefix = prefix_at(db, cursor, through, &proof)?;
    if through.0 == head && prefix != head_prefix {
        return Err(corrupt("stream head differs from its last record"));
    }
    let fence = StreamProgress::Confirmed {
        cursor: StreamCursor { committed: through, ..cursor.clone() },
        epoch_proof: proof.clone(),
        prefix_digest: prefix,
    };
    let (direction, lower, upper) = match request.position {
        StreamPosition::After(after) => (Direction::Forward, after.0, through.0),
        StreamPosition::Before(before) => (Direction::Backward, 0, through.0.min(before.0.saturating_sub(1))),
        StreamPosition::Latest => (Direction::Backward, 0, through.0),
    };
    let mut page =
        StreamPage { fence, direction, records: Vec::new(), proofs: Vec::new(), next: Some(Sequence(u64::MAX)) };
    let mut used = size(&Message::StreamPage { holding: holding.clone(), page: page.clone() })?;
    if used > max {
        return Err(invalid("the byte budget cannot hold a stream page envelope"));
    }
    let mut proofs = BTreeMap::<Digest, SourceBatch>::new();
    let order = if direction == Direction::Forward { "ASC" } else { "DESC" };
    let query = format!(
        "SELECT sequence,operation,kind,schema,CASE WHEN length(payload)<=?8 THEN payload END,CASE WHEN length(causes)<=?8 THEN causes END,batch_proof,prefix_digest FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence>?5 AND sequence<=?6 ORDER BY sequence {order} LIMIT ?7"
    );
    let mut statement = db.prepare_cached(&query)?;
    let mut rows = statement.query(params![
        scope.hub.to_string(),
        scope.scope.as_str(),
        cursor.stream.as_str(),
        cursor.epoch.as_str(),
        lower.to_be_bytes().as_slice(),
        upper.to_be_bytes().as_slice(),
        request.max_records + 1,
        fastctx_types::sync::PAGE_BYTES
    ])?;
    let mut previous_sequence = None;
    let mut more = false;
    while let Some(row) = rows.next()? {
        if page.records.len() >= request.max_records as usize {
            more = true;
            break;
        }
        let sequence = Sequence(sql::counter(row, 0)?);
        let expected = match direction {
            Direction::Forward => previous_sequence.unwrap_or(lower).checked_add(1),
            Direction::Backward => previous_sequence.map(|value: u64| value.saturating_sub(1)).or(Some(upper)),
        };
        if expected != Some(sequence.0) {
            return Err(corrupt("stream range contains a sequence gap"));
        }
        let id = epoch::digest(row.get(6)?)?;
        let loaded = if proofs.contains_key(&id) { None } else { Some(source(db, cursor, &id, &descriptor)?) };
        let batch = match &loaded {
            Some((_, batch)) => batch,
            None => &proofs[&id],
        };
        let record = sequence
            .0
            .checked_sub(batch.after.0)
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| batch.records.get(n as usize))
            .ok_or_else(|| corrupt("record is outside its original proof"))?;
        let operation: String = row.get(1)?;
        let kind: String = row.get(2)?;
        let schema: u16 = row.get(3)?;
        let payload: Vec<u8> =
            row.get::<_, Option<Vec<u8>>>(4)?.ok_or_else(|| corrupt("stored payload metadata exceeds a page"))?;
        let causes: Vec<u8> =
            row.get::<_, Option<Vec<u8>>>(5)?.ok_or_else(|| corrupt("stored causes exceed a page"))?;
        if record.sequence != sequence
            || record.operation.as_str() != operation
            || record.kind != kind
            || record.schema != schema
            || record.payload != serde_json::from_slice(&payload)?
            || record.causes != serde_json::from_slice::<Vec<_>>(&causes)?
        {
            return Err(corrupt("indexed record differs from its original proof"));
        }
        let before = prefix_at(db, cursor, Sequence(sequence.0 - 1), &proof)?;
        let digest = epoch::digest(row.get(7)?)?;
        if feed::stream_advance(&before, record).map_err(corrupt)?.1 != digest {
            return Err(corrupt("stored stream prefix differs from its record"));
        }
        let record = ProvenRecord { record: record.clone(), batch_proof: id.clone() };
        let proof_bytes = loaded.as_ref().map(|(signed, _)| size(signed)).transpose()?.unwrap_or(0);
        let needed = used
            + size(&record)?
            + usize::from(!page.records.is_empty())
            + proof_bytes
            + usize::from(loaded.is_some() && !page.proofs.is_empty());
        if needed > max {
            if page.records.is_empty() {
                return Err(invalid("the byte budget cannot hold one record and its original batch proof"));
            }
            more = true;
            break;
        }
        used = needed;
        if let Some((signed, batch)) = loaded {
            page.proofs.push(signed);
            proofs.insert(id, batch);
        }
        page.records.push(record);
        previous_sequence = Some(sequence.0);
    }
    if !more && lower < upper {
        let expected = if direction == Direction::Forward { upper } else { lower + 1 };
        if previous_sequence != Some(expected) {
            return Err(corrupt("stream range ends before its promised fence"));
        }
    }
    page.next = if more { previous_sequence.map(Sequence) } else { None };
    if direction == Direction::Backward {
        page.records.reverse();
    }
    Ok(Message::StreamPage { holding, page })
}

fn prefix_at(db: &Connection, cursor: &StreamCursor, at: Sequence, epoch: &Digest) -> Result<Digest> {
    if at == Sequence(0) {
        return Ok(feed::stream_genesis(epoch));
    }
    let value: Option<String> = db.prepare_cached("SELECT prefix_digest FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence=?5")?
        .query_row(params![cursor.scope.hub.to_string(), cursor.scope.scope.as_str(), cursor.stream.as_str(), cursor.epoch.as_str(), at.0.to_be_bytes().as_slice()], |row| row.get(0))
        .optional()?.flatten();
    epoch::digest(value.ok_or_else(|| corrupt("the confirmed stream has no prefix at the requested sequence"))?)
}
fn source(
    db: &Connection,
    cursor: &StreamCursor,
    id: &Digest,
    epoch: &EpochDescriptor,
) -> Result<(SignedSourceBatch, SourceBatch)> {
    let (purpose, data, signer, signature, state) = db.prepare_cached("SELECT purpose,CASE WHEN length(signed_bytes)<=?4 THEN signed_bytes END,signer,signature,state FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND proof=?3")?
        .query_row(params![cursor.scope.hub.to_string(), cursor.scope.scope.as_str(), id.as_str(), fastctx_types::sync::PAGE_BYTES], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?, row.get::<_, Vec<u8>>(2)?, row.get::<_, Vec<u8>>(3)?, row.get::<_, String>(4)?))
        }).optional()?.ok_or_else(|| corrupt("the stream's original source proof is missing"))?;
    if state != "accepted" {
        return Err(unavailable("the source batch is not accepted by the hub"));
    }
    let purpose = ProofPurpose::from_domain(&purpose).ok_or_else(|| corrupt("unknown stored proof domain"))?;
    let proof = epoch::statement(
        purpose,
        data.ok_or_else(|| unavailable("the source proof exceeds a metadata page"))?,
        signer,
        signature,
    )?;
    if proof.id() != *id {
        return Err(corrupt("stored source proof differs from its identity"));
    }
    let batch: SourceBatch = serde_json::from_slice(&proof.data.0)?;
    let domain = match epoch.producer {
        crate::identity::Producer::Hub => ProofPurpose::HubSourceBatchV1,
        crate::identity::Producer::Device { .. } => ProofPurpose::SourceBatchV1,
    };
    if batch.records.len() > 256
        || purpose != domain
        || batch.scope != cursor.scope
        || batch.stream != cursor.stream
        || batch.epoch != cursor.epoch
        || batch.producer != epoch.producer
        || batch.kind != epoch.kind
        || batch.schema != epoch.schema
    {
        return Err(corrupt("source proof differs from the requested stream epoch"));
    }
    batch.validate_shape().map_err(corrupt)?;
    Ok((SignedSourceBatch { proof }, batch))
}
