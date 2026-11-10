//! Snapshot patches are applied by ID and compared with an independent full read.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fastctx_sync::timeline::{
    ByteRange, TimelineChangesRead, TimelineChangesResult, TimelineContentRead, TimelineContentResult, TimelinePage,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn patch(
    timelines: &SqliteTimelines,
    context: &AccessContext,
    from: &TimelinePage,
    expected: &[TimelineItem],
) -> Result<()> {
    let mut items: BTreeMap<_, _> = from.items.iter().cloned().map(|item| (item.id.clone(), item)).collect();
    let mut to = None;
    let mut after = None;
    let mut changed = BTreeSet::new();
    for _ in 0..32 {
        let result = timelines
            .changes(
                context.clone(),
                TimelineChangesRead {
                    from: from.fence.clone(),
                    to: to.clone(),
                    after: after.clone(),
                    max_items: 1,
                    max_bytes: 262_144,
                },
            )
            .await?;
        let TimelineChangesResult::Page { to: found, changes, next } = result else {
            return Err(format!("diff not ready: {result:?}").into());
        };
        if let Some(to) = &to {
            assert_eq!(to, &*found, "pagination retains the exact target hold");
        } else {
            to = Some(*found);
        }
        for change in changes {
            let id = match &change {
                TimelineChange::Put { item } => &item.id,
                TimelineChange::Retract { item, .. } => item,
                _ => return Err("a reader patch emitted SelectBranch".into()),
            };
            assert!(after.as_ref().is_none_or(|after| id > after));
            assert!(changed.insert(id.clone()), "a paged patch repeats an item");
            match change {
                TimelineChange::Put { item } => {
                    items.insert(item.id.clone(), item);
                }
                TimelineChange::Retract { item, revision } => {
                    assert_eq!(items.remove(&item).unwrap().revision, revision);
                }
                _ => unreachable!(),
            }
        }
        if next.is_none() {
            let expected: BTreeMap<_, _> = expected.iter().cloned().map(|item| (item.id.clone(), item)).collect();
            assert_eq!(items, expected, "applying the patch equals directly reading the new snapshot");
            timelines.release(context.clone(), to.unwrap()).await?;
            return Ok(());
        }
        assert_ne!(next, after, "a patch page must advance its ID seek");
        after = next;
    }
    Err("patch pagination did not finish".into())
}

async fn bytes(
    timelines: &SqliteTimelines,
    context: &AccessContext,
    fence: &TimelineFence,
    item: &ObjectId,
    content: ContentRef,
) -> Result<Vec<u8>> {
    let mut offset = ByteCount(0);
    let mut output = Vec::new();
    loop {
        let result = timelines
            .content(
                context.clone(),
                TimelineContentRead {
                    fence: fence.clone(),
                    item: item.clone(),
                    content: content.clone(),
                    range: Some(ByteRange { offset, bytes: 3 }),
                    path: Vec::new(),
                },
            )
            .await?;
        assert!(serde_json::to_vec(&result)?.len() <= fastctx_types::sync::PAGE_BYTES as usize);
        let TimelineContentResult::Content { base64, offset: got, total, next } = result else {
            return Err("owned content was not ready".into());
        };
        assert_eq!(got, offset);
        output.extend(STANDARD.decode(base64)?);
        let Some(next) = next else {
            assert_eq!(output.len() as u64, total.0);
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    Ok(output)
}

pub(super) async fn content(
    timelines: &SqliteTimelines,
    context: &AccessContext,
    page: &TimelinePage,
    other: &TimelineItem,
) -> Result<()> {
    let item = &page.items[0];
    let part = item.display.parts.first().ok_or("no display part")?.content.clone();
    assert_eq!(bytes(timelines, context, &page.fence, &item.id, part.clone()).await?, b"\x89PNG first");
    let mut forged = page.fence.clone();
    forged.root.0.domain.scope = ScopeId::from_bytes([199; 32]);
    assert!(matches!(
        timelines
            .content(
                context.clone(),
                TimelineContentRead {
                    fence: forged,
                    item: item.id.clone(),
                    content: part.clone(),
                    range: None,
                    path: Vec::new()
                }
            )
            .await?,
        TimelineContentResult::ReanchorNeeded
    ));
    let mut foreign_scope = part.clone();
    match &mut foreign_scope {
        ContentRef::Single { block, .. } => block.domain.scope = ScopeId::from_bytes([199; 32]),
        ContentRef::Manifest { root } => root.0.domain.scope = ScopeId::from_bytes([199; 32]),
    }
    assert!(matches!(
        timelines
            .content(
                context.clone(),
                TimelineContentRead {
                    fence: page.fence.clone(),
                    item: item.id.clone(),
                    content: foreign_scope,
                    range: None,
                    path: Vec::new(),
                }
            )
            .await,
        Err(fastctx_sync::store::Error::Denied(_))
    ));

    let body = bytes(timelines, context, &page.fence, &item.id, item.body.clone()).await?;
    let _: fastctx_types::sync::conversation_timeline::ProjectedBody = serde_json::from_slice(&body)?;
    let foreign = other.display.parts.first().ok_or("no other part")?.content.clone();
    let refused = timelines
        .content(
            context.clone(),
            TimelineContentRead {
                fence: page.fence.clone(),
                item: item.id.clone(),
                content: foreign,
                range: None,
                path: Vec::new(),
            },
        )
        .await;
    assert!(
        matches!(refused, Err(fastctx_sync::store::Error::Denied(_))),
        "an object-wide grant does not authorize another item's content"
    );
    Ok(())
}
