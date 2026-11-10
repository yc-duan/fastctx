//! Large indexes stay addressable using witnesses reconstructed from public pages.
use super::*;
use fastctx_sync::{
    store::Error,
    timeline::{ByteRange, TimelineContentRead, TimelineContentResult},
};
#[path = "timeline_content_paths/fixture.rs"]
mod fixture;
#[path = "timeline_content_paths/read.rs"]
mod read;

pub(crate) async fn run(root: &Path) -> Result<()> {
    let directory = root.join("timeline-content-paths");
    fastctx_platform::ensure_private_dir(&directory)?;
    let path = directory.join("replica.sqlite3");
    let db = open(&path)?;
    let blocks =
        SqliteBlocks::new(db.clone(), directory.join("files"), holder(), policy(false), operation(198), 1 << 24)?;
    let sources = SqliteSources::new(
        blocks.clone(),
        Arc::new(Signer),
        vec![Arc::new(Gate(AtomicUsize::new(0)))],
        IngestBudget::for_machine(None)?,
    )?;
    let timelines = SqliteTimelines::new(blocks.clone());
    let primary = target(&custody(198, "conversation.timeline"), 198);
    let step = sources.begin_step(primary.clone(), None, HostGeneration::from_bytes([198; 32])).await?;
    let bound = sources.bind_target(step.clone(), primary.clone()).await?;
    let pictures = fixture::build(&sources, &bound, &primary.custody.object.object, fixture::Kind::Pictures, 1).await?;
    let outputs = fixture::build(&sources, &bound, &primary.custody.object.object, fixture::Kind::Outputs, 2).await?;
    let mut facts = Vec::new();
    for fixture in [&pictures, &outputs] {
        facts.push(observed(fixture.identity.clone(), 1, fixture.original.clone(), fixture.display.clone()));
        let mut roots = fixture.item.display.owned_content();
        roots.push(fixture.item.body.clone());
        facts.push(Fact {
            fact: TimelineFact::ItemRevision { change: TimelineChange::Put { item: fixture.item.clone() } },
            roots,
        });
    }
    let (prepared, _) = prepare_commit(&sources, &step, &bound, &primary, None, 0, facts).await?;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        match sources.ingest(prepared.clone()).await {
            Ok(_) => break,
            Err(Error::Busy) if std::time::Instant::now() < deadline => {}
            Err(error) => return Err(format!("committing large descriptor indexes: {error}").into()),
        }
    }
    settle_bounded(&db, &blocks, &timelines, 1024).await?;
    let result = timelines
        .timeline(
            primary.custody.context.clone(),
            TimelineRead {
                conversation: primary.custody.object.clone(),
                fence: None,
                anchor: TimelineAnchor::Latest,
                max_items: 10,
                max_bytes: 262144,
            },
        )
        .await?;
    let TimelineResult::Page { page } = result else {
        return Err(format!("large fixtures did not install: {result:?}").into());
    };
    assert_eq!(page.items.len(), 2);
    let fence = page.fence;
    let mut reports = Vec::new();
    let mut walked = Vec::new();
    for fixture in [&pictures, &outputs] {
        let leaves = read::walk(&timelines, &primary, &fence, fixture).await?;
        let left = &leaves[0];
        read::legacy_limit(&timelines, &primary, &fence, fixture, &left.content).await?;
        assert!(matches!(
            read::request(&timelines, &primary, &fence, fixture, left.content.clone(), Some(Vec::new()), None).await,
            Err(Error::Denied(_))
        ));
        assert!(
            matches!(
                read::request(
                    &timelines,
                    &primary,
                    &fence,
                    fixture,
                    fixture.item.body.clone(),
                    Some(vec![fixture.index.clone()]),
                    None
                )
                .await,
                Err(Error::Denied(_))
            ),
            "a directly owned target does not excuse an invalid supplied path"
        );
        let wrong_form = ContentRef::Single { block: left.content.id().clone(), decoded_bytes: 1 };
        assert!(
            matches!(
                read::request(&timelines, &primary, &fence, fixture, wrong_form, Some(left.path.clone()), None).await,
                Err(Error::Denied(_))
            ),
            "matching hashes do not excuse a different reference kind"
        );
        let mut skipped = left.path.clone();
        skipped.remove(skipped.len() - 1);
        assert!(matches!(
            read::request(&timelines, &primary, &fence, fixture, left.content.clone(), Some(skipped), None).await,
            Err(Error::Denied(_))
        ));
        let (payload, _) = read::bytes(&timelines, &primary, &fence, fixture, &left.content, &left.path).await?;
        read::ranges(&timelines, &primary, &fence, fixture, &left.content, &left.path, &payload).await?;
        let mut repeated = left.path.clone();
        repeated.push(repeated[0].clone());
        assert!(matches!(
            read::request(&timelines, &primary, &fence, fixture, left.content.clone(), Some(repeated), None).await,
            Err(Error::Invalid(_))
        ));
        let excessive = vec![fixture.item.body.clone(); 129];
        assert!(matches!(
            read::request(&timelines, &primary, &fence, fixture, left.content.clone(), Some(excessive), None).await,
            Err(Error::Invalid(_))
        ));
        reports.push(read::measure(&timelines, &primary, &fence, fixture, &leaves).await?);
        walked.push(leaves);
    }
    let left = &walked[0][0];
    assert!(
        matches!(
            read::request(&timelines, &primary, &fence, &outputs, left.content.clone(), Some(left.path.clone()), None)
                .await,
            Err(Error::Denied(_))
        ),
        "another item's valid object grant is insufficient"
    );
    let (left_payload, _) = read::bytes(&timelines, &primary, &fence, &pictures, &left.content, &left.path).await?;
    let leaf: fastctx_types::sync::conversation_timeline::ItemDisplayPartsV1 = serde_json::from_slice(&left_payload)?;
    let image = leaf.parts[0].content.clone();
    let mut image_path = left.path.clone();
    image_path.push(left.content.clone());
    let (image_bytes, _) = read::bytes(&timelines, &primary, &fence, &pictures, &image, &image_path).await?;
    read::ranges(&timelines, &primary, &fence, &pictures, &image, &image_path, &image_bytes).await?;
    let mut forged_length = image.clone();
    let ContentRef::Single { decoded_bytes, .. } = &mut forged_length else { unreachable!() };
    *decoded_bytes += 1;
    assert!(matches!(
        read::request(&timelines, &primary, &fence, &pictures, forged_length, Some(image_path.clone()), None).await,
        Err(Error::Denied(_))
    ));
    let mut foreign_path = image_path.clone();
    let ContentRef::Manifest { root } = &mut foreign_path[0] else { unreachable!() };
    root.0.domain.scope = ScopeId::from_bytes([199; 32]);
    assert!(matches!(
        read::request(&timelines, &primary, &fence, &pictures, image.clone(), Some(foreign_path), None).await,
        Err(Error::Denied(_))
    ));
    let mut single_parent = image_path;
    single_parent.push(image);
    assert!(matches!(
        read::request(&timelines, &primary, &fence, &pictures, pictures.original.clone(), Some(single_parent), None)
            .await,
        Err(Error::Denied(_))
    ));
    let direct = read::request(&timelines, &primary, &fence, &pictures, pictures.item.body.clone(), None, None).await?;
    assert!(matches!(direct, TimelineContentResult::Content { .. }), "legacy directly owned roots still work");
    let original_path = vec![pictures.item.body.clone()];
    assert!(
        matches!(
            read::request(&timelines, &primary, &fence, &pictures, pictures.original.clone(), None, None).await?,
            TimelineContentResult::Content { .. }
        ),
        "a small legacy closure still works"
    );
    read::bytes(&timelines, &primary, &fence, &pictures, &pictures.original, &original_path).await?;
    serde_contract(&primary, &fence, &pictures)?;
    fastctx_platform::disk::write(directory.join("measurements.json"), serde_json::to_vec_pretty(&reports)?)?;
    println!("timeline content paths: {}", serde_json::to_string(&reports)?);

    // Missing unrelated pages cannot turn a valid path into a global search.
    let unrelated = pictures.leaves.last().unwrap().0.digest.as_str().to_owned();
    let on_path = pictures.leaves[0].0.digest.as_str().to_owned();
    drop((sources, timelines, blocks, db));
    super::super::super::scheduler::closed(&path).await?;
    let db = open_with(&path, |db| {
        assert_eq!(db.execute("DELETE FROM sync_asset_manifests WHERE hash=?1", [&unrelated])?, 1);
        Ok(())
    })?;
    let blocks =
        SqliteBlocks::new(db.clone(), directory.join("files"), holder(), policy(false), operation(198), 1 << 24)?;
    let timelines = SqliteTimelines::new(blocks);
    read::bytes(&timelines, &primary, &fence, &pictures, &left.content, &left.path).await?;
    drop((timelines, db));
    super::super::super::scheduler::closed(&path).await?;
    let db = open_with(&path, |db| {
        assert_eq!(db.execute("DELETE FROM sync_asset_manifests WHERE hash=?1", [&on_path])?, 1);
        Ok(())
    })?;
    let blocks =
        SqliteBlocks::new(db.clone(), directory.join("files"), holder(), policy(false), operation(198), 1 << 24)?;
    let timelines = SqliteTimelines::new(blocks);
    assert!(matches!(
        read::request(
            &timelines,
            &primary,
            &fence,
            &pictures,
            leaf.parts[0].content.clone(),
            Some({
                let mut path = left.path.clone();
                path.push(left.content.clone());
                path
            }),
            None
        )
        .await?,
        TimelineContentResult::Pending { .. }
    ));
    timelines.release(primary.custody.context.clone(), fence.clone()).await?;
    assert!(matches!(
        read::request(&timelines, &primary, &fence, &pictures, left.content.clone(), Some(left.path.clone()), None)
            .await?,
        TimelineContentResult::ReanchorNeeded
    ));
    println!(
        "timeline content paths: exact references, terminal legacy limit, public pagination, ranges, missing ancestor Pending, unrelated missing sibling and released fences passed"
    );
    Ok(())
}
fn serde_contract(primary: &SourceTarget, fence: &TimelineFence, fixture: &fixture::Fixture) -> Result<()> {
    use fastctx_types::{ResidentId, timeline_reads::TimelineCall};
    let resident: ResidentId = "claude-a1b2c3d4".parse()?;
    for path in [None, Some(Vec::new()), Some(vec![fixture.item.body.clone()])] {
        let call = TimelineCall::Content {
            resident: resident.clone(),
            fence: Box::new(fence.clone()),
            item: fixture.item.id.clone(),
            content: fixture.original.clone(),
            range: None,
            path: path.clone(),
        };
        let value = serde_json::to_value(&call)?;
        assert_eq!(value.get("path").is_some(), path.is_some());
        let restored: TimelineCall = serde_json::from_value(value)?;
        assert!(matches!(restored, TimelineCall::Content { path: got, .. } if got == path));
        let read = TimelineContentRead {
            fence: fence.clone(),
            item: fixture.item.id.clone(),
            content: fixture.original.clone(),
            range: None,
            path,
        };
        let restored: TimelineContentRead = serde_json::from_slice(&serde_json::to_vec(&read)?)?;
        assert_eq!(restored, read);
    }
    assert_eq!(primary.custody.object, fence.conversation);
    Ok(())
}
