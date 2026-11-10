use super::roots::{direct_roots, root_set};
use crate::{
    catalog::{RecordRef, SourceRecord},
    identity::{Digest, ObjectId, ScopeKey},
};
use fastctx_types::sync::{
    conversation_identity::ItemRole,
    conversation_timeline::{BodyAuthority, BranchChange, DisplaySlot, KIND, SCHEMA, TimelineEvent, TimelineFact},
    manifest::ContentRef,
    timeline::TimelineChange,
    transport_codec::bounded_json,
};

/// An event is metadata, not an inline host record or growing text body.
pub const EVENT_BYTES: usize = 64 * 1024;

pub fn encode(event: &TimelineEvent) -> Result<Vec<u8>, String> {
    bounded_json(event, EVENT_BYTES).map_err(|_| "timeline event exceeds 64 KiB".into())
}

pub fn decode(bytes: &[u8]) -> Result<TimelineEvent, String> {
    if bytes.len() > EVENT_BYTES {
        return Err("timeline event exceeds 64 KiB".into());
    }
    serde_json::from_slice(bytes).map_err(|_| "invalid conversation.timeline event".into())
}

/// Validate the self-contained schema. Prior item state, receipt authentication,
/// ancestor resolution and decoded Manifest extents belong to the producer's
/// admitted read phase; this function never invents those missing witnesses.
pub fn validate(conversation: &ObjectId, scope: &ScopeKey, event: &TimelineEvent) -> Result<Vec<ContentRef>, String> {
    if event.evidence.accepted.0 == 0 || event.evidence.record.sequence.0 == 0 {
        return Err("timeline evidence has no canonical order or record position".into());
    }
    match &event.fact {
        TimelineFact::Observed { observation } => {
            if observation.identity.conversation() != conversation
                || observation.created.0 == 0
                || (observation.authority == BodyAuthority::HostFinal && !observation.body.complete)
                || !matches!(observation.body.display, ContentRef::Manifest { .. })
                || (observation.body.text_part.is_none() && observation.body.text_bytes.0 != 0)
            {
                return Err("observation has inconsistent identity, display or completion".into());
            }
            if let Some(placement) = &observation.placement {
                if matches!(placement.slot, DisplaySlot::Opening) && observation.identity.role() != ItemRole::User {
                    return Err("only a question may occupy its turn's opening slot".into());
                }
                placement_shape(&observation.identity.id(), placement)?;
            }
        }
        TimelineFact::Turn { turn } => {
            // Leading host prefixes use an ordinary nonzero slot. A declaration
            // does not assert that an Opening item exists inside the record.
            if &turn.key.conversation != conversation || turn.order.0 == 0 {
                return Err("turn declaration differs from its conversation or order".into());
            }
        }
        TimelineFact::Placement { item, placement } => placement_shape(item, placement)?,
        TimelineFact::ItemRevision { change } => match change {
            TimelineChange::Put { item } => {
                if item.id != item.position.item
                    || item.revision.0 == 0
                    || item.position.accepted.0 == 0
                    || !super::super::order::valid_position(item)
                    || item.parent.as_ref() == Some(&item.id)
                    || !matches!(item.body, ContentRef::Manifest { .. })
                {
                    return Err("canonical item has inconsistent identity, position or body".into());
                }
            }
            TimelineChange::Retract { revision, .. } => {
                if revision.0 == 0 {
                    return Err("item retraction has no revision".into());
                }
            }
            TimelineChange::SelectBranch { projection, .. } => {
                if projection.0 == 0 {
                    return Err("branch selection has no projection revision".into());
                }
            }
        },
        TimelineFact::TextAppend { append } => {
            if append.authority != BodyAuthority::HostDraft
                || append.previous == event.evidence.operation
                || append.offset.0.checked_add(append.bytes.0).is_none()
                || matches!(append.chunk, ContentRef::Single { decoded_bytes, .. } if u64::from(decoded_bytes) != append.bytes.0)
            {
                return Err("text append has an invalid authority, predecessor or extent".into());
            }
        }
        TimelineFact::HostRewritten { rewrite } => {
            if rewrite.previous == rewrite.generation {
                return Err("host rewrite must change its host generation".into());
            }
            if let Some(prefix) = &rewrite.preserved {
                preceding(&prefix.through, event)?;
            }
        }
        TimelineFact::Alias { binding, receipt } => {
            if &binding.target().conversation != conversation {
                return Err("alias belongs to another conversation".into());
            }
            preceding(receipt, event)?;
        }
        TimelineFact::Branch { change: BranchChange::Select { projection, .. } } => {
            if projection.0 == 0 {
                return Err("branch selection has no projection revision".into());
            }
        }
        TimelineFact::Branch { change: BranchChange::Membership { present, rollback, .. } } => {
            if !present && rollback.is_none() {
                return Err("hidden membership requires a rollback receipt".into());
            }
            if let Some(receipt) = rollback {
                preceding(receipt, event)?;
            }
        }
        TimelineFact::Agent { agent } => {
            if &agent.parent.object != conversation
                || agent.parent == agent.child
                || agent.parent.scope.hub != scope.hub
                || agent.child.scope.hub != scope.hub
            {
                return Err("agent link has an inconsistent parent or hub".into());
            }
        }
        TimelineFact::Run { .. } | TimelineFact::Image { .. } => {}
    }
    direct_roots(scope, &event.fact)
}

/// Bind decoded evidence and owned roots to the authenticated enclosing record.
pub fn validate_record(
    conversation: &ObjectId,
    scope: &ScopeKey,
    event: &TimelineEvent,
    record: &SourceRecord,
    bytes: &[u8],
) -> Result<(), String> {
    if record.kind != KIND
        || record.schema != SCHEMA
        || event.evidence.operation != record.operation
        || event.evidence.record.sequence != record.sequence
        || record.payload.id.domain != *scope
        || record.payload.id.digest != Digest::of(bytes)
        || record.payload.decoded_bytes as usize != bytes.len()
    {
        return Err("timeline event differs from its source record".into());
    }
    let expected = validate(conversation, scope, event)?;
    if root_set(scope, &expected)? != root_set(scope, &record.roots)? {
        return Err("timeline payload and declared roots differ".into());
    }
    for cause in &record.causes {
        preceding(cause, event)?;
    }
    let required = match &event.fact {
        TimelineFact::Alias { receipt, .. } => Some(receipt),
        TimelineFact::Branch { change: BranchChange::Membership { rollback, .. } } => rollback.as_ref(),
        TimelineFact::HostRewritten { rewrite } => rewrite.preserved.as_ref().map(|prefix| &prefix.through),
        _ => None,
    };
    if required.is_some_and(|cause| !record.causes.contains(cause)) {
        return Err("timeline record omits a referenced receipt or prefix cause".into());
    }
    if matches!(event.fact, TimelineFact::ItemRevision { .. }) && record.causes.is_empty() {
        return Err("canonical item revision has no supporting input cause".into());
    }
    Ok(())
}

fn placement_shape(
    item: &ObjectId,
    placement: &fastctx_types::sync::conversation_timeline::Placement,
) -> Result<(), String> {
    use fastctx_types::sync::conversation_timeline::DisplayOrder;
    if matches!(placement.slot, DisplaySlot::Session) != placement.turn.is_none() {
        return Err("session placement and turn presence differ".into());
    }
    if matches!(&placement.slot, DisplaySlot::Child { parent } if parent == item) {
        return Err("item cannot be its own display parent".into());
    }
    match &placement.order {
        Some(DisplayOrder::Native { sequence }) if sequence.0 == 0 => {
            Err("native sibling order must be positive".into())
        }
        Some(DisplayOrder::Gap { before, sequence, .. })
            if sequence.0 == 0 || before.as_ref() == Some(item) || matches!(placement.slot, DisplaySlot::Opening) =>
        {
            Err("gap order has an invalid source sequence, anchor or opening slot".into())
        }
        _ => Ok(()),
    }
}
fn preceding(cause: &RecordRef, event: &TimelineEvent) -> Result<(), String> {
    let own = &event.evidence.record;
    if cause.sequence.0 == 0
        || (cause.stream == own.stream && cause.epoch == own.epoch && cause.sequence >= own.sequence)
    {
        return Err("timeline cause is empty, self-referential or later in its stream".into());
    }
    Ok(())
}
