//! Prefixes use the original record, including the original order of its roots.
use crate::{
    catalog::SourceBatch,
    cursor::Sequence,
    identity::{Digest, ProducerEpoch, ScopeKey, StreamId},
    store::sql,
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, params},
};
use std::collections::BTreeMap;

/// A bounded page over the stream's sequence index. Proof decoding and hashing
/// happen on a read snapshot, outside the writer transaction.
pub(super) fn page(
    db: &Connection,
    scope: &ScopeKey,
    stream: &StreamId,
    epoch: &ProducerEpoch,
    after: Sequence,
    previous: &Digest,
) -> Result<Vec<(Sequence, Digest)>> {
    let keys = db.prepare_cached("SELECT sequence,batch_proof FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence>?5 ORDER BY sequence LIMIT 64")?
        .query_map(params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),epoch.as_str(),after.0.to_be_bytes().as_slice()],|r|Ok((Sequence(sql::counter(r,0)?),r.get::<_,String>(1)?)))?
        .collect::<std::result::Result<Vec<_>,_>>()?;
    let mut cache = BTreeMap::<String, SourceBatch>::new();
    let mut bytes = 0;
    let mut digest = previous.clone();
    let mut sequence = after;
    let mut page = Vec::new();
    for (next, proof) in keys {
        if sequence.0.checked_add(1) != Some(next.0) {
            return Err(sql::invalid("local source prefix encountered a record gap"));
        }
        if !cache.contains_key(&proof) {
            let data: Vec<u8> = db
                .prepare_cached("SELECT signed_bytes FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND proof=?3")?
                .query_row(params![scope.hub.to_string(), scope.scope.as_str(), proof], |r| r.get(0))?;
            if data.len() > fastctx_types::sync::PAGE_BYTES as usize {
                return Err(sql::invalid("stored source proof exceeds its metadata bound"));
            }
            if bytes + data.len() > fastctx_types::sync::PAGE_BYTES as usize * 2 {
                break;
            }
            bytes += data.len();
            let batch: SourceBatch = serde_json::from_slice(&data).map_err(|e| sql::invalid(e.to_string()))?;
            batch.validate_shape().map_err(sql::invalid)?;
            if batch.scope != *scope || batch.stream != *stream || batch.epoch != *epoch {
                return Err(sql::invalid("record's proof belongs to another stream"));
            }
            cache.insert(proof.clone(), batch);
        }
        let batch = &cache[&proof];
        let record = batch
            .records
            .iter()
            .find(|r| r.sequence == next)
            .ok_or_else(|| sql::invalid("record is absent from its original proof"))?;
        digest = fastctx_types::sync::feed::stream_advance(&digest, record).map_err(sql::invalid)?.1;
        sequence = next;
        page.push((sequence, digest.clone()));
    }
    Ok(page)
}
