//! An ID-ordered Merkle zipper over two immutable snapshots. Equal references
//! skip a whole subtree; unequal boundaries are expanded independently.
use super::{
    TimelineChange, TimelineChangesResult, TimelineFence,
    pages::{self, Budget, Meter, Outcome, PageSource, Work},
    position, query,
    tree::{self, Entry, Kind, TreeNode},
};
use crate::{
    identity::ObjectId,
    manifest::ManifestChild,
    store::{Error, PageLimit},
};
use fastctx_types::sync::{
    metadata::{TimelineLookupEntry, TimelineRoot},
    transport_codec::bounded_json,
};
use std::{cmp::Ordering, collections::VecDeque};

/// Preparation counters measure followed pages, including snapshot roots and
/// ordered-item lookups. Skipped subtrees require no descendant page reads.
#[derive(Debug)]
pub struct DiffPage {
    pub changes: Vec<TimelineChange>,
    pub next: Option<ObjectId>,
    pub pages: usize,
    pub skipped_subtrees: usize,
}

enum Segment {
    Tree { child: ManifestChild, depth: usize },
    Entry(TimelineLookupEntry),
}
impl Segment {
    fn first(&self) -> String {
        match self {
            Self::Tree { child, .. } => child.first_key.clone(),
            Self::Entry(e) => position::id_key(&e.item),
        }
    }
    fn last(&self) -> String {
        match self {
            Self::Tree { child, .. } => child.last_key.clone(),
            Self::Entry(e) => position::id_key(&e.item),
        }
    }
}
struct Cursor(VecDeque<Segment>);
impl Cursor {
    fn new(root: &TimelineRoot) -> Self {
        Self(root.lookup.iter().cloned().map(|child| Segment::Tree { child, depth: 0 }).collect())
    }
    fn seek(&mut self, after: Option<&str>) {
        while self.0.front().is_some_and(|front| after.is_some_and(|after| front.last().as_str() <= after)) {
            self.0.pop_front();
        }
    }
    fn expand_branch(&mut self, source: &impl PageSource, root: &TimelineRoot, meter: &mut Meter) -> Work<bool> {
        let Some(Segment::Tree { child, .. }) = self.0.front() else {
            return Ok(false);
        };
        let bytes = source
            .manifest(&child.root)?
            .ok_or_else(|| pages::Fault::Missing(crate::manifest::ContentRef::Manifest { root: child.root.clone() }))?;
        meter.page(bytes.len())?;
        let reply = crate::manifest::ManifestReply {
            root: child.root.clone(),
            page_json: std::str::from_utf8(bytes).map_err(|_| pages::corrupt("diff manifest is not UTF-8"))?.into(),
        };
        crate::blocks::manifest::validate(&root.conversation.scope, reply)?;
        match serde_json::from_slice::<crate::manifest::ManifestPage>(bytes)
            .map_err(|_| pages::corrupt("invalid diff manifest"))?
        {
            crate::manifest::ManifestPage::Branch { .. } => {
                self.expand(source, root, meter)?;
                Ok(true)
            }
            crate::manifest::ManifestPage::Metadata {
                codec: fastctx_types::sync::metadata::MetadataCodec::TimelineV1,
                ..
            } => Ok(false),
            _ => Err(pages::corrupt("not a timeline lookup manifest").into()),
        }
    }
    fn expand(&mut self, source: &impl PageSource, root: &TimelineRoot, meter: &mut Meter) -> Work<()> {
        let Some(Segment::Tree { child, depth }) = self.0.pop_front() else {
            return Err(pages::corrupt("diff tried to expand a lookup entry").into());
        };
        if depth >= tree::MAX_DEPTH {
            return Err(pages::corrupt("diff tree exceeds its depth").into());
        }
        match tree::node(source, &child, Kind::Lookup, root, meter)? {
            TreeNode::Branch(children) => {
                for child in children.into_iter().rev() {
                    self.0.push_front(Segment::Tree { child, depth: depth + 1 });
                }
            }
            TreeNode::Leaf(entries) => {
                for entry in entries.into_iter().rev() {
                    let Entry::Lookup(entry) = entry else {
                        return Err(pages::corrupt("diff found a non-lookup entry").into());
                    };
                    self.0.push_front(Segment::Entry(entry));
                }
            }
        }
        Ok(())
    }
}

/// The executor validates both persisted holds before and after this call. A
/// missing page returns Pending with no partial patch, against the same fences.
pub fn page(
    source: &impl PageSource,
    from: &TimelineFence,
    to: &TimelineFence,
    after: Option<&ObjectId>,
    limit: PageLimit,
    budget: Budget,
) -> Result<Outcome<DiffPage>, Error> {
    pages::outcome(read(source, from, to, after, limit, budget))
}
fn read(
    source: &impl PageSource,
    from: &TimelineFence,
    to: &TimelineFence,
    after: Option<&ObjectId>,
    limit: PageLimit,
    budget: Budget,
) -> Work<DiffPage> {
    query::limits(limit)?;
    if from.conversation != to.conversation {
        return Err(Error::Invalid("diff fences name different conversations".into()).into());
    }
    let mut meter = Meter::new(budget);
    let old = query::read_root(source, from, &mut meter)?;
    let new = query::read_root(source, to, &mut meter)?;
    let (mut left, mut right) = (Cursor::new(&old), Cursor::new(&new));
    let after = after.map(position::id_key);
    let mut changes = Vec::new();
    let mut last = None;
    let mut skipped_subtrees = 0;
    let empty = TimelineChangesResult::Page { to: Box::new(to.clone()), changes: Vec::new(), next: None };
    let base = bounded_json(&empty, limit.bytes as usize).map_err(|_| Error::Busy)?.len();
    let mut encoded = 0usize;
    loop {
        left.seek(after.as_deref());
        right.seek(after.as_deref());
        let next = match (left.0.front(), right.0.front()) {
            (None, None) => return Ok(DiffPage { changes, next: None, pages: meter.pages(), skipped_subtrees }),
            (Some(Segment::Tree { child: a, .. }), Some(Segment::Tree { child: b, .. })) if a.root == b.root => {
                if a != b {
                    return Err(pages::corrupt("shared lookup reference has conflicting bounds").into());
                }
                left.0.pop_front();
                right.0.pop_front();
                skipped_subtrees += 1;
                continue;
            }
            (Some(a @ Segment::Tree { .. }), Some(b @ Segment::Tree { .. })) => {
                if let (Segment::Tree { child: a, .. }, Segment::Tree { child: b, .. }) = (a, b)
                    && a.entries == b.entries
                    && a.first_key <= b.last_key
                    && b.first_key <= a.last_key
                {
                    // A one-child wrapper can have the same extent and count as
                    // a shared leaf. Inspect branches without decoding that leaf.
                    if left.expand_branch(source, &old, &mut meter)? || right.expand_branch(source, &new, &mut meter)? {
                        continue;
                    }
                    left.expand(source, &old, &mut meter)?;
                    right.expand(source, &new, &mut meter)?;
                    continue;
                }
                let expand_left = if a.last() < b.first() {
                    true
                } else if b.last() < a.first() {
                    false
                } else {
                    match (a, b) {
                        (Segment::Tree { child: a, .. }, Segment::Tree { child: b, .. }) => a.entries >= b.entries,
                        _ => unreachable!(),
                    }
                };
                if expand_left {
                    left.expand(source, &old, &mut meter)?;
                } else {
                    right.expand(source, &new, &mut meter)?;
                }
                continue;
            }
            (Some(a @ Segment::Tree { .. }), Some(Segment::Entry(b))) if a.first() > position::id_key(&b.item) => {
                let b = b.clone();
                right.0.pop_front();
                put(source, &new, &b, &mut meter)?
            }
            (Some(Segment::Entry(a)), Some(b @ Segment::Tree { .. })) if b.first() > position::id_key(&a.item) => {
                let change = TimelineChange::Retract { item: a.item.clone(), revision: a.revision };
                left.0.pop_front();
                change
            }
            (Some(Segment::Tree { .. }), _) => {
                left.expand(source, &old, &mut meter)?;
                continue;
            }
            (_, Some(Segment::Tree { .. })) => {
                right.expand(source, &new, &mut meter)?;
                continue;
            }
            (Some(Segment::Entry(a)), Some(Segment::Entry(b))) => match a.item.cmp(&b.item) {
                Ordering::Equal => {
                    let (a, b) = (a.clone(), b.clone());
                    left.0.pop_front();
                    right.0.pop_front();
                    if a.revision == b.revision {
                        continue;
                    }
                    put(source, &new, &b, &mut meter)?
                }
                Ordering::Less => {
                    let change = TimelineChange::Retract { item: a.item.clone(), revision: a.revision };
                    left.0.pop_front();
                    change
                }
                Ordering::Greater => {
                    let b = b.clone();
                    right.0.pop_front();
                    put(source, &new, &b, &mut meter)?
                }
            },
            (Some(Segment::Entry(a)), None) => {
                let change = TimelineChange::Retract { item: a.item.clone(), revision: a.revision };
                left.0.pop_front();
                change
            }
            (None, Some(Segment::Entry(b))) => {
                let b = b.clone();
                right.0.pop_front();
                put(source, &new, &b, &mut meter)?
            }
        };
        let id = match &next {
            TimelineChange::Put { item } => item.id.clone(),
            TimelineChange::Retract { item, .. } => item.clone(),
            TimelineChange::SelectBranch { .. } => unreachable!(),
        };
        let bytes = bounded_json(&next, fastctx_types::sync::PAGE_BYTES as usize).map_err(|_| Error::Busy)?.len();
        let total = base
            .checked_add(encoded)
            .and_then(|n| n.checked_add(bytes))
            .and_then(|n| n.checked_add(usize::from(!changes.is_empty())))
            .and_then(|n| n.checked_add(id.as_str().len() + 2 - 4))
            .ok_or(Error::Busy)?;
        if total > limit.bytes as usize {
            if changes.is_empty() {
                return Err(Error::Busy.into());
            }
            return Ok(DiffPage { changes, next: last, pages: meter.pages(), skipped_subtrees });
        }
        encoded = encoded.checked_add(bytes + usize::from(!changes.is_empty())).ok_or(Error::Busy)?;
        last = Some(id);
        changes.push(next);
        if changes.len() == limit.items as usize {
            return Ok(DiffPage { changes, next: last, pages: meter.pages(), skipped_subtrees });
        }
    }
}
fn put(
    source: &impl PageSource,
    root: &TimelineRoot,
    location: &TimelineLookupEntry,
    meter: &mut Meter,
) -> Work<TimelineChange> {
    match tree::find(source, root.items.as_ref(), Kind::Items, root, &position::key(&location.position)?, meter)? {
        Some(Entry::Item(item, _)) if item.item.id == location.item && item.item.revision == location.revision => {
            Ok(TimelineChange::Put { item: item.item })
        }
        _ => Err(pages::corrupt("diff lookup differs from its ordered tree").into()),
    }
}
