//! A step's private supply belongs to its original batch and deterministic parts.
use super::model::Request;
use crate::{
    blocks::pins,
    cursor::Sequence,
    identity::{ProducerEpoch, SourceId, StreamId},
    proof::{SOURCE_RECORDS, part_operation},
    store::sql,
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) fn retained(db: &Connection, request: &Request, source: &SourceId) -> Result<bool> {
    let object = &request.object;
    let scope = &object.scope;
    let binding = db.query_row(
        "SELECT p.stream,p.epoch,p.first_sequence FROM sync_asset_uploads u JOIN sync_source_step_targets t ON t.hub=u.hub AND t.scope=u.source_scope AND t.source=u.source AND t.step=u.source_step AND t.target_scope=u.scope AND t.object=u.object AND t.kind=u.kind JOIN sync_source_proofs p ON p.hub=t.hub AND p.scope=t.target_scope AND p.stream=t.stream AND p.operation=u.operation WHERE u.hub=?1 AND u.scope=?2 AND u.object=?3 AND u.kind=?4 AND u.operation=?5 AND t.target_source=?6",
        params![scope.hub.to_string(),scope.scope.as_str(),object.object.as_str(),request.kind,request.operation.as_str(),source.as_str()],
        |row| Ok((row.get::<_, String>(0)?,row.get::<_, String>(1)?,sql::counter(row,2)?)),
    ).optional()?;
    let Some((stream, epoch, first)) = binding else { return Ok(false) };
    let stream = StreamId::try_from(stream).map_err(sql::invalid)?;
    let epoch = ProducerEpoch::try_from(epoch).map_err(sql::invalid)?;
    let last = first.saturating_add(SOURCE_RECORDS as u64 - 1);
    let (form, length) = pins::shape(&request.root);
    let mut query = db.prepare_cached(
        "WITH facts AS MATERIALIZED (SELECT sequence,kind,batch_proof FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence BETWEEN ?5 AND ?6) SELECT DISTINCT p.operation,p.first_sequence FROM facts f CROSS JOIN sync_source_proofs p ON p.hub=?1 AND p.scope=?2 AND p.proof=f.batch_proof AND p.stream=?3 AND p.epoch=?4 CROSS JOIN sync_stream_roots r ON r.hub=?1 AND r.scope=?2 AND r.stream=?3 AND r.epoch=?4 AND r.sequence=f.sequence WHERE p.first_sequence>=?5 AND p.last_sequence<=?6 AND f.sequence BETWEEN p.first_sequence AND p.last_sequence AND f.kind=?7 AND r.hash=?8 AND r.root_kind=?9 AND r.decoded_bytes IS ?10",
    )?;
    let mut rows = query.query(params![
        scope.hub.to_string(),
        scope.scope.as_str(),
        stream.as_str(),
        epoch.as_str(),
        first.to_be_bytes().as_slice(),
        last.to_be_bytes().as_slice(),
        request.kind,
        request.root.id().digest.as_str(),
        form,
        length,
    ])?;
    while let Some(row) = rows.next()? {
        let operation: String = row.get(0)?;
        let first = Sequence(sql::counter(row, 1)?);
        if operation == request.operation.as_str()
            || operation == part_operation(&request.operation, scope, &stream, &epoch, first).as_str()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
