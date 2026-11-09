//! Alias routing changes derived reducer inputs, never retained source facts or
//! already signed ItemRevision output. Resolve each addressed item before
//! selecting its keyed state. Relationship evidence keeps its original keys;
//! display parents and reverse indexes resolve those keys separately.

use super::*;
use fastctx_types::sync::conversation_timeline::*;

pub(super) fn redirected(state: &ItemState) -> Option<ItemReduction> {
    Some(ItemReduction {
        state: state.clone(),
        effects: Effects { redirect: Some(state.alias.clone()?), ..Effects::default() },
    })
}

/// Applies one verified alias to the addressed item in a derived input fact.
/// Follow redirects before loading destination indexes. Call keys, parent
/// evidence, native selectors and original source facts remain unchanged.
pub fn redirect_fact(fact: &TimelineFact, source: &ItemState, target: &ItemState) -> Result<TimelineFact, Error> {
    if source.alias.as_ref() != Some(&target.id())
        || target.alias.is_some()
        || source.identity.conversation() != target.identity.conversation()
        || source.identity.role() != target.identity.role()
    {
        return Err(Error::InvalidAlias);
    }
    let from = source.id();
    let to = target.id();
    let replace = |id: &mut fastctx_types::sync::identity::ObjectId| {
        if id == &from {
            *id = to.clone();
        }
    };

    let mut derived = fact.clone();
    match &mut derived {
        TimelineFact::Observed { observation } => {
            if observation.identity.id() == from {
                observation.identity = target.identity.clone();
                observation.created = target.created;
            }
        }
        TimelineFact::Placement { item, .. } | TimelineFact::Run { item, .. } => replace(item),
        TimelineFact::TextAppend { append } => replace(&mut append.item),
        TimelineFact::Image { picture } => replace(&mut picture.item),
        TimelineFact::Branch { change: BranchChange::Membership { item, .. } } => replace(item),

        TimelineFact::ItemRevision { .. } | TimelineFact::Alias { .. } => return Err(Error::InvalidAlias),
        _ => {}
    }
    Ok(derived)
}
