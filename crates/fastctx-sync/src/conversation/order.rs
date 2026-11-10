//! Fixed-width sibling frames preserve parent prefixes without treating order
//! words (timestamps, ordinals or hashes) as ancestor identities.
use super::*;
use crate::{cursor::Sequence, identity::ObjectId};
use fastctx_types::sync::conversation_timeline::*;

const WORDS: usize = 17;
const NODE: usize = WORDS - 4;

fn identity(id: &ObjectId) -> [Sequence; 4] {
    let text = &id.as_str()[2..];
    std::array::from_fn(|n| Sequence(u64::from_str_radix(&text[n * 16..(n + 1) * 16], 16).expect("validated identity")))
}

pub(super) fn contains(path: &[Sequence], id: &ObjectId) -> Result<bool, Error> {
    let frames = path.get(1..).ok_or(Error::InvalidPlacement)?;
    if frames.len() % WORDS != 0 {
        return Err(Error::InvalidPlacement);
    }
    Ok(frames.chunks_exact(WORDS).any(|frame| frame[NODE..] == identity(id)))
}

pub(super) fn sibling(
    state: &ItemState,
    placed: &Placement,
    prefix: &[Sequence],
    neighbors: &Neighbors<'_>,
    effects: &mut Effects,
) -> Result<Option<Vec<Sequence>>, Error> {
    let id = state.id();
    let mut words = [Sequence(0); WORDS];
    words[NODE..].copy_from_slice(&identity(&id));
    let Some(DisplayOrder::Gap { before, at, sequence, identity: stable }) = &placed.order else {
        let ordinal = match placed.order {
            Some(DisplayOrder::Native { sequence }) => sequence,
            _ => state.display_created,
        };
        if ordinal.0 == 0 {
            return Err(Error::InvalidPlacement);
        }
        words[0] = ordinal;
        words[1..5].copy_from_slice(&identity(&id));
        words[5] = Sequence(1);
        return Ok(Some(words.to_vec()));
    };
    if sequence.0 == 0 || matches!(placed.slot, DisplaySlot::Opening) {
        return Err(Error::InvalidPlacement);
    }
    if let Some(before) = before {
        if before == &id {
            return Err(Error::Cycle);
        }
        let Some(original) = neighbors.anchor else {
            effects.pending.push(Dependency::Anchor(before.clone()));
            return Ok(None);
        };
        if &original.id() != before {
            return Err(Error::WrongKey);
        }
        let anchor = if let Some(target) = &original.alias {
            let Some(resolved) = neighbors.resolved_anchor else {
                effects.pending.push(Dependency::Anchor(target.clone()));
                return Ok(None);
            };
            if &resolved.id() != target || resolved.alias.is_some() {
                return Err(Error::InvalidAlias);
            }
            resolved
        } else {
            original
        };
        if anchor.id() == id {
            return Err(Error::Cycle);
        }
        if anchor.identity.conversation() != state.identity.conversation() {
            return Err(Error::WrongConversation);
        }
        if anchor.placement.as_ref().is_some_and(|p| matches!(p.value.order, Some(DisplayOrder::Gap { .. }))) {
            return Err(Error::InvalidPlacement);
        }
        let Some(position) = anchor
            .position
            .as_ref()
            .filter(|_| anchor.placement.as_ref().map(|p| &p.value) == anchor.positioned.as_ref())
        else {
            effects.pending.push(Dependency::Anchor(anchor.id()));
            return Ok(None);
        };
        if anchor.positioned.as_ref().is_none_or(|p| p.turn != placed.turn)
            || position.display_ancestry.len() != prefix.len() + WORDS
            || !position.display_ancestry.starts_with(prefix)
        {
            effects.pending.push(Dependency::Anchor(anchor.id()));
            return Ok(None);
        }
        // The native pivot is unchanged by every insertion into its preceding gap.
        words[..5].copy_from_slice(&position.display_ancestry[prefix.len()..prefix.len() + 5]);
    } else {
        words[..5].fill(Sequence(u64::MAX));
        words[5] = Sequence(2);
    }
    let time = (at.as_nanosecond() as u128) ^ (1_u128 << 127);
    words[6] = Sequence((time >> 64) as u64);
    words[7] = Sequence(time as u64);
    words[8] = *sequence;
    words[9..13].copy_from_slice(&identity(stable));
    Ok(Some(words.to_vec()))
}
