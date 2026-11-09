//! Bounded, side-effect-free plans for publishing a pair of immutable indexes.
use super::{
    TimelineChange,
    index::{self, IndexChange},
    pages::{self, Budget, EncodedPage, Meter, Outcome, PageSource, Work},
    position,
    tree::{self, Entry, Kind, Overlay},
};
use crate::{
    catalog::RecordRef,
    cursor::{Revision, StreamCursor},
    identity::ObjectKey,
    manifest::{ContentBlock, ManifestRoot},
    store::Error,
};
use fastctx_types::sync::metadata::{
    IndexedTimelineItem, ProofReference, TimelineLookupEntry, TimelinePayload, TimelineRoot,
};
#[derive(Clone, Debug)]
pub struct Projection {
    pub conversation: ObjectKey,
    pub revision: Revision,
    pub through: StreamCursor,
}
#[derive(Clone, Debug)]
pub struct SourceEvidence {
    pub record: RecordRef,
    pub payload: ContentBlock,
    pub proof: ProofReference,
}
#[derive(Clone, Debug)]
pub struct Update {
    pub change: TimelineChange,
    pub evidence: Option<SourceEvidence>,
}
#[derive(Debug)]
pub struct UpdatePlan {
    pub expected_root: Option<ManifestRoot>,
    pub root: ManifestRoot,
    pub snapshot: TimelineRoot,
    pub pages: Vec<EncodedPage>,
    pub indexes: Vec<IndexChange>,
    pub rebuild_index: bool,
}
/// Plans canonical reducer results whose source permissions, proof bindings and
/// replay order the executor has already verified. Retraction revision fences
/// remain in reducer state, not this visible-item index. The executor compares
/// expected_root and index preconditions, then installs pages, rows and durable
/// projection watermarks/notifications. Busy/Pending publish nothing.
pub fn plan_update(
    source: &impl PageSource,
    previous: Option<&ManifestRoot>,
    projection: &Projection,
    updates: &[Update],
    budget: Budget,
) -> Result<Outcome<UpdatePlan>, Error> {
    pages::outcome(plan(source, previous, projection, updates, budget))
}
fn plan(
    source: &impl PageSource,
    previous: Option<&ManifestRoot>,
    projection: &Projection,
    updates: &[Update],
    budget: Budget,
) -> Work<UpdatePlan> {
    if updates.len() > budget.max_changes {
        return Err(Error::Busy.into());
    }
    if projection.revision.0 == 0 || projection.conversation.scope != projection.through.scope {
        return Err(Error::Invalid("invalid timeline projection context".into()).into());
    }
    let mut meter = Meter::new(budget);
    let old = previous.map(|r| tree::snapshot(source, r, &mut meter)).transpose()?;
    if old.as_ref().is_some_and(|old| {
        old.conversation != projection.conversation
            || old.projection >= projection.revision
            || regresses(&old.through, &projection.through)
    }) {
        return Err(Error::Invalid("timeline projection must advance the same conversation".into()).into());
    }
    let mut root = TimelineRoot {
        conversation: projection.conversation.clone(),
        projection: projection.revision,
        through: projection.through.clone(),
        items: old.as_ref().and_then(|v| v.items.clone()),
        lookup: old.as_ref().and_then(|v| v.lookup.clone()),
    };
    let mut overlay = Overlay::new(source);
    let mut indexes = Vec::new();
    for update in updates {
        let size = fastctx_types::sync::transport_codec::bounded_json(&update.change, budget.work_bytes)
            .map_err(|_| Error::Busy)?
            .len();
        meter.charge(size.checked_mul(8).ok_or(Error::Busy)?)?;
        if let TimelineChange::SelectBranch { root: selected, projection: revision } = &update.change {
            if updates.len() != 1 || update.evidence.is_some() {
                return Err(Error::Invalid("branch selection needs its own bounded publication".into()).into());
            }
            let selected_root = tree::snapshot(source, selected, &mut meter)?;
            if selected_root.conversation != projection.conversation
                || selected_root.projection != *revision
                || selected_root.projection > projection.revision
                || regresses(&selected_root.through, &projection.through)
            {
                return Err(Error::Invalid("selected branch root differs from its identity or revision".into()).into());
            }
            root.items = selected_root.items;
            root.lookup = selected_root.lookup;
            let page =
                pages::encode_payload(&projection.conversation.scope, &TimelinePayload::Root(Box::new(root.clone())))?
                    .ok_or(Error::Busy)?;
            let result = overlay.add(page, &mut meter)?;
            return Ok(UpdatePlan {
                expected_root: previous.cloned(),
                root: result.clone(),
                snapshot: root,
                pages: overlay.finish(&result)?,
                indexes,
                rebuild_index: true,
            });
        }
        let id = match &update.change {
            TimelineChange::Put { item } => &item.id,
            TimelineChange::Retract { item, .. } => item,
            _ => unreachable!(),
        };
        let old = tree::item(&overlay, &root, id, &mut meter)?;
        let old_item = match &old {
            Some(Entry::Item(e, _)) => Some(&e.item),
            _ => None,
        };
        match &update.change {
            TimelineChange::Put { item } => {
                let evidence = update
                    .evidence
                    .as_ref()
                    .ok_or_else(|| Error::Invalid("timeline Put has no original source evidence".into()))?;
                if let Some(Entry::Item(previous, proof)) = &old
                    && previous.item == *item
                    && (previous.record != evidence.record
                        || previous.payload != evidence.payload
                        || previous.batch != evidence.proof.id
                        || proof != &evidence.proof)
                {
                    return Err(Error::Invalid("same timeline revision has different source evidence".into()).into());
                }
            }
            TimelineChange::Retract { .. } if update.evidence.is_some() => {
                return Err(Error::Invalid("retraction unexpectedly carries Put evidence".into()).into());
            }
            _ => {}
        }
        let effect = index::index_changes(old_item, &update.change)?;
        if effect.is_none() {
            continue;
        }
        if let Some(old) = old_item {
            root.items = overlay.replace(
                root.items.as_ref(),
                Kind::Items,
                &root,
                &position::key(&old.position)?,
                None,
                &mut meter,
            )?;
        }
        let replacement = match &update.change {
            TimelineChange::Put { item } => {
                let evidence = update
                    .evidence
                    .as_ref()
                    .ok_or_else(|| Error::Invalid("timeline Put has no original source evidence".into()))?;
                let entry = IndexedTimelineItem {
                    item: item.clone(),
                    record: evidence.record.clone(),
                    payload: evidence.payload.clone(),
                    batch: evidence.proof.id.clone(),
                };
                root.items = overlay.replace(
                    root.items.as_ref(),
                    Kind::Items,
                    &root,
                    &position::key(&item.position)?,
                    Some(Entry::Item(Box::new(entry), evidence.proof.clone())),
                    &mut meter,
                )?;
                Some(Entry::Lookup(TimelineLookupEntry {
                    item: item.id.clone(),
                    position: item.position.clone(),
                    revision: item.revision,
                }))
            }
            TimelineChange::Retract { .. } => None,
            _ => unreachable!(),
        };
        root.lookup = overlay.replace(
            root.lookup.as_ref(),
            Kind::Lookup,
            &root,
            &position::id_key(id),
            replacement,
            &mut meter,
        )?;
        indexes.push(effect.expect("checked effect"));
    }
    let page = pages::encode_payload(&projection.conversation.scope, &TimelinePayload::Root(Box::new(root.clone())))?
        .ok_or(Error::Busy)?;
    let result = overlay.add(page, &mut meter)?;
    Ok(UpdatePlan {
        expected_root: previous.cloned(),
        root: result.clone(),
        snapshot: root,
        pages: overlay.finish(&result)?,
        indexes,
        rebuild_index: false,
    })
}

fn regresses(before: &StreamCursor, after: &StreamCursor) -> bool {
    before.stream == after.stream && before.epoch == after.epoch && before.committed > after.committed
}
