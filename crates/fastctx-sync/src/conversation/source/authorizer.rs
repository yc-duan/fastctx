use super::event;
use crate::{
    identity::{ObjectId, Producer},
    store::{
        access::{Decision, HoldingUse},
        source_authorize::{PreparedSourceChange, SourceAuthorizer},
    },
};
use fastctx_types::sync::{
    conversation_identity::ConversationObject,
    conversation_timeline::{KIND, SCHEMA, TimelineFact},
};

/// The live host's timeline gate. The engine first authenticates RuntimeSource
/// provenance and custody; merely knowing a conversation ID is not authority.
/// Legacy imports and bridges need their own registered provenance adapter and
/// may share `event` validation without impersonating a live host.
pub struct TimelineAuthorizer;
impl SourceAuthorizer for TimelineAuthorizer {
    fn kind(&self) -> &'static str {
        KIND
    }
    fn authorize(&self, change: PreparedSourceChange<'_>) -> Decision {
        let revision = change.target.custody.context.observed_acl;
        match validate(&change) {
            Ok(()) => Decision::Allowed { revision },
            Err(reason) => Decision::Denied { revision, reason },
        }
    }
}

fn validate(change: &PreparedSourceChange<'_>) -> Result<(), String> {
    let target = change.target;
    let custody = &target.custody;
    let scope = &custody.object.scope;
    let registration = custody.kind.registration();
    let HoldingUse::RuntimeSource { host } = &custody.purpose else {
        return Err("timeline source requires authenticated host provenance".into());
    };
    if registration.id != KIND
        || registration.schema != SCHEMA
        || host.conversation.scope.hub != scope.hub
        || custody.object.object != ConversationObject::Timeline.id(&host.conversation.object)
        || target.producer != (Producer::Device { device: host.host.clone() })
    {
        return Err("timeline target differs from its host conversation or producer".into());
    }
    if change.checkpoint.revision.0 == 0 || change.checkpoint.parser_version == 0 {
        return Err("timeline checkpoint has no revision or parser version".into());
    }
    change.effects.validate_bounds().map_err(|error| error.to_string())?;
    change.effects.validate_target(target).map_err(|error| error.to_string())?;
    if change
        .effects
        .dirty()
        .iter()
        .any(|dirty| dirty.kind.registration().id != "derived.timeline" || dirty.object != custody.object)
    {
        return Err("timeline source dirties an unrelated projection".into());
    }
    let Some(batch) = change.batch else {
        if !change.payloads.is_empty() {
            return Err("checkpoint-only source carries record payloads".into());
        }
        return Ok(());
    };
    batch.validate_shape()?;
    if batch.records.len() > 256
        || batch.scope != *scope
        || batch.stream != target.stream
        || batch.epoch != target.epoch
        || batch.producer != target.producer
        || batch.kind != KIND
        || batch.schema != SCHEMA
        || batch.records.len() != change.payloads.len()
    {
        return Err("timeline batch differs from its authenticated destination".into());
    }
    if change.effects.dirty().is_empty() {
        return Err("timeline facts must advance their projection in the same commit".into());
    }
    let mut output_started = false;
    let mut previous_input = None;
    for (record, payload) in batch.records.iter().zip(change.payloads) {
        if record != payload.record {
            return Err("timeline payload order differs from its batch".into());
        }
        let value = event::decode(payload.bytes)?;
        if value.evidence.record.stream != batch.stream || value.evidence.record.epoch != batch.epoch {
            return Err("timeline evidence points outside its enclosing stream".into());
        }
        event::validate_record(&host.conversation.object, scope, &value, record, payload.bytes)?;
        if matches!(value.fact, TimelineFact::ItemRevision { .. }) {
            output_started = true;
        } else {
            if output_started || previous_input.is_some_and(|previous| previous >= value.evidence.accepted) {
                return Err("timeline inputs must precede outputs in canonical accepted order".into());
            }
            previous_input = Some(value.evidence.accepted);
        }
        checkpoint(change, &value.fact, &host.conversation.object)?;
    }
    Ok(())
}

fn checkpoint(change: &PreparedSourceChange<'_>, fact: &TimelineFact, conversation: &ObjectId) -> Result<(), String> {
    if let TimelineFact::HostRewritten { rewrite } = fact {
        // Fan-out destinations carry only the primary checkpoint. The source ID
        // inside this fact names that original parser, not a destination alias.
        if rewrite.source != change.checkpoint.source || rewrite.generation != change.checkpoint.generation {
            return Err("host rewrite differs from the committed parser generation".into());
        }
    }
    if let TimelineFact::Agent { agent } = fact
        && &agent.parent.object != conversation
    {
        return Err("agent edge belongs to another parent conversation".into());
    }
    Ok(())
}
