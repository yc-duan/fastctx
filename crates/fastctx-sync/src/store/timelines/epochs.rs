//! Installation follows the confirmed producer chain, finishing its predecessor
//! before consuming a successor. Arrival order never selects the active epoch.
use super::state::{self, Stream};
use crate::{
    cursor::{LocalSequence, Sequence},
    identity::{ObjectKey, ProducerEpoch, StreamId},
    store::sql,
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) fn stream(db: &Connection, conversation: &ObjectKey) -> Result<Option<Stream>> {
    let scope = &conversation.scope;
    if let Some(head) = state::head(db, conversation)? {
        let current = get(db, conversation, &head.through.stream, &head.through.epoch)?;
        if current.as_ref().is_some_and(|s| s.sequence > head.through.committed) {
            return Ok(current);
        }
        let next:Option<String>=db.query_row("SELECT successor FROM sync_replica_epoch_fences WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND closed_through=?5",params![scope.hub.to_string(),scope.scope.as_str(),head.through.stream.as_str(),head.through.epoch.as_str(),head.through.committed.0.to_be_bytes().as_slice()],|r|r.get(0)).optional()?;
        if let Some(next) = next {
            let epoch = ProducerEpoch::try_from(next).map_err(sql::invalid)?;
            if let Some(found) = get(db, conversation, &head.through.stream, &epoch)? {
                return Ok(Some(found));
            }
        }
        return Ok(current);
    }
    let mut q=db.prepare_cached("SELECT h.stream,h.epoch,h.sequence,h.local FROM sync_stream_heads h LEFT JOIN sync_epoch_descriptors e ON e.hub=h.hub AND e.scope=h.scope AND e.stream=h.stream AND e.epoch=h.epoch WHERE h.hub=?1 AND h.scope=?2 AND h.object=?3 AND h.kind='conversation.timeline' AND (h.origin='local' OR e.predecessor IS NULL AND e.proof IS NOT NULL) LIMIT 2")?;
    let found = q
        .query_map(params![scope.hub.to_string(), scope.scope.as_str(), conversation.object.as_str()], row)?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    match found.len() {
        0 => Ok(None),
        1 => parse(found.into_iter().next().unwrap()).map(Some),
        _ => Err(sql::invalid("conversation has more than one initial producer epoch")),
    }
}
fn get(db: &Connection, conversation: &ObjectKey, stream: &StreamId, epoch: &ProducerEpoch) -> Result<Option<Stream>> {
    db.query_row("SELECT stream,epoch,sequence,local FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND object=?3 AND kind='conversation.timeline' AND stream=?4 AND epoch=?5",params![conversation.scope.hub.to_string(),conversation.scope.scope.as_str(),conversation.object.as_str(),stream.as_str(),epoch.as_str()],row).optional()?.map(parse).transpose()
}
fn row(r: &fastctx_storage::rusqlite::Row<'_>) -> fastctx_storage::rusqlite::Result<(String, String, u64, u64)> {
    Ok((r.get(0)?, r.get(1)?, sql::counter(r, 2)?, sql::counter(r, 3)?))
}
fn parse((stream, epoch, sequence, local): (String, String, u64, u64)) -> Result<Stream> {
    Ok(Stream {
        stream: StreamId::try_from(stream).map_err(sql::invalid)?,
        epoch: ProducerEpoch::try_from(epoch).map_err(sql::invalid)?,
        sequence: Sequence(sequence),
        local: LocalSequence(local),
    })
}
pub(super) fn follows(db: &Connection, previous: &crate::cursor::StreamCursor, next: &Stream) -> Result<bool> {
    if previous.stream != next.stream {
        return Ok(false);
    }
    db.query_row("SELECT EXISTS(SELECT 1 FROM sync_replica_epoch_fences WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND successor=?5 AND closed_through=?6)",params![previous.scope.hub.to_string(),previous.scope.scope.as_str(),previous.stream.as_str(),previous.epoch.as_str(),next.epoch.as_str(),previous.committed.0.to_be_bytes().as_slice()],|r|r.get(0)).map_err(Into::into)
}
