//! Copy only the path whose key changed. Both indexes use the same bounded tree
//! mechanics; lookup leaves never duplicate display bodies.
use super::{
    pages::{self, EncodedPage, Meter, Node, PageSource, Work},
    position,
};
use crate::{
    cursor::Count,
    identity::ObjectId,
    manifest::{BlobId, ContentBlock, ContentRef, ManifestChild, ManifestPage, ManifestRoot},
    store::Error,
};
use fastctx_types::sync::metadata::{
    IndexedTimelineItem, ProofReference, TimelineLeaf, TimelineLookupEntry, TimelineLookupLeaf, TimelinePayload,
    TimelineRoot,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const MAX_DEPTH: usize = 64;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Items,
    Lookup,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Entry {
    Item(Box<IndexedTimelineItem>, ProofReference),
    Lookup(TimelineLookupEntry),
}
impl Entry {
    pub fn key(&self) -> Result<String, Error> {
        match self {
            Self::Item(e, _) => position::key(&e.item.position),
            Self::Lookup(e) => Ok(position::id_key(&e.item)),
        }
    }
}
pub(super) enum TreeNode {
    Branch(Vec<ManifestChild>),
    Leaf(Vec<Entry>),
}
pub(super) fn snapshot(source: &impl PageSource, root: &ManifestRoot, meter: &mut Meter) -> Work<TimelineRoot> {
    match pages::load(source, root, meter)? {
        Node::Payload(TimelinePayload::Root(root)) => Ok(*root),
        _ => Err(pages::corrupt("timeline snapshot has no Root payload").into()),
    }
}
pub(super) fn node(
    source: &impl PageSource,
    child: &ManifestChild,
    kind: Kind,
    root: &TimelineRoot,
    meter: &mut Meter,
) -> Work<TreeNode> {
    if child.root.0.domain != root.conversation.scope {
        return Err(pages::corrupt("tree child crosses its conversation scope").into());
    }
    let node = match pages::load(source, &child.root, meter)? {
        Node::Branch(children) => TreeNode::Branch(children),
        Node::Payload(TimelinePayload::Items(leaf)) if kind == Kind::Items => {
            if leaf.conversation != root.conversation
                || leaf.projection > root.projection
                || (leaf.through.stream == root.through.stream
                    && leaf.through.epoch == root.through.epoch
                    && leaf.through.committed > root.through.committed)
            {
                return Err(pages::corrupt("item leaf is outside its snapshot").into());
            }
            let proofs: BTreeMap<_, _> = leaf.proofs.into_iter().map(|p| (p.id.clone(), p)).collect();
            TreeNode::Leaf(
                leaf.items
                    .into_iter()
                    .map(|e| {
                        let proof = proofs.get(&e.batch).expect("metadata checked proof").clone();
                        Entry::Item(Box::new(e), proof)
                    })
                    .collect(),
            )
        }
        Node::Payload(TimelinePayload::ItemLookup(leaf)) if kind == Kind::Lookup => {
            if leaf.conversation != root.conversation {
                return Err(pages::corrupt("lookup leaf belongs to another conversation").into());
            }
            TreeNode::Leaf(leaf.entries.into_iter().map(Entry::Lookup).collect())
        }
        _ => return Err(pages::corrupt("wrong timeline tree leaf kind").into()),
    };
    let (first, last, count) = match &node {
        TreeNode::Branch(children) => (
            children[0].first_key.clone(),
            children.last().expect("nonempty branch").last_key.clone(),
            total(children)?,
        ),
        TreeNode::Leaf(entries) => {
            let Some(first) = entries.first() else { return Err(pages::corrupt("empty indexed leaf").into()) };
            (first.key()?, entries.last().expect("nonempty").key()?, entries.len() as u64)
        }
    };
    if first != child.first_key || last != child.last_key || count != child.entries.0 {
        return Err(pages::corrupt("child range or count differs from its parent").into());
    }
    Ok(node)
}
pub(super) fn find(
    source: &impl PageSource,
    child: Option<&ManifestChild>,
    kind: Kind,
    root: &TimelineRoot,
    key: &str,
    meter: &mut Meter,
) -> Work<Option<Entry>> {
    let Some(mut child) = child.cloned() else { return Ok(None) };
    for _ in 0..MAX_DEPTH {
        if key < child.first_key.as_str() || key > child.last_key.as_str() {
            return Ok(None);
        }
        match node(source, &child, kind, root, meter)? {
            TreeNode::Branch(children) => {
                let i = children.partition_point(|c| c.last_key.as_str() < key);
                let Some(next) = children.get(i) else { return Ok(None) };
                child = next.clone();
            }
            TreeNode::Leaf(entries) => {
                for entry in entries {
                    let k = entry.key()?;
                    if k == key {
                        return Ok(Some(entry));
                    }
                    if k.as_str() > key {
                        break;
                    }
                }
                return Ok(None);
            }
        }
    }
    Err(pages::corrupt("timeline tree exceeds its traversal depth").into())
}
pub(super) struct Overlay<'a, S> {
    pub source: &'a S,
    generated: BTreeMap<ManifestRoot, EncodedPage>,
    payloads: BTreeMap<BlobId, ManifestRoot>,
}
impl<'a, S: PageSource> Overlay<'a, S> {
    pub fn new(source: &'a S) -> Self {
        Self { source, generated: BTreeMap::new(), payloads: BTreeMap::new() }
    }
    pub fn add(&mut self, page: EncodedPage, meter: &mut Meter) -> Result<ManifestRoot, Error> {
        let root = page.manifest.root.clone();
        if !self.generated.contains_key(&root) {
            meter.page(page.manifest.page_json.len() + page.payload.as_ref().map_or(0, |(_, b)| b.len()))?;
            if let Some((block, _)) = &page.payload {
                self.payloads.insert(block.id.clone(), root.clone());
            }
            self.generated.insert(root.clone(), page);
        }
        Ok(root)
    }
    pub fn finish(mut self, root: &ManifestRoot) -> Result<Vec<EncodedPage>, Error> {
        let mut stack = vec![root.clone()];
        let mut used = BTreeSet::new();
        while let Some(root) = stack.pop() {
            if !used.insert(root.clone()) {
                continue;
            }
            let Some(page) = self.generated.get(&root) else { continue };
            let manifest: ManifestPage = serde_json::from_str(&page.manifest.page_json)
                .map_err(|_| pages::corrupt("generated manifest is invalid"))?;
            let refs = match manifest {
                ManifestPage::Branch { children } => children.into_iter().map(|c| c.root).collect::<Vec<_>>(),
                ManifestPage::Metadata { references, .. } => references
                    .into_iter()
                    .filter_map(|r| match r {
                        ContentRef::Manifest { root } => Some(root),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            stack.extend(refs.into_iter().filter(|r| self.generated.contains_key(r)));
        }
        Ok(used.into_iter().filter_map(|r| self.generated.remove(&r)).collect())
    }
    pub fn replace(
        &mut self,
        child: Option<&ManifestChild>,
        kind: Kind,
        context: &TimelineRoot,
        key: &str,
        value: Option<Entry>,
        meter: &mut Meter,
    ) -> Work<Option<ManifestChild>> {
        let mut roots = self.path(child, kind, context, key, value, meter, 0)?;
        while roots.len() > 1 {
            roots = self.branches(&context.conversation.scope, roots, meter)?;
        }
        Ok(roots.pop())
    }
    #[allow(clippy::too_many_arguments)]
    fn path(
        &mut self,
        child: Option<&ManifestChild>,
        kind: Kind,
        context: &TimelineRoot,
        key: &str,
        value: Option<Entry>,
        meter: &mut Meter,
        depth: usize,
    ) -> Work<Vec<ManifestChild>> {
        if depth >= MAX_DEPTH {
            return Err(pages::corrupt("timeline tree exceeds its traversal depth").into());
        }
        let old = match child {
            Some(c) => node(self, c, kind, context, meter)?,
            None => TreeNode::Leaf(Vec::new()),
        };
        match old {
            TreeNode::Leaf(mut entries) => {
                let mut at = entries.len();
                for (i, entry) in entries.iter().enumerate() {
                    if entry.key()?.as_str() >= key {
                        at = i;
                        break;
                    }
                }
                let exists = at < entries.len() && entries[at].key()? == key;
                if (exists && value.as_ref() == Some(&entries[at])) || (!exists && value.is_none()) {
                    return Ok(child.cloned().into_iter().collect());
                }
                if exists {
                    entries.remove(at);
                }
                if let Some(value) = value {
                    if value.key()? != key {
                        return Err(Error::Invalid("replacement key mismatch".into()).into());
                    }
                    entries.insert(at, value);
                }
                self.leaves(kind, context, entries, meter)
            }
            TreeNode::Branch(mut children) => {
                let at = children.partition_point(|c| c.last_key.as_str() < key).min(children.len() - 1);
                let changed = self.path(Some(&children[at]), kind, context, key, value, meter, depth + 1)?;
                if changed.as_slice() == std::slice::from_ref(&children[at]) {
                    return Ok(child.cloned().into_iter().collect());
                }
                children.splice(at..at + 1, changed);
                self.branches(&context.conversation.scope, children, meter)
            }
        }
    }
    fn leaves(
        &mut self,
        kind: Kind,
        context: &TimelineRoot,
        entries: Vec<Entry>,
        meter: &mut Meter,
    ) -> Work<Vec<ManifestChild>> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < entries.len() {
            let remaining = &entries[at..];
            let (count, page) = largest(remaining.len().min(256), |n| {
                pages::encode_payload(&context.conversation.scope, &leaf(kind, context, &remaining[..n])?)
            })?;
            let root = self.add(page, meter)?;
            out.push(ManifestChild {
                first_key: remaining[0].key()?,
                last_key: remaining[count - 1].key()?,
                root,
                entries: Count(count as u64),
            });
            at += count;
        }
        Ok(out)
    }
    fn branches(
        &mut self,
        scope: &crate::identity::ScopeKey,
        children: Vec<ManifestChild>,
        meter: &mut Meter,
    ) -> Work<Vec<ManifestChild>> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < children.len() {
            let remaining = &children[at..];
            let (count, page) =
                largest(remaining.len().min(256), |n| pages::encode_branch(scope, remaining[..n].to_vec()))?;
            if count == 1 && remaining.len() > 1 {
                return Err(Error::Invalid("timeline branch cannot fit two child ranges".into()).into());
            }
            let root = self.add(page, meter)?;
            out.push(ManifestChild {
                first_key: remaining[0].first_key.clone(),
                last_key: remaining[count - 1].last_key.clone(),
                root,
                entries: Count(total(&remaining[..count])?),
            });
            at += count;
        }
        Ok(out)
    }
}
impl<S: PageSource> PageSource for Overlay<'_, S> {
    fn manifest(&self, root: &ManifestRoot) -> Result<Option<&[u8]>, Error> {
        match self.generated.get(root) {
            Some(p) => Ok(Some(p.manifest.page_json.as_bytes())),
            None => self.source.manifest(root),
        }
    }
    fn payload(&self, block: &ContentBlock) -> Result<Option<&[u8]>, Error> {
        if let Some(root) = self.payloads.get(&block.id) {
            let (block_found, bytes) = self.generated[root].payload.as_ref().expect("generated payload");
            if block_found != block {
                return Err(pages::corrupt("payload length mismatch"));
            }
            Ok(Some(bytes))
        } else {
            self.source.payload(block)
        }
    }
}
fn largest(
    max: usize,
    mut build: impl FnMut(usize) -> Result<Option<EncodedPage>, Error>,
) -> Result<(usize, EncodedPage), Error> {
    let mut low = 1;
    let mut high = max;
    let mut best = None;
    while low <= high {
        let mid = low + (high - low) / 2;
        match build(mid)? {
            Some(page) => {
                best = Some((mid, page));
                low = mid + 1
            }
            None => high = mid - 1,
        }
    }
    best.ok_or_else(|| Error::Invalid("one timeline entry exceeds the final page budget".into()))
}
fn leaf(kind: Kind, root: &TimelineRoot, entries: &[Entry]) -> Result<TimelinePayload, Error> {
    match kind {
        Kind::Items => {
            let mut items = Vec::new();
            let mut proofs = BTreeMap::new();
            for entry in entries {
                let Entry::Item(item, proof) = entry else { return Err(pages::corrupt("lookup in item tree")) };
                items.push(*item.clone());
                if let Some(old) = proofs.insert(proof.id.clone(), proof.clone())
                    && old != *proof
                {
                    return Err(Error::Invalid("same proof identity has different payloads".into()));
                }
            }
            Ok(TimelinePayload::Items(Box::new(TimelineLeaf {
                conversation: root.conversation.clone(),
                projection: root.projection,
                through: root.through.clone(),
                items,
                proofs: proofs.into_values().collect(),
            })))
        }
        Kind::Lookup => {
            let mut values = Vec::new();
            for entry in entries {
                let Entry::Lookup(value) = entry else { return Err(pages::corrupt("item in lookup tree")) };
                values.push(value.clone())
            }
            Ok(TimelinePayload::ItemLookup(TimelineLookupLeaf {
                conversation: root.conversation.clone(),
                entries: values,
            }))
        }
    }
}
fn total(children: &[ManifestChild]) -> Result<u64, Error> {
    children
        .iter()
        .try_fold(0u64, |n, c| n.checked_add(c.entries.0).ok_or_else(|| pages::corrupt("timeline count overflow")))
}
pub(super) fn item(
    source: &impl PageSource,
    root: &TimelineRoot,
    id: &ObjectId,
    meter: &mut Meter,
) -> Work<Option<Entry>> {
    let Some(Entry::Lookup(location)) =
        find(source, root.lookup.as_ref(), Kind::Lookup, root, &position::id_key(id), meter)?
    else {
        return Ok(None);
    };
    let entry = find(source, root.items.as_ref(), Kind::Items, root, &position::key(&location.position)?, meter)?;
    match &entry {
        Some(Entry::Item(value, _)) if value.item.id == *id && value.item.revision == location.revision => Ok(entry),
        _ => Err(pages::corrupt("timeline lookup differs from its ordered tree").into()),
    }
}
