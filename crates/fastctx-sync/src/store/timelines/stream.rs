//! A page of a conversation's source stream, each record as its producer signed
//! it: records are taken from the original signed batches, never rebuilt from
//! rows, so any holder can verify and forward them unchanged.
use super::{SqliteTimelines, state};
use crate::{
    catalog::{SourceBatch, SourceRecord},
    cursor::Sequence,
    identity::{Digest, ObjectId, ObjectKey},
    store::{Error, access::AccessContext, sql},
    timeline::{Direction, ProvenRecord, StreamPage, StreamPosition, StreamRead},
};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::sync::transport_codec::{CodecError, bounded_json};
use std::collections::BTreeMap;

impl SqliteTimelines {
    pub(super) async fn read_stream(&self, context: AccessContext, request: StreamRead) -> Result<StreamPage, Error> {
        let cursor = request.stream.clone();
        if request.max_records == 0 || request.max_records > 256 || request.max_bytes == 0 {
            return Err(Error::Invalid("a stream page needs 1 to 256 records and a byte budget".into()));
        }
        let scope = cursor.scope.clone();
        let (stream, epoch) = (cursor.stream.clone(), cursor.epoch.clone());
        let owner = self
            .blocks
            .database()
            .read(move |db| {
                db.prepare_cached("SELECT object,sequence FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
                    .query_row(
                        params![scope.hub.to_string(), scope.scope.as_str(), stream.as_str(), epoch.as_str()],
                        |row| Ok((row.get::<_, String>(0)?, sql::counter(row, 1)?)),
                    )
                    .optional()
                    .map_err(Into::into)
            })
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?
            .ok_or_else(|| Error::Unavailable("the stream has no committed records here".into()))?;
        let object =
            ObjectKey { scope: cursor.scope.clone(), object: ObjectId::try_from(owner.0).map_err(Error::Invalid)? };
        self.authorize(&context, &object)?;
        let returned_object = object.clone();
        let head = Sequence(owner.1);
        let through = request.through.map_or(head, |through| Sequence(through.0.min(head.0)));
        let (direction, low, high, descending) = match request.position {
            StreamPosition::Latest => (Direction::Backward, 0, through.0, true),
            StreamPosition::Before(sequence) => {
                (Direction::Backward, 0, sequence.0.saturating_sub(1).min(through.0), true)
            }
            StreamPosition::After(sequence) => (Direction::Forward, sequence.0.saturating_add(1), through.0, false),
        };
        let read = cursor.clone();
        let limit = request.max_records;
        let rows = self
            .blocks
            .database()
            .read(move |db| {
                let statement = if descending {
                    "SELECT sequence,batch_proof FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence>=?5 AND sequence<=?6 ORDER BY sequence DESC LIMIT ?7"
                } else {
                    "SELECT sequence,batch_proof FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence>=?5 AND sequence<=?6 ORDER BY sequence LIMIT ?7"
                };
                let rows = db
                    .prepare_cached(statement)?
                    .query_map(
                        params![
                            read.scope.hub.to_string(),
                            read.scope.scope.as_str(),
                            read.stream.as_str(),
                            read.epoch.as_str(),
                            low.to_be_bytes().as_slice(),
                            high.to_be_bytes().as_slice(),
                            limit + 1
                        ],
                        |row| Ok((sql::counter(row, 0)?, row.get::<_, String>(1)?)),
                    )?
                    .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
                let mut proofs = BTreeMap::new();
                for (_, proof) in &rows {
                    if !proofs.contains_key(proof) {
                        let id = Digest::try_from(proof.clone()).map_err(sql::invalid)?;
                        proofs.insert(proof.clone(), state::proof(db, &object, &id)?);
                    }
                }
                let progress = state::progress(db, &object, &crate::cursor::StreamCursor { committed: head, ..read })?;
                Ok((rows, proofs, progress))
            })
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
        let (rows, proofs, fence) = rows;
        let mut batches: BTreeMap<String, SourceBatch> = BTreeMap::new();
        for (id, signed) in &proofs {
            let batch: SourceBatch = serde_json::from_slice(&signed.proof.data.0)
                .map_err(|error| Error::Corrupt(format!("a stored source proof does not hold a batch: {error}")))?;
            batches.insert(id.clone(), batch);
        }
        // Reserve the longest continuation counter before adding records or proofs.
        let mut page =
            StreamPage { fence, direction, records: Vec::new(), proofs: Vec::new(), next: Some(Sequence(u64::MAX)) };
        let budget = request.max_bytes as usize;
        let mut bytes = encoded_size(&page, budget)?
            .ok_or_else(|| Error::Invalid("the byte budget cannot hold a stream page envelope".into()))?;
        let mut used = BTreeMap::new();
        let mut next = None;
        for (index, (sequence, proof)) in rows.iter().enumerate() {
            let record: SourceRecord = batches[proof]
                .records
                .iter()
                .find(|record| record.sequence.0 == *sequence)
                .cloned()
                .ok_or_else(|| Error::Corrupt("a stored record is missing from its signed batch".into()))?;
            let record = ProvenRecord { record, batch_proof: Digest::try_from(proof.clone()).map_err(Error::Invalid)? };
            let fresh = !used.contains_key(proof);
            let proof_bytes = if fresh { encoded_size(&proofs[proof], budget)? } else { Some(0) };
            let needed = encoded_size(&record, budget)?.zip(proof_bytes).and_then(|(record, proof)| {
                bytes
                    .checked_add(record)?
                    .checked_add(usize::from(!page.records.is_empty()))?
                    .checked_add(proof)?
                    .checked_add(usize::from(fresh && !used.is_empty()))
            });
            if index == limit as usize || needed.is_none_or(|needed| needed > budget) {
                if page.records.is_empty() {
                    return Err(Error::Invalid(
                        "the byte budget cannot hold one record and its original batch proof".into(),
                    ));
                }
                next = page.records.last().map(|last| last.record.sequence);
                break;
            }
            bytes = needed.expect("checked page size");
            used.insert(proof.clone(), ());
            page.records.push(record);
        }
        if descending {
            page.records.reverse();
        }
        page.next = next;
        page.proofs = used.into_keys().map(|id| proofs[&id].clone()).collect();
        self.authorize(&context, &returned_object)?;
        Ok(page)
    }
}

/// Bound the full JSON representation, including base64 and signature envelopes.
fn encoded_size(value: &impl serde::Serialize, budget: usize) -> Result<Option<usize>, Error> {
    match bounded_json(value, budget) {
        Ok(bytes) => Ok(Some(bytes.len())),
        Err(CodecError::TooLarge) => Ok(None),
        Err(error) => Err(Error::Corrupt(format!("encoding a stored stream page: {error}"))),
    }
}
