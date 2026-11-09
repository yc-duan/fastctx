use crate::{
    identity::ScopeKey,
    manifest::{BlobId, ContentRef},
};
use fastctx_types::sync::{
    conversation_timeline::{BranchChange, PictureTarget, TimelineFact},
    timeline::TimelineChange,
};
use std::collections::BTreeMap;

/// Direct ownership only. Typed metadata pages own their transitive dependencies;
/// logical object, record and alias references never grant content custody.
pub fn direct_roots(scope: &ScopeKey, fact: &TimelineFact) -> Result<Vec<ContentRef>, String> {
    let roots = match fact {
        TimelineFact::Observed { observation } => {
            vec![observation.body.original.clone(), observation.body.display.clone()]
        }
        TimelineFact::ItemRevision { change: TimelineChange::Put { item } } => {
            item.display.validate()?;
            item.display.validate_scopes(scope)?;
            let mut roots = vec![item.body.clone()];
            roots.extend(item.display.owned_content());
            roots
        }
        TimelineFact::ItemRevision { change: TimelineChange::SelectBranch { root, .. } }
        | TimelineFact::Branch { change: BranchChange::Select { root, .. } } => {
            vec![ContentRef::Manifest { root: root.clone() }]
        }
        TimelineFact::TextAppend { append } => vec![append.chunk.clone()],
        TimelineFact::Run { run, .. } => run.summary.iter().cloned().collect(),
        TimelineFact::Image { picture } => match &picture.target {
            PictureTarget::Owned { content } => vec![content.clone()],
            PictureTarget::Object { .. } => Vec::new(),
        },
        TimelineFact::ItemRevision { change: TimelineChange::Retract { .. } }
        | TimelineFact::HostRewritten { .. }
        | TimelineFact::Branch { change: BranchChange::Membership { .. } }
        | TimelineFact::Alias { .. }
        | TimelineFact::Turn { .. }
        | TimelineFact::Placement { .. }
        | TimelineFact::Agent { .. } => Vec::new(),
    };
    Ok(root_set(scope, &roots)?.into_values().collect())
}

pub(super) fn root_set(scope: &ScopeKey, roots: &[ContentRef]) -> Result<BTreeMap<BlobId, ContentRef>, String> {
    let mut result = BTreeMap::new();
    for root in roots {
        root.validate()?;
        if &root.id().domain != scope {
            return Err("timeline content crosses its source scope".into());
        }
        if let Some(old) = result.insert(root.id().clone(), root.clone())
            && old != *root
        {
            return Err("timeline references disagree on a content shape or length".into());
        }
    }
    if result.len() > fastctx_types::sync::RECORD_ROOTS {
        return Err("timeline event exceeds its direct root bound".into());
    }
    Ok(result)
}
