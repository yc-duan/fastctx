//! Large shared trees retain their own page identities across boundary changes.
use super::*;
use fastctx_sync::{
    manifest::{ContentBlock, ManifestChild},
    timeline::{
        changes,
        pages::{Budget, Outcome, PageSource},
        position,
    },
};
use fastctx_types::sync::{
    cursor::Count,
    metadata::{
        IndexedTimelineItem, MetadataCodec, ProofReference, TimelineLeaf, TimelineLookupEntry, TimelineLookupLeaf,
        TimelinePayload, TimelineRoot,
    },
};
use std::{cell::RefCell, collections::BTreeMap};

fn position_key(value: &TimelinePosition) -> Result<String> {
    Ok(position::encode(value)?.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn id_key(id: &ObjectId) -> String {
    id.as_str().strip_prefix("o-").unwrap().to_owned()
}

#[derive(Clone, Default)]
struct Memory {
    manifests: BTreeMap<ManifestRoot, Vec<u8>>,
    payloads: BTreeMap<BlobId, Vec<u8>>,
    reads: RefCell<BTreeMap<ManifestRoot, usize>>,
}
impl PageSource for Memory {
    fn manifest(&self, root: &ManifestRoot) -> std::result::Result<Option<&[u8]>, fastctx_sync::store::Error> {
        *self.reads.borrow_mut().entry(root.clone()).or_default() += 1;
        Ok(self.manifests.get(root).map(Vec::as_slice))
    }
    fn payload(&self, block: &ContentBlock) -> std::result::Result<Option<&[u8]>, fastctx_sync::store::Error> {
        Ok(self.payloads.get(&block.id).map(Vec::as_slice))
    }
}
impl Memory {
    fn page(&mut self, scope: &ScopeKey, value: ManifestPage) -> Result<ManifestRoot> {
        let bytes = serde_json::to_vec(&value)?;
        let root = ManifestRoot(BlobId {
            domain: scope.clone(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(&bytes),
        });
        fastctx_sync::blocks::manifest::validate(
            scope,
            ManifestReply { root: root.clone(), page_json: String::from_utf8(bytes.clone())? },
        )?;
        self.manifests.insert(root.clone(), bytes);
        Ok(root)
    }
    fn metadata(&mut self, scope: &ScopeKey, value: TimelinePayload) -> Result<ManifestRoot> {
        let mut references = BTreeMap::new();
        let mut add = |content: ContentRef| {
            references.insert((content.id().clone(), matches!(content, ContentRef::Manifest { .. })), content);
        };
        match &value {
            TimelinePayload::Root(root) => {
                for child in root.items.iter().chain(root.lookup.iter()) {
                    add(ContentRef::Manifest { root: child.root.clone() });
                }
            }
            TimelinePayload::Items(leaf) => {
                for proof in &leaf.proofs {
                    add(ContentRef::from(proof.payload.clone()));
                }
                for entry in &leaf.items {
                    add(ContentRef::from(entry.payload.clone()));
                    add(entry.item.body.clone());
                    for content in entry.item.display.owned_content() {
                        add(content);
                    }
                }
            }
            TimelinePayload::ItemLookup(_) => {}
        }
        let bytes = serde_json::to_vec(&value)?;
        let payload = ContentBlock {
            id: BlobId { domain: scope.clone(), algorithm: DigestAlgorithm::Sha256, digest: Digest::of(&bytes) },
            decoded_bytes: bytes.len() as u32,
        };
        self.payloads.insert(payload.id.clone(), bytes);
        self.page(
            scope,
            ManifestPage::Metadata {
                codec: MetadataCodec::TimelineV1,
                payload,
                references: references.into_values().collect(),
            },
        )
    }
    fn branch(&mut self, scope: &ScopeKey, children: Vec<ManifestChild>) -> Result<ManifestChild> {
        let child = ManifestChild {
            first_key: children[0].first_key.clone(),
            last_key: children.last().unwrap().last_key.clone(),
            entries: Count(children.iter().map(|c| c.entries.0).sum()),
            root: self.page(scope, ManifestPage::Branch { children })?,
        };
        Ok(child)
    }
    fn leaves(&mut self, fence: &TimelineFence, items: &[TimelineItem]) -> Result<(ManifestChild, ManifestChild)> {
        let scope = &fence.conversation.scope;
        let proof = ProofReference {
            id: Digest::of(b"source batch"),
            payload: ContentBlock {
                id: BlobId { domain: scope.clone(), algorithm: DigestAlgorithm::Sha256, digest: Digest::of(b"proof") },
                decoded_bytes: 5,
            },
        };
        let indexed = items
            .iter()
            .map(|item| IndexedTimelineItem {
                item: item.clone(),
                record: RecordRef {
                    stream: fence.through.cursor().stream.clone(),
                    epoch: fence.through.cursor().epoch.clone(),
                    sequence: Sequence(1),
                },
                payload: proof.payload.clone(),
                batch: proof.id.clone(),
            })
            .collect();
        let root = self.metadata(
            scope,
            TimelinePayload::Items(Box::new(TimelineLeaf {
                conversation: fence.conversation.clone(),
                projection: fence.projection,
                through: fence.through.cursor().clone(),
                items: indexed,
                proofs: vec![proof],
            })),
        )?;
        let ordered = ManifestChild {
            first_key: position_key(&items[0].position)?,
            last_key: position_key(&items.last().unwrap().position)?,
            entries: Count(items.len() as u64),
            root,
        };
        let entries = items
            .iter()
            .map(|item| TimelineLookupEntry {
                item: item.id.clone(),
                revision: item.revision,
                position: item.position.clone(),
            })
            .collect();
        let root = self.metadata(
            scope,
            TimelinePayload::ItemLookup(TimelineLookupLeaf { conversation: fence.conversation.clone(), entries }),
        )?;
        let lookup = ManifestChild {
            first_key: id_key(&items[0].id),
            last_key: id_key(&items.last().unwrap().id),
            entries: Count(items.len() as u64),
            root,
        };
        Ok((ordered, lookup))
    }
    fn snapshot(
        &mut self,
        fence: &TimelineFence,
        ordered: ManifestChild,
        lookup: ManifestChild,
    ) -> Result<TimelineFence> {
        let root = self.metadata(
            &fence.conversation.scope,
            TimelinePayload::Root(Box::new(TimelineRoot {
                conversation: fence.conversation.clone(),
                projection: fence.projection,
                through: fence.through.cursor().clone(),
                items: Some(ordered),
                lookup: Some(lookup),
            })),
        )?;
        Ok(TimelineFence { root, ..fence.clone() })
    }
}
fn item(template: &TimelineItem, number: u64) -> TimelineItem {
    let mut seed = [0; 32];
    seed[24..].copy_from_slice(&number.to_be_bytes());
    let id = ObjectId::from_bytes(seed);
    TimelineItem {
        id: id.clone(),
        position: TimelinePosition {
            turn: Sequence(1),
            display_ancestry: vec![Sequence(number + 1)],
            accepted: Sequence(1),
            item: id,
        },
        ..template.clone()
    }
}
pub(super) fn run(fence: &TimelineFence, template: &TimelineItem) -> Result<()> {
    let mut memory = Memory::default();
    let values: Vec<_> = (1..=1000).map(|n| item(template, n)).collect();
    let mut ordered = Vec::new();
    let mut lookup = Vec::new();
    for items in values.chunks(50) {
        let (a, b) = memory.leaves(fence, items)?;
        ordered.push(a);
        lookup.push(b);
    }
    let scope = &fence.conversation.scope;
    let old_order = memory.branch(scope, ordered.clone())?;
    let old_lookup = memory.branch(scope, lookup.clone())?;
    let from = memory.snapshot(fence, old_order.clone(), old_lookup.clone())?;
    let mut edited = values[500..550].to_vec();
    edited[0].revision = Revision(2);
    let next_fence = TimelineFence { projection: Revision(fence.projection.0 + 1), ..fence.clone() };
    let (a, b) = memory.leaves(&next_fence, &edited)?;
    ordered[10] = a;
    lookup[10] = b.clone();
    let changed_order = memory.branch(scope, ordered)?;
    let wrapped = memory.branch(scope, lookup[10..].to_vec())?;
    let mut mixed = lookup[..10].to_vec();
    mixed.push(wrapped);
    let changed_lookup = memory.branch(scope, mixed)?;
    let to = memory.snapshot(&next_fence, changed_order.clone(), changed_lookup.clone())?;
    let budget = Budget { work_bytes: 16 << 20, max_pages: 32, max_changes: 0 };
    let limit = PageLimit { items: 100, bytes: 262_144 };
    let Outcome::Ready(diff) = changes::page(&memory, &from, &to, None, limit, budget)? else {
        return Err("shared-tree diff pending".into());
    };
    assert!(matches!(diff.changes.as_slice(), [TimelineChange::Put { item }] if item == &edited[0]));
    assert!(
        diff.pages <= 16 && diff.skipped_subtrees >= 19,
        "followed {} pages, skipped {} subtrees",
        diff.pages,
        diff.skipped_subtrees
    );
    let pages = diff.pages;
    // A prepended leaf must not force the unchanged old lookup subtree open.
    let extra = item(template, 0);
    let (a, b) = memory.leaves(&next_fence, std::slice::from_ref(&extra))?;
    let new_order = memory.branch(scope, vec![a.clone(), old_order])?;
    let new_lookup = memory.branch(scope, vec![b.clone(), old_lookup.clone()])?;
    let prepended = memory.snapshot(&next_fence, new_order, new_lookup)?;
    memory.reads.borrow_mut().clear();
    let Outcome::Ready(diff) = changes::page(&memory, &from, &prepended, None, limit, budget)? else {
        return Err("prefix diff pending".into());
    };
    assert!(matches!(diff.changes.as_slice(), [TimelineChange::Put { item }] if item == &extra));
    assert_eq!(memory.reads.borrow().get(&old_lookup.root), None, "shared lookup subtree was opened");
    // A missing changed leaf after an earlier Put cannot produce a successful partial page.
    let new_order = memory.branch(scope, vec![a, changed_order])?;
    let new_lookup = memory.branch(scope, vec![b.clone(), changed_lookup])?;
    let combined = memory.snapshot(&next_fence, new_order, new_lookup)?;
    let page: ManifestPage = serde_json::from_slice(&memory.manifests[&lookup[10].root])?;
    let ManifestPage::Metadata { payload, .. } = page else { unreachable!() };
    memory.payloads.remove(&payload.id);
    assert!(matches!(changes::page(&memory, &from, &combined, None, limit, budget)?, Outcome::Pending { .. }));
    println!(
        "timeline Merkle diff: 1000 items, {pages} followed pages, 19 shared subtrees skipped, mixed depths, prepended boundary and missing changed leaf passed"
    );
    Ok(())
}
