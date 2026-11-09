use super::{api::*, emit, state::Engine};
use crate::store::{
    ingest::IngestFacts,
    ingest_effects::{IndexValue, SourceEffects, SourceIndexChange},
};

fn effect_rows(engine: &Engine<'_>, other: &SourceEffects) -> (Vec<SourceIndexChange>, usize) {
    let mut indexes = other.indexes().to_vec();
    indexes.extend(engine.rows.changes());
    indexes.sort_by(|a, b| (a.family as u8, &a.key).cmp(&(b.family as u8, &b.key)));
    for index in &mut indexes {
        index.key = std::mem::take(&mut index.key).into_boxed_slice().into_vec();
        if let IndexValue::Put(v) = &mut index.value {
            *v = std::mem::take(v).into_boxed_slice().into_vec();
        }
    }
    let indexes = indexes.into_boxed_slice().into_vec();
    let bytes = indexes.capacity() * size_of::<SourceIndexChange>()
        + other.dirty().len() * 512
        + other.roots().len() * 4096
        + indexes
            .iter()
            .map(|r| {
                r.key.capacity()
                    + match &r.value {
                        IndexValue::Put(v) => v.capacity(),
                        IndexValue::Delete => 0,
                    }
            })
            .sum::<usize>();
    (indexes, bytes)
}
pub(super) fn effects(engine: &Engine<'_>, other: &SourceEffects) -> Result<(SourceEffects, usize)> {
    let (indexes, bytes) = effect_rows(engine, other);
    let dirty = other.dirty().to_vec().into_boxed_slice().into_vec();
    let roots = other.roots().to_vec().into_boxed_slice().into_vec();
    Ok((SourceEffects::new(indexes, dirty, roots)?, bytes))
}
pub(super) fn usage(
    engine: &Engine<'_>,
    after: crate::cursor::Sequence,
    other: &SourceEffects,
    units: u32,
) -> Result<MergeUsage> {
    let (indexes, effect_bytes) = effect_rows(engine, other);
    let facts = emit::encode_batch(&engine.content, &engine.target, after, &engine.inputs, &engine.changes, false)?;
    let mut future_publication = 0usize;
    let (records, payload_bytes, batch_bytes) = match facts {
        Some(IngestFacts::Local { batch }) => {
            for record in &batch.records {
                if !engine.content.has_block(&record.payload)? {
                    future_publication = future_publication.saturating_add(record.payload.decoded_bytes as usize + 512);
                }
            }
            let bytes = serde_json::to_vec(&batch).map_err(|e| MergeError::InvalidInput(e.to_string()))?.len();
            (batch.records.len(), batch.records.iter().map(|r| u64::from(r.payload.decoded_bytes)).sum::<u64>(), bytes)
        }
        _ => (0, 0, 0),
    };
    let input_bytes = engine
        .inputs
        .iter()
        .map(|i| serde_json::to_vec(i).map(|v| v.len()).unwrap_or(usize::MAX / 1024))
        .sum::<usize>();
    let change_bytes = engine
        .changes
        .iter()
        .map(|c| serde_json::to_vec(&(&c.change, &c.trigger, &c.causes)).map(|v| v.len()).unwrap_or(usize::MAX / 1024))
        .sum::<usize>();
    let work_bytes = engine
        .rows
        .bytes()
        .saturating_add(engine.content.bytes()?)
        .saturating_add(input_bytes)
        .saturating_add(change_bytes)
        .saturating_add(future_publication)
        .saturating_add(effect_bytes)
        .saturating_add(records * 512)
        .saturating_mul(2);
    Ok(MergeUsage {
        effects: indexes.len() + other.dirty().len() + other.roots().len(),
        effect_bytes,
        records,
        payload_bytes,
        commit_bytes: effect_bytes + batch_bytes,
        work_bytes,
        work_units: units,
        content_reads: engine.content.reads(),
    })
}
pub(super) fn exceeded(u: &MergeUsage, b: &MergeBudget) -> Option<BudgetKind> {
    if u.effects > b.max_effects {
        Some(BudgetKind::Effects)
    } else if u.effect_bytes > b.max_effect_bytes {
        Some(BudgetKind::EffectBytes)
    } else if u.records > b.max_records {
        Some(BudgetKind::Records)
    } else if u.payload_bytes > b.max_payload_bytes {
        Some(BudgetKind::PayloadBytes)
    } else if u.commit_bytes > b.max_commit_bytes {
        Some(BudgetKind::CommitBytes)
    } else if u.work_bytes > b.max_work_bytes {
        Some(BudgetKind::WorkBytes)
    } else if u.content_reads > b.max_content_reads {
        Some(BudgetKind::ContentReads)
    } else {
        None
    }
}
pub(super) fn too_large(needed: MergeUsage, available: &MergeBudget, _kind: BudgetKind) -> MergeError {
    MergeError::UnitTooLarge { needed: Box::new(needed), available: Box::new(available.clone()), limit: _kind }
}
