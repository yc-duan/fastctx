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
    let Some(turn) = neighbors.turn else {
        effects.pending.push(Dependency::Turn(placed.value.turn.clone()));
        return Ok(());
    };
    if turn.declaration.key.id() != placed.value.turn {
        return Err(Error::WrongKey);
    }
    if &turn.declaration.key.conversation != state.identity.conversation() {
        return Err(Error::WrongConversation);
    }
    let mut display_parent = None;
    let path = match &placed.value.slot {
        DisplaySlot::Opening => {
            if state.identity.role() != ItemRole::User {
                return Err(Error::InvalidPlacement);
            }
            vec![Sequence(0), state.display_created]
        }
        DisplaySlot::Sibling => vec![Sequence(1), state.display_created],
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
                || parent_state.placement.as_ref().map(|p| (&p.value.turn, &p.value.slot))
                    != parent_state.positioned.as_ref().map(|p| (&p.turn, &p.slot))
            {
                effects.pending.push(Dependency::Parent(parent_state.id()));
                return Ok(());
            }
            // Creation keys are unique within a canonical producer. The leading
            // 0/1 is a reserved slot, so only the remaining keys can be ancestors.
            if parent_position.display_ancestry.iter().skip(1).any(|key| key == &state.display_created) {
                return Err(Error::Cycle);
            }
            let mut path = parent_position.display_ancestry.clone();
            path.push(state.display_created);
            path
        }
    };
    let new = TimelinePosition {
        turn: turn.declaration.order,
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
    if located.turn != placed.value.turn || located.slot != placed.value.slot {
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
    if effects.pending.iter().any(|p| matches!(p, Dependency::Turn(_) | Dependency::Parent(_))) {
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
