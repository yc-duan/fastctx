use super::*;
use fastctx_types::sync::{
    conversation_identity::ItemRole, conversation_timeline::*, cursor::Sequence, timeline::TimelinePosition,
};

pub(super) fn choose(current: &mut Option<Versioned<Placement>>, incoming: Versioned<Placement>) -> Result<(), Error> {
    if let Some(old) = current {
        let newer = old.accepts(&incoming)?;
        let replace = match (old.value.proof.is_strong(), incoming.value.proof.is_strong()) {
            (true, false) => false,
            (false, true) => true,
            _ => newer,
        };
        if !replace {
            return Ok(());
        }
    }
    *current = Some(incoming);
    Ok(())
}

pub(super) fn position(state: &mut ItemState, neighbors: Neighbors<'_>, effects: &mut Effects) -> Result<(), Error> {
    let id = state.id();
    let Some(placed) = &state.placement else {
        effects.pending.push(Dependency::Placement(id));
        return Ok(());
    };
    let turn_order = match (&placed.value.slot, &placed.value.turn) {
        (DisplaySlot::Session, None) => Sequence(0),
        (DisplaySlot::Session, Some(_)) | (_, None) => return Err(Error::InvalidPlacement),
        (_, Some(id)) => {
            let Some(turn) = neighbors.turn else {
                effects.pending.push(Dependency::Turn(id.clone()));
                return Ok(());
            };
            if turn.declaration.key.id() != *id {
                return Err(Error::WrongKey);
            }
            if &turn.declaration.key.conversation != state.identity.conversation() {
                return Err(Error::WrongConversation);
            }
            turn.declaration.order
        }
    };
    let mut display_parent = None;
    let mut path = match &placed.value.slot {
        DisplaySlot::Session => vec![Sequence(0)],
        DisplaySlot::Opening => {
            if state.identity.role() != ItemRole::User {
                return Err(Error::InvalidPlacement);
            }
            vec![Sequence(0)]
        }
        DisplaySlot::Sibling => vec![Sequence(1)],
        DisplaySlot::Child { parent } => {
            if parent == &id {
                return Err(Error::Cycle);
            }
            let Some(parent_state) = neighbors.parent else {
                effects.pending.push(Dependency::Parent(parent.clone()));
                return Ok(());
            };
            if &parent_state.id() != parent {
                return Err(Error::WrongKey);
            }
            let parent_state = if let Some(target) = &parent_state.alias {
                let Some(resolved) = neighbors.resolved_parent else {
                    effects.pending.push(Dependency::Parent(target.clone()));
                    return Ok(());
                };
                if &resolved.id() != target || resolved.alias.is_some() {
                    return Err(Error::InvalidAlias);
                }
                resolved
            } else {
                parent_state
            };
            if parent_state.id() == id {
                return Err(Error::Cycle);
            }
            display_parent = Some(parent_state.id());
            if parent_state.identity.conversation() != state.identity.conversation() {
                return Err(Error::WrongConversation);
            }
            let Some(parent_position) = &parent_state.position else {
                effects.pending.push(Dependency::Parent(parent_state.id()));
                return Ok(());
            };
            if parent_state.positioned.as_ref().map(|p| &p.turn) != Some(&placed.value.turn)
                || parent_state.placement.as_ref().map(|p| &p.value) != parent_state.positioned.as_ref()
            {
                effects.pending.push(Dependency::Parent(parent_state.id()));
                return Ok(());
            }
            if super::order::contains(&parent_position.display_ancestry, &id)? {
                return Err(Error::Cycle);
            }
            parent_position.display_ancestry.clone()
        }
    };
    let Some(sibling) = super::order::sibling(state, &placed.value, &path, &neighbors, effects)? else {
        return Ok(());
    };
    path.extend(sibling);
    let new = TimelinePosition {
        turn: turn_order,
        display_ancestry: path,
        accepted: state.display_created,
        item: id.clone(),
    };
    if state.position.as_ref().is_some_and(|old| old != &new) {
        effects.indexes.push(IndexEffect::RepositionChildren(id));
    }
    state.position = Some(new);
    state.positioned = Some(placed.value.clone());
    state.display_parent = display_parent;
    Ok(())
}

pub(super) fn publish(state: &mut ItemState, effects: &mut Effects) -> Result<(), Error> {
    if !state.visible || state.alias.is_some() {
        return Ok(());
    }
    let (Some(body), Some(position), Some(placed)) = (state.body(), state.position.clone(), state.placement.as_ref())
    else {
        return Ok(());
    };
    let Some(located) = state.positioned.as_ref() else {
        return Ok(());
    };
    if located != &placed.value {
        return Ok(());
    }
    let signature = fastctx_types::sync::identity::Digest::of(
        &serde_json::to_vec(&(&position, &located.turn, &located.slot, &state.display_parent, &body))
            .map_err(|e| Error::InvalidContent(e.to_string()))?,
    );
    if state.published.as_ref() == Some(&signature) {
        return Ok(());
    }
    // An unresolved relocation must not publish the new turn with an old position.
    if effects.pending.iter().any(|p| matches!(p, Dependency::Turn(_) | Dependency::Parent(_) | Dependency::Anchor(_)))
    {
        return Ok(());
    }
    state.revision = next(state.revision)?;
    state.published = Some(signature);
    effects.puts.push(PreparedPut {
        id: state.id(),
        revision: state.revision,
        turn: placed.value.turn.clone(),
        parent: state.display_parent.clone(),
        position,
        body,
    });
    Ok(())
}
