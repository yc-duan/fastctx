//! Public descriptors alone produce witnesses for subsequent requests.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fastctx_platform::readings;
use fastctx_types::sync::conversation_timeline::{
    ItemDisplayPartsBranchV1, ItemDisplayPartsV1, ItemFactBodyV1, ItemFactV1,
};
use std::time::Instant;

#[derive(Clone, Debug)]
pub(super) struct Leaf {
    pub content: ContentRef,
    pub path: Vec<ContentRef>,
    pub start: u64,
    pub count: u64,
}
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub(super) struct Fetches {
    item: u64,
    ownership: u64,
    content: u64,
}
impl Fetches {
    fn now() -> Self {
        Self {
            item: readings::counter("sync.timeline.item_reads.ordinary").get(),
            ownership: readings::counter("sync.timeline.ownership_reads.ordinary").get(),
            content: readings::counter("sync.timeline.content_reads.ordinary").get(),
        }
    }
    fn since(self, before: Self) -> Self {
        Self {
            item: self.item - before.item,
            ownership: self.ownership - before.ownership,
            content: self.content - before.content,
        }
    }
}
pub(super) async fn request(
    timelines: &SqliteTimelines,
    primary: &SourceTarget,
    fence: &TimelineFence,
    fixture: &fixture::Fixture,
    content: ContentRef,
    path: Option<Vec<ContentRef>>,
    range: Option<ByteRange>,
) -> std::result::Result<TimelineContentResult, fastctx_sync::store::Error> {
    timelines
        .content(
            primary.custody.context.clone(),
            TimelineContentRead { fence: fence.clone(), item: fixture.item.id.clone(), content, range, path },
        )
        .await
}
pub(super) async fn bytes(
    timelines: &SqliteTimelines,
    primary: &SourceTarget,
    fence: &TimelineFence,
    fixture: &fixture::Fixture,
    content: &ContentRef,
    path: &[ContentRef],
) -> Result<(Vec<u8>, Fetches)> {
    let before = Fetches::now();
    let result = request(timelines, primary, fence, fixture, content.clone(), Some(path.to_vec()), None).await?;
    let fetches = Fetches::now().since(before);
    let TimelineContentResult::Content { base64, offset, total, next } = result else {
        return Err(format!("descriptor was not readable: {result:?}").into());
    };
    let bytes = STANDARD.decode(base64)?;
    assert_eq!(offset, ByteCount(0));
    assert_eq!(total.0, bytes.len() as u64);
    assert_eq!(next, None, "a descriptor leaf fits one public page");
    assert!(fetches.ownership + fetches.content <= 1024);
    Ok((bytes, fetches))
}
pub(super) async fn legacy_limit(
    timelines: &SqliteTimelines,
    primary: &SourceTarget,
    fence: &TimelineFence,
    fixture: &fixture::Fixture,
    target: &ContentRef,
) -> Result<()> {
    let before = Fetches::now();
    let result = request(timelines, primary, fence, fixture, target.clone(), None, None).await;
    let fetches = Fetches::now().since(before);
    assert!(
        matches!(result, Err(Error::Invalid(ref text)) if text == "content ownership path required; repeat the request with path"),
        "wide legacy lookup terminates with a path request: {result:?}"
    );
    assert_eq!(fetches.ownership, 512);
    assert_eq!(fetches.content, 0);
    Ok(())
}
pub(super) async fn walk(
    timelines: &SqliteTimelines,
    primary: &SourceTarget,
    fence: &TimelineFence,
    fixture: &fixture::Fixture,
) -> Result<Vec<Leaf>> {
    let prefix = if matches!(fixture.kind, fixture::Kind::Outputs) {
        let (bytes, fetches) = bytes(timelines, primary, fence, fixture, &fixture.anchor, &[]).await?;
        assert_eq!(fetches.ownership, 0);
        let tool: ItemFactBodyV1 = serde_json::from_slice(&bytes)?;
        let ItemFactV1::Tool { output: Some(root), .. } = tool.fact else { return Err("tool has no output".into()) };
        assert_eq!(root, fixture.index);
        vec![fixture.anchor.clone()]
    } else {
        Vec::new()
    };
    let mut pending =
        vec![Leaf { content: fixture.index.clone(), path: prefix, start: 0, count: fixture::ITEMS as u64 }];
    let mut leaves = Vec::new();
    while let Some(node) = pending.pop() {
        let (bytes, _) = bytes(timelines, primary, fence, fixture, &node.content, &node.path).await?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        if value.get("children").is_some() {
            let branch: ItemDisplayPartsBranchV1 = serde_json::from_value(value)?;
            assert_eq!((branch.version, branch.start.0, branch.count.0), (1, node.start, node.count));
            let mut path = node.path;
            path.push(node.content);
            let mut end = node.start;
            let mut children = Vec::new();
            for child in branch.children {
                assert_eq!(child.start.0, end, "public ranges are contiguous");
                end += child.count.0;
                children.push(Leaf {
                    content: ContentRef::Manifest { root: child.root },
                    path: path.clone(),
                    start: child.start.0,
                    count: child.count.0,
                });
            }
            assert_eq!(end, node.start + node.count);
            pending.extend(children.into_iter().rev());
        } else {
            match fixture.kind {
                fixture::Kind::Pictures => {
                    let leaf: ItemDisplayPartsV1 = serde_json::from_value(value)?;
                    assert_eq!(leaf.start.0, node.start);
                    assert_eq!(leaf.parts.len() as u64, node.count);
                    for (index, part) in leaf.parts.iter().enumerate() {
                        assert_eq!(part.index.0, node.start + index as u64);
                    }
                }
                fixture::Kind::Outputs => {
                    let leaf: ItemFactBodyV1 = serde_json::from_value(value)?;
                    let ItemFactV1::Parts { start, parts, .. } = leaf.fact else {
                        return Err("not a parts leaf".into());
                    };
                    assert_eq!((start.0, parts.len() as u64), (node.start, node.count));
                }
            }
            leaves.push(node);
        }
    }
    assert_eq!(leaves.iter().map(|node| node.count).sum::<u64>(), fixture::ITEMS as u64);
    assert_eq!(leaves.len(), fixture.leaves.len());
    for (leaf, root) in leaves.iter().zip(&fixture.leaves) {
        assert_eq!(leaf.content, ContentRef::Manifest { root: root.clone() });
    }
    Ok(leaves)
}
pub(super) async fn measure(
    timelines: &SqliteTimelines,
    primary: &SourceTarget,
    fence: &TimelineFence,
    fixture: &fixture::Fixture,
    leaves: &[Leaf],
) -> Result<serde_json::Value> {
    let mut micros = Vec::new();
    let mut max_fetches = Fetches::default();
    let mut max_depth = 0;
    for index in 0..1000 {
        let leaf = &leaves[index * 7919 % leaves.len()];
        let started = Instant::now();
        let (_, fetches) = bytes(timelines, primary, fence, fixture, &leaf.content, &leaf.path).await?;
        micros.push(started.elapsed().as_micros() as u64);
        let expected = leaf.path.len() as u64 + u64::from(matches!(fixture.kind, fixture::Kind::Outputs));
        assert_eq!(
            fetches.ownership, expected,
            "only selected ancestors are fetched, including metadata payload validation"
        );
        assert_eq!(fetches.content, 2, "a metadata leaf reads its envelope and payload");
        assert!(fetches.item > 0 && fetches.item <= 8);
        max_fetches.item = max_fetches.item.max(fetches.item);
        max_fetches.ownership = max_fetches.ownership.max(fetches.ownership);
        max_fetches.content = max_fetches.content.max(fetches.content);
        max_depth = max_depth.max(leaf.path.len());
    }
    micros.sort_unstable();
    Ok(serde_json::json!({
        "kind": format!("{:?}", fixture.kind), "items": fixture::ITEMS,
        "leaves": leaves.len(), "samples": micros.len(), "max_edges": max_depth,
        "max_fetches": max_fetches,
        "p50_us": micros[499], "p95_us": micros[949], "p99_us": micros[989], "max_us": micros[999],
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "io_unit": "storage fetch attempts, not physical disk operations",
        "cache": "descriptor traversal precedes sampling"
    }))
}
pub(super) async fn ranges(
    timelines: &SqliteTimelines,
    primary: &SourceTarget,
    fence: &TimelineFence,
    fixture: &fixture::Fixture,
    content: &ContentRef,
    path: &[ContentRef],
    expected: &[u8],
) -> Result<()> {
    let mut output = Vec::new();
    let mut offset = ByteCount(0);
    loop {
        let result = request(
            timelines,
            primary,
            fence,
            fixture,
            content.clone(),
            Some(path.to_vec()),
            Some(ByteRange { offset, bytes: 17 }),
        )
        .await?;
        let TimelineContentResult::Content { base64, offset: got, total, next } = result else {
            return Err(format!("range not ready: {result:?}").into());
        };
        assert_eq!(got, offset);
        assert_eq!(total.0, expected.len() as u64);
        output.extend(STANDARD.decode(base64)?);
        let Some(next) = next else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(output, expected);
    Ok(())
}
