//! Large public descriptor indexes share tiny bodies, so index width is the workload.
use super::*;
use fastctx_sync::manifest::{ContentBlock, ManifestChild};
use fastctx_types::sync::{
    conversation_identity::{ImageSelector, ItemKey, ItemRole, NativeId, NativeItemKey},
    conversation_timeline::*,
    cursor::Count,
    metadata::MetadataCodec,
};

pub(super) const ITEMS: usize = 10_000;
const IMAGE: &[u8] = b"\x89PNG\r\n\x1a\nimage fixture";
pub(super) const TEXT: &[u8] = b"one public output row\n";

#[derive(Clone, Copy, Debug)]
pub(super) enum Kind {
    Pictures,
    Outputs,
}
pub(super) struct Fixture {
    pub item: TimelineItem,
    pub identity: ItemIdentity,
    pub original: ContentRef,
    pub display: ContentRef,
    pub anchor: ContentRef,
    pub index: ContentRef,
    pub leaves: Vec<ManifestRoot>,
    pub kind: Kind,
}
struct Node {
    root: ManifestRoot,
    start: u64,
    count: u64,
}
async fn metadata_page(
    sources: &SqliteSources,
    bound: &SourceStepTarget,
    codec: MetadataCodec,
    bytes: Vec<u8>,
    references: Vec<ContentRef>,
) -> Result<ContentRef> {
    let payload = block(sources, bound, &bytes).await?;
    let page = source_ingest::metadata(codec, payload, references)?;
    let root = page.root.clone();
    sources.put_manifest(bound.clone(), page).await?;
    Ok(ContentRef::Manifest { root })
}
async fn branch(sources: &SqliteSources, bound: &SourceStepTarget, children: &[Node]) -> Result<Node> {
    let start = children[0].start;
    let count = children.iter().map(|node| node.count).sum();
    let page_json = serde_json::to_string(&ManifestPage::Branch {
        children: children
            .iter()
            .map(|node| ManifestChild {
                first_key: format!("{:016x}", node.start),
                last_key: format!("{:016x}", node.start + node.count - 1),
                root: node.root.clone(),
                entries: Count(node.count),
            })
            .collect(),
    })?;
    let root = ManifestRoot(BlobId {
        domain: scope(),
        algorithm: DigestAlgorithm::Sha256,
        digest: Digest::of(page_json.as_bytes()),
    });
    sources.put_manifest(bound.clone(), ManifestReply { root: root.clone(), page_json }).await?;
    Ok(Node { root, start, count })
}
async fn tree(
    sources: &SqliteSources,
    bound: &SourceStepTarget,
    kind: Kind,
) -> Result<(ContentRef, Vec<ManifestRoot>)> {
    let shared: ContentBlock = block(
        sources,
        bound,
        match kind {
            Kind::Pictures => IMAGE,
            Kind::Outputs => TEXT,
        },
    )
    .await?;
    let shared = ContentRef::from(shared);
    let leaf_size = match kind {
        Kind::Pictures => 8,
        Kind::Outputs => FACT_PARTS,
    };
    let mut level = Vec::new();
    let mut leaves = Vec::new();
    for start in (0..ITEMS).step_by(leaf_size) {
        let count = leaf_size.min(ITEMS - start);
        let (codec, bytes, references) = match kind {
            Kind::Pictures => {
                let page = ItemDisplayPartsV1 {
                    version: 1,
                    start: Count(start as u64),
                    parts: (start..start + count)
                        .map(|index| DisplayPart {
                            index: Count(index as u64),
                            channel: DisplayChannel::Image,
                            content: shared.clone(),
                            bytes: ByteCount(IMAGE.len() as u64),
                            preview: None,
                            media_type: Some("image/png".into()),
                            picture: Some(DisplayPicture {
                                selector: ImageSelector::ClaudeMessage {
                                    uuid: NativeId::new("many-pictures").unwrap(),
                                    image: Count(index as u64),
                                },
                                target: PictureTarget::Owned { content: shared.clone() },
                            }),
                        })
                        .collect(),
                };
                (MetadataCodec::DisplayPartsV1, page.encode()?, page.owned_content())
            }
            Kind::Outputs => {
                let page = ItemFactBodyV1 {
                    version: 1,
                    fact: ItemFactV1::Parts {
                        part_kind: FactPartKindV1::Output,
                        start: Count(start as u64),
                        parts: (0..count)
                            .map(|_| FactPartV1::Output { output: ToolOutputV1::Text { text: shared.clone() } })
                            .collect(),
                    },
                };
                (MetadataCodec::ItemFactsV1, page.encode()?, page.owned_content())
            }
        };
        let ContentRef::Manifest { root } = metadata_page(sources, bound, codec, bytes, references).await? else {
            unreachable!()
        };
        leaves.push(root.clone());
        level.push(Node { root, start: start as u64, count: count as u64 });
    }
    let fanout = match kind {
        Kind::Pictures => 32,
        Kind::Outputs => 2,
    };
    while level.len() > 1 {
        let mut next = Vec::new();
        for children in level.chunks(fanout) {
            next.push(branch(sources, bound, children).await?);
        }
        level = next;
    }
    Ok((ContentRef::Manifest { root: level.pop().unwrap().root }, leaves))
}
pub(super) async fn build(
    sources: &SqliteSources,
    bound: &SourceStepTarget,
    conversation: &ObjectId,
    kind: Kind,
    accepted: u64,
) -> Result<Fixture> {
    let (index, leaves) = tree(sources, bound, kind).await?;
    let (identity, anchor, facts, more, tool, display_kind) = match kind {
        Kind::Pictures => {
            let identity = ItemIdentity::Native(ItemKey {
                conversation: conversation.clone(),
                role: ItemRole::User,
                native: NativeItemKey::ClaudeMessage { uuid: NativeId::new("many-pictures")? },
            });
            let ContentRef::Manifest { root } = &index else { unreachable!() };
            (
                identity,
                index.clone(),
                None,
                Some(DisplayPartsRef { root: root.clone(), start: Count(0), count: Count(ITEMS as u64) }),
                None,
                DisplayKind::User,
            )
        }
        Kind::Outputs => {
            let detail = ItemFactBodyV1 {
                version: 1,
                fact: ItemFactV1::ToolDetail { uuid: None, requested: None, requested_machine: None, stored: None },
            };
            let detail =
                metadata_page(sources, bound, MetadataCodec::ItemFactsV1, detail.encode()?, detail.owned_content())
                    .await?;
            let value = ItemFactBodyV1 {
                version: 1,
                fact: ItemFactV1::Tool {
                    name: None,
                    server: None,
                    machine: None,
                    source: Some(ToolSourceV1::Native),
                    does: Some(Box::new(ToolActionV1::Call)),
                    input: None,
                    output: Some(index.clone()),
                    structured: None,
                    detail,
                },
            };
            let anchor =
                metadata_page(sources, bound, MetadataCodec::ItemFactsV1, value.encode()?, value.owned_content())
                    .await?;
            let identity = ItemIdentity::Native(ItemKey {
                conversation: conversation.clone(),
                role: ItemRole::ToolCall,
                native: NativeItemKey::ClaudeTool { tool_use_id: NativeId::new("many-outputs")? },
            });
            let facts = Some(Box::new(DisplayFactsV1 { semantic: Some(anchor.clone()), ..Default::default() }));
            let tool = Some(ToolSummary {
                name: "fixture".into(),
                server: None,
                machine: None,
                does: ActionSummary { kind: ActionKind::Call, text: None, command: None, path: None },
            });
            (identity, anchor, facts, None, tool, DisplayKind::Tool)
        }
    };
    let descriptor = ItemDisplayV1 {
        version: 1,
        kind: display_kind,
        tool,
        status: Some(DisplayStatus::Complete),
        facts,
        append: None,
        parts: Vec::new(),
        more,
    };
    let display =
        metadata_page(sources, bound, MetadataCodec::DisplayV1, descriptor.encode()?, descriptor.owned_content())
            .await?;
    let original = ContentRef::from(block(sources, bound, b"source fixture evidence").await?);
    let value = ProjectedBody {
        identity: identity.clone(),
        primary: BodySnapshot {
            original: original.clone(),
            display: display.clone(),
            complete: true,
            text_part: None,
            text_bytes: ByteCount(0),
        },
        authority: BodyAuthority::HostFinal,
        world: None,
        host: None,
        run: None,
    };
    let body = metadata_page(
        sources,
        bound,
        MetadataCodec::BodyV1,
        serde_json::to_vec(&value)?,
        value.owned_content().into_iter().cloned().collect(),
    )
    .await?;
    let item = TimelineItem {
        id: identity.id(),
        revision: Revision(1),
        turn: Some(ObjectId::from_bytes([198; 32])),
        parent: None,
        position: TimelinePosition {
            turn: Sequence(accepted),
            display_ancestry: vec![Sequence(1)],
            accepted: Sequence(accepted),
            item: identity.id(),
        },
        body,
        display: Box::new(descriptor),
    };
    Ok(Fixture { item, identity, original, display, anchor, index, leaves, kind })
}
