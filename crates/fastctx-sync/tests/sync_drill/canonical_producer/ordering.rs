//! Both input arrival orders use the same source writer, reducer and installed
//! position index. Old fences keep the old immutable order across relocation.
use super::*;
use fastctx_sync::timeline::{
    TimelineAnchor, TimelineFence, TimelinePage, TimelineRead, TimelineReader, TimelineResult,
};

struct Fixture {
    db: Database,
    blocks: SqliteBlocks,
    sources: SqliteSources,
    timelines: SqliteTimelines,
    primary: SourceTarget,
    conversation: ObjectId,
    turn: TurnDeclaration,
}
impl Fixture {
    fn open(dir: &Path) -> Result<Self> {
        fastctx_platform::ensure_private_dir(dir)?;
        let db = open(&dir.join("replica.sqlite3"))?;
        let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(97), 1 << 24)?;
        let sources = SqliteSources::new(
            blocks.clone(),
            Arc::new(Signer),
            vec![Arc::new(Gate(AtomicUsize::new(0))) as Arc<dyn SourceAuthorizer>],
            fastctx_sync::store::ingest_memory::IngestBudget::new(32 << 20)?,
        )?;
        let timelines = SqliteTimelines::new(blocks.clone());
        let conversation = ObjectId::from_bytes([197; 32]);
        let mut owner = custody(97, KIND);
        owner.object.object =
            fastctx_types::sync::conversation_identity::ConversationObject::Timeline.id(&conversation);
        let primary = target(&owner, 97);
        let turn = TurnDeclaration {
            key: TurnKey {
                conversation: conversation.clone(),
                native: NativeTurnKey::World { turn: NativeId::new("ordered-turn")? },
            },
            order: Sequence(1),
        };
        Ok(Self { db, blocks, sources, timelines, primary, conversation, turn })
    }
    fn key(&self, name: &str) -> ItemKey {
        ItemKey {
            conversation: self.conversation.clone(),
            role: if name == "title" { ItemRole::System } else { ItemRole::AssistantBlock },
            native: NativeItemKey::ClaudeMessage { uuid: NativeId::new(name).unwrap() },
        }
    }
    fn native(&self, ordinal: u64) -> Placement {
        Placement {
            turn: Some(self.turn.key.id()),
            slot: DisplaySlot::Sibling,
            proof: PlacementProof::NativeTurn,
            order: Some(DisplayOrder::Native { sequence: Sequence(ordinal) }),
        }
    }
    fn gap(&self, before: &str) -> Placement {
        Placement {
            turn: Some(self.turn.key.id()),
            slot: DisplaySlot::Sibling,
            proof: PlacementProof::Causal,
            order: Some(DisplayOrder::Gap {
                before: Some(self.key(before).id()),
                at: "2026-01-01T00:00:20Z".parse().unwrap(),
                sequence: Sequence(7),
                identity: self.key("W").id(),
            }),
        }
    }
    async fn advance(&self, action: Option<(&str, Placement, bool)>, units: u32) -> Result<MergeProgress> {
        let generation = HostGeneration::from_bytes([97; 32]);
        let expected = self.sources.checkpoint(self.primary.clone()).await?;
        let after = self.sources.progress(self.primary.clone()).await?.map_or(Sequence(0), |p| p.cursor().committed);
        let step = self
            .sources
            .begin_step(self.primary.clone(), expected.as_ref().map(|c| c.revision), generation.clone())
            .await?;
        let bound = self.sources.bind_target(step, self.primary.clone()).await?;
        let _window = acquire_window(&self.sources, &bound).await?;
        let mut cache = Supplied::default();
        let mut inputs = Vec::new();
        let needs_turn = action.as_ref().is_some_and(|(_, p, _)| p.turn.is_some());
        if needs_turn
            && self
                .sources
                .get_index(
                    self.primary.clone(),
                    SourceFamily::Reducer,
                    format!("canon/v1/{}/turn/{}", self.conversation.as_str(), self.turn.key.id().as_str())
                        .into_bytes(),
                )
                .await?
                .is_none()
        {
            inputs.push(input(&bound, after.0 + 1, after.0 + 1, TimelineFact::Turn { turn: self.turn.clone() }));
        }
        if let Some((name, placement, observed)) = action {
            let sequence = after.0 + inputs.len() as u64 + 1;
            let fact = if observed {
                let text = if name == "title" { format!("title-{sequence}") } else { name.into() };
                let display = cache.display(&self.sources, &bound, &text).await?;
                let original = cache.publish_block(&self.sources, &bound, text.as_bytes()).await?;
                let old = self
                    .sources
                    .get_index(
                        self.primary.clone(),
                        SourceFamily::Reducer,
                        format!("canon/v1/{}/item/{}", self.conversation.as_str(), self.key(name).id().as_str())
                            .into_bytes(),
                    )
                    .await?;
                let created = old
                    .map(|r| serde_json::from_slice::<ItemState>(&r.value))
                    .transpose()?
                    .map_or(Sequence(sequence), |s| s.created);
                TimelineFact::Observed {
                    observation: Observation {
                        identity: ItemIdentity::Native(self.key(name)),
                        created,
                        authority: if name == "W" { BodyAuthority::World } else { BodyAuthority::HostFinal },
                        body: BodySnapshot {
                            original,
                            display,
                            complete: true,
                            text_part: None,
                            text_bytes: ByteCount(0),
                        },
                        placement: Some(placement),
                    },
                }
            } else {
                TimelineFact::Placement { item: self.key(name).id(), placement }
            };
            inputs.push(input(&bound, sequence, sequence, fact));
        }
        let initial = InitialState {
            active_branch: self.conversation.clone(),
            sources: vec![SourceSeed { source: self.primary.source.clone(), generation }],
        };
        let output = plan_step(
            &self.sources,
            MergeRequest {
                target: &bound,
                conversation: &self.conversation,
                after,
                inputs: &inputs,
                supplied: &cache,
                initial: expected.is_none().then_some(&initial),
                other_effects: effects(&self.primary)?,
                budget: MergeBudget { max_work_units: units, ..MergeBudget::default() },
            },
        )
        .await?
        .publish(&self.sources)
        .await?;
        assert_eq!(output.progress.consumed_inputs, inputs.len());
        let progress = output.progress.clone();
        commit(&self.sources, &bound, expected, output).await?;
        Ok(progress)
    }
    async fn settle(&self) -> Result<()> {
        for _ in 0..64 {
            let progress = self.advance(None, 2).await?;
            if !progress.ready_work {
                return settle(&self.db, &self.blocks, &self.timelines).await;
            }
        }
        Err("paged order repair did not settle".into())
    }
    async fn page(&self, fence: Option<TimelineFence>) -> Result<TimelinePage> {
        let context = AccessContext { principal: Principal::LocalService, scope: scope(), observed_acl: Revision(1) };
        let read = TimelineRead {
            conversation: self.primary.custody.object.clone(),
            fence,
            anchor: TimelineAnchor::Latest,
            max_items: 100,
            max_bytes: 262144,
        };
        match self.timelines.timeline(context, read).await? {
            TimelineResult::Page { page } => Ok(*page),
            result => Err(format!("order page is not ready: {result:?}").into()),
        }
    }
    fn names<'a>(&self, page: &'a TimelinePage) -> Vec<&'a str> {
        page.items.iter().map(|i| i.display.parts[0].preview.as_deref().unwrap()).collect()
    }
}

pub(super) async fn run(root: &Path) -> Result<()> {
    session_channel(root).await?;
    for world_first in [false, true] {
        let dir = root.join(if world_first { "ordering-world-first" } else { "ordering-host-first" });
        let mut f = Fixture::open(&dir)?;
        if world_first {
            let pending = f.advance(Some(("W", f.gap("B"), true)), 16).await?;
            assert!(pending.waiting_dependencies);
            drop(f);
            super::super::super::scheduler::closed(&dir.join("replica.sqlite3")).await?;
            f = Fixture::open(&dir)?;
        }
        f.advance(Some(("A", f.native(10), true)), 16).await?;
        f.advance(Some(("B", f.native(20), true)), 16).await?;
        f.settle().await?;
        let before = f.page(None).await?;
        if !world_first {
            assert_eq!(f.names(&before), ["A", "B"]);
            f.advance(Some(("W", f.gap("B"), true)), 16).await?;
            f.settle().await?;
        }
        let inserted = f.page(None).await?;
        assert_eq!(f.names(&inserted), ["A", "W", "B"]);
        for native in before.items.iter().filter(|i| i.id != f.key("W").id()) {
            assert_eq!(
                inserted.items.iter().find(|i| i.id == native.id).unwrap(),
                native,
                "inserting World facts neither moves nor revises native items"
            );
        }
        let child = Placement { slot: DisplaySlot::Child { parent: f.key("W").id() }, ..f.native(1) };
        f.advance(Some(("child", child, true)), 16).await?;
        f.settle().await?;
        let old = f.page(None).await?;
        assert_eq!(f.names(&old), ["A", "W", "child", "B"]);
        f.advance(Some(("W", f.gap("A"), false)), 1).await?;
        // The relocation and its child's repair survive another source-engine restart.
        drop(f);
        super::super::super::scheduler::closed(&dir.join("replica.sqlite3")).await?;
        f = Fixture::open(&dir)?;
        f.settle().await?;
        let moved = f.page(None).await?;
        assert_eq!(f.names(&moved), ["W", "child", "A", "B"]);
        assert_eq!(f.page(Some(old.fence.clone())).await?.items, old.items);
        assert_eq!(count(&f.db, "SELECT COUNT(*) FROM sync_timeline_items WHERE active=1")?, 4);
        for item in &moved.items {
            let bytes = fastctx_sync::timeline::position::encode(&item.position)?;
            assert_eq!(fastctx_sync::timeline::position::decode(&bytes)?, item.position);
        }
        let keys = moved
            .items
            .iter()
            .map(|i| fastctx_sync::timeline::position::encode(&i.position))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        // Moving a native pivot refreshes exactly the World gaps anchored to it.
        let queued = f.advance(Some(("A", f.native(30), false)), 1).await?;
        assert!(queued.ready_work);
        f.settle().await?;
        assert_eq!(f.names(&f.page(None).await?), ["B", "W", "child", "A"]);
        let pending = f.advance(Some(("W", f.gap("never-arrives"), false)), 16).await?;
        assert!(pending.waiting_dependencies);
        f.advance(Some(("W", f.gap("B"), false)), 16).await?;
        f.settle().await?;
        let complete = f.advance(None, 16).await?;
        assert!(!complete.waiting_dependencies, "a newer placement retires its missing old anchor wait");
        assert_eq!(f.names(&f.page(None).await?), ["W", "child", "B", "A"]);
    }
    println!(
        "timeline ordering: late gap insertion, both input orders, durable missing anchors, paged relocation, child repair, old fences and active indexes passed"
    );
    Ok(())
}

async fn session_channel(root: &Path) -> Result<()> {
    let f = Fixture::open(&root.join("ordering-session"))?;
    let placement = Placement {
        turn: None,
        slot: DisplaySlot::Session,
        proof: PlacementProof::Causal,
        order: Some(DisplayOrder::Native { sequence: Sequence(1) }),
    };
    let progress = f.advance(Some(("title", placement.clone(), true)), 16).await?;
    assert!(!progress.waiting_dependencies);
    f.settle().await?;
    let old = f.page(None).await?;
    assert_eq!(old.items.len(), 1);
    assert_eq!(old.items[0].turn, None);
    assert_eq!(old.items[0].position.turn, Sequence(0));
    let turn_key = format!("canon/v1/{}/turn/{}", f.conversation.as_str(), f.turn.key.id().as_str()).into_bytes();
    assert!(f.sources.get_index(f.primary.clone(), SourceFamily::Reducer, turn_key).await?.is_none());
    f.advance(Some(("title", placement, true)), 16).await?;
    f.settle().await?;
    let revised = f.page(None).await?;
    assert_eq!(revised.items.len(), 1);
    assert_eq!(revised.items[0].id, old.items[0].id);
    assert_eq!(revised.items[0].revision, Revision(2));
    assert_ne!(revised.items[0].display, old.items[0].display);
    f.advance(Some(("A", f.native(1), true)), 16).await?;
    f.settle().await?;
    let page = f.page(None).await?;
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0], revised.items[0]);
    assert_eq!(page.items[1].id, f.key("A").id());
    assert_eq!(f.page(Some(old.fence)).await?.items, old.items);
    assert_eq!(count(&f.db, "SELECT COUNT(*) FROM sync_timeline_items WHERE active=1 AND turn IS NULL")?, 1);
    println!("session channel: title-only source, no fabricated turn, latest revision and pre-turn ordering passed");
    Ok(())
}
