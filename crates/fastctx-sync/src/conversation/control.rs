use super::*;
use fastctx_types::sync::{conversation_timeline::*, identity::ObjectId, timeline::TimelineChange};
use serde::{Deserialize, Serialize};

pub fn turn(
    old: Option<&TurnState>,
    declaration: &TurnDeclaration,
    evidence: &Evidence,
) -> Result<(TurnState, Effects), Error> {
    if declaration.order.0 == 0 {
        return Err(Error::InvalidPlacement);
    }
    if let Some(old) = old {
        if old.declaration != *declaration {
            return Err(Error::CreationChanged);
        }
        return Ok((old.clone(), Effects::default()));
    }
    let mut effects = Effects::default();
    effects.indexes.push(IndexEffect::TurnReady(declaration.key.id()));
    Ok((TurnState { declaration: declaration.clone(), evidence: evidence.clone() }, effects))
}

pub fn picture(
    old: Option<&Versioned<PictureLink>>,
    link: &PictureLink,
    evidence: &Evidence,
) -> Result<(Versioned<PictureLink>, Effects), Error> {
    if let PictureTarget::Owned { content } = &link.target {
        content.validate().map_err(Error::InvalidContent)?;
    }
    let value = Versioned { evidence: evidence.clone(), value: link.clone() };
    if let Some(old) = old {
        if old.value.item != link.item || old.value.selector != link.selector {
            return Err(Error::WrongKey);
        }
        if !old.accepts(&value)? {
            return Ok((old.clone(), Effects::default()));
        }
    }
    let mut effects = Effects::default();
    effects.indexes.push(IndexEffect::Picture(Box::new(value.clone())));
    Ok((value, effects))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentState {
    pub parent: fastctx_types::sync::identity::ObjectKey,
    pub child: fastctx_types::sync::identity::ObjectKey,
    pub call: Option<Versioned<ObjectId>>,
    pub name: Option<Versioned<fastctx_types::sync::conversation_identity::NativeId>>,
    pub status: Option<Versioned<fastctx_types::sync::conversation_identity::NativeId>>,
}
fn field<T: Clone + PartialEq>(
    old: &mut Option<Versioned<T>>,
    value: &Option<T>,
    evidence: &Evidence,
) -> Result<(), Error> {
    let Some(value) = value else {
        return Ok(());
    };
    let incoming = Versioned { evidence: evidence.clone(), value: value.clone() };
    if old.as_ref().map(|v| v.accepts(&incoming)).transpose()?.unwrap_or(true) {
        *old = Some(incoming);
    }
    Ok(())
}
/// `ancestors` is the keyed parent chain already resolved by the caller. A
/// roster with absent fields cannot clear a child or replace a tree with empty.
pub fn agent(
    old: Option<&AgentState>,
    link: &AgentLink,
    evidence: &Evidence,
    ancestors: &[ObjectId],
) -> Result<(AgentState, Effects), Error> {
    if link.parent == link.child || ancestors.contains(&link.child.object) {
        return Err(Error::Cycle);
    }
    let mut state = old.cloned().unwrap_or_else(|| AgentState {
        parent: link.parent.clone(),
        child: link.child.clone(),
        call: None,
        name: None,
        status: None,
    });
    if state.parent != link.parent || state.child != link.child {
        return Err(Error::WrongKey);
    }
    field(&mut state.call, &link.call, evidence)?;
    field(&mut state.name, &link.name, evidence)?;
    field(&mut state.status, &link.status, evidence)?;
    let mut effects = Effects::default();
    if old != Some(&state) {
        effects.indexes.push(IndexEffect::Agent(Box::new(state.clone())));
    }
    Ok((state, effects))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceState {
    pub source: fastctx_types::sync::identity::SourceId,
    pub current: fastctx_types::sync::identity::HostGeneration,
    pub last: Option<Versioned<HostRewritten>>,
}
#[derive(Clone, Debug)]
pub struct RewriteReduction {
    pub state: SourceState,
    pub effects: Effects,
}

/// Host truncation, replacement and cleanup are not rollback authority. This
/// changes only local generation/branch evidence; it emits no item retractions.
pub fn rewrite(old: &SourceState, change: &HostRewritten, evidence: &Evidence) -> Result<RewriteReduction, Error> {
    if old.source != change.source || change.generation == change.previous {
        return Err(Error::InvalidGeneration);
    }
    let value = Versioned { evidence: evidence.clone(), value: change.clone() };
    if let Some(last) = &old.last
        && !last.accepts(&value)?
    {
        return Ok(RewriteReduction { state: old.clone(), effects: Effects::default() });
    }
    if old.current != change.previous {
        let mut effects = Effects::default();
        effects
            .pending
            .push(Dependency::Generation { source: change.source.clone(), previous: change.previous.clone() });
        return Ok(RewriteReduction { state: old.clone(), effects });
    }
    Ok(RewriteReduction {
        state: SourceState { source: old.source.clone(), current: change.generation.clone(), last: Some(value) },
        effects: Effects::default(),
    })
}

#[derive(Clone, Debug)]
pub struct BranchReduction {
    pub state: Versioned<BranchChange>,
    pub item: Option<ItemState>,
    pub effects: Effects,
}
/// Membership rows are keyed by (branch,item), selection is a separate row.
/// Changes to an inactive branch retain evidence without mutating the live view.
pub fn branch(
    old: Option<&Versioned<BranchChange>>,
    item: Option<&ItemState>,
    active: &ObjectId,
    change: &BranchChange,
    evidence: &Evidence,
) -> Result<BranchReduction, Error> {
    let value = Versioned { evidence: evidence.clone(), value: change.clone() };
    if let Some(old) = old {
        let same_key = match (&old.value, change) {
            (BranchChange::Select { .. }, BranchChange::Select { .. }) => true,
            (
                BranchChange::Membership { item: a, branch: b, .. },
                BranchChange::Membership { item: c, branch: d, .. },
            ) => a == c && b == d,
            _ => false,
        };
        if !same_key {
            return Err(Error::WrongKey);
        }
        if !old.accepts(&value)? && (old != &value || matches!(change, BranchChange::Select { .. })) {
            return Ok(BranchReduction { state: old.clone(), item: item.cloned(), effects: Effects::default() });
        }
    }
    let mut effects = Effects::default();
    let mut result = item.cloned();
    match change {
        BranchChange::Select { root, projection, .. } => {
            effects.changes.push(TimelineChange::SelectBranch { root: root.clone(), projection: *projection })
        }
        BranchChange::Membership { item: id, branch, present, rollback } => {
            if !present && rollback.is_none() {
                return Err(Error::MissingRollbackReceipt);
            }
            if branch == active {
                if let Some(state) = &mut result {
                    if &state.id() != id {
                        return Err(Error::WrongKey);
                    }
                    if !present && state.visible && state.published.is_some() {
                        state.revision = next(state.revision)?;
                        effects.changes.push(TimelineChange::Retract { item: id.clone(), revision: state.revision });
                    }
                    state.visible = *present;
                    if !present {
                        state.published = None;
                    }
                    if *present {
                        placement::publish(state, &mut effects)?;
                    }
                } else {
                    effects.pending.push(Dependency::Item(id.clone()));
                }
            }
        }
    }
    Ok(BranchReduction { state: value, item: result, effects })
}
