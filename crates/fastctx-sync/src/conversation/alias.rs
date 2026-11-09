use super::*;
use fastctx_types::sync::{
    conversation_identity::{AliasBinding, AliasSource},
    conversation_timeline::*,
    timeline::TimelineChange,
};

#[derive(Clone, Debug)]
pub struct AliasReduction {
    pub source: ItemState,
    pub target: ItemState,
    pub effects: Effects,
}

/// Receipt verification is the caller's responsibility. Equal bodies or times
/// never establish an alias. A pending target retains the source's visible row
/// until the target Put and source Retract can be committed together.
pub fn bind_alias(
    binding: &AliasBinding,
    source: &ItemState,
    target: &ItemState,
    neighbors: Neighbors<'_>,
) -> Result<AliasReduction, Error> {
    let expected = match binding.source() {
        AliasSource::Provisional(key) => ItemIdentity::Provisional(key.clone()),
        AliasSource::Legacy(key) => ItemIdentity::Native(key.clone()),
    };
    if source.identity != expected || target.identity != ItemIdentity::Native(binding.target().clone()) {
        return Err(Error::InvalidAlias);
    }
    if target.alias.is_some() || source.alias.as_ref().is_some_and(|id| id != &target.id()) {
        return Err(Error::InvalidAlias);
    }
    let mut from = source.clone();
    let mut to = target.clone();
    let mut effects = Effects::default();

    for (lane, body) in &from.bodies {
        if to.bodies.get(lane).map(|old| old.accepts(body)).transpose()?.unwrap_or(true) {
            to.bodies.insert(*lane, body.clone());
        }
    }
    if let Some(placed) = &from.placement {
        placement::choose(&mut to.placement, placed.clone())?;
    }
    if let Some(run) = &from.run
        && to.run.as_ref().map(|old| old.accepts(run)).transpose()?.unwrap_or(true)
    {
        to.run = Some(run.clone());
    }
    to.display_created = to.display_created.min(from.display_created);
    if from.alias.is_none() {
        effects.indexes.push(IndexEffect::Alias { from: from.id(), to: to.id() });
    }
    from.alias = Some(to.id());
    placement::position(&mut to, neighbors, &mut effects)?;
    placement::publish(&mut to, &mut effects)?;
    if !to.visible || effects.pending.is_empty() && to.published.is_some() {
        if from.visible && from.published.is_some() {
            from.revision = next(from.revision)?;
            effects.changes.push(TimelineChange::Retract { item: from.id(), revision: from.revision });
        }
        from.visible = false;
        from.published = None;
    }
    Ok(AliasReduction { source: from, target: to, effects })
}

/// Re-keys a derived observation through a retained, verified alias. The caller
/// keeps the original TimelineFact and its evidence unchanged in the source log.
pub fn redirect_observation(
    observation: &Observation,
    source: &ItemState,
    target: &ItemState,
) -> Result<Observation, Error> {
    if observation.identity != source.identity
        || source.alias.as_ref() != Some(&target.id())
        || source.identity.conversation() != target.identity.conversation()
        || source.identity.role() != target.identity.role()
    {
        return Err(Error::InvalidAlias);
    }
    let mut derived = observation.clone();
    derived.identity = target.identity.clone();
    derived.created = target.created;
    Ok(derived)
}
