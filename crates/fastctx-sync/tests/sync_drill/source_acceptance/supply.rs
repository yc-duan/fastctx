//! A second device reads only accepted content, never producer-private staging.
use super::*;
use fastctx_sync::{
    authority::{Peer, ReadPolicy, Service},
    manifest::{ContentBlock, ManifestChild, ManifestPage, ManifestReply, ManifestRoot},
    replicate::BlockRequest,
    transfer::Message,
};
use fastctx_types::sync::{
    cursor::{BlockRange, Count},
    manifest::{BlobId, DigestAlgorithm},
};
fn page(scope: &ScopeKey, value: ManifestPage) -> Result<ManifestReply> {
    let page_json = serde_json::to_string(&value)?;
    let root = ManifestRoot(BlobId {
        domain: scope.clone(),
        algorithm: DigestAlgorithm::Sha256,
        digest: Digest::of(page_json.as_bytes()),
    });
    Ok(ManifestReply { root, page_json })
}
struct Visibility(AtomicBool);
impl ReadPolicy for Visibility {
    fn can_read(&self, _: &Peer, _: &fastctx_sync::scope::ScopeDescriptor) -> bool {
        !self.0.load(Ordering::Acquire)
    }
}
async fn json(service: &Service, peer: &Peer, request: Message) -> Result<Message> {
    let request = serde_json::from_slice(&serde_json::to_vec(&request)?)?;
    let reply = service.answer(peer, request).await?;
    Ok(serde_json::from_slice(&serde_json::to_vec(&reply)?)?)
}
async fn fetch(
    service: &Service,
    peer: &Peer,
    request: &BlockRequest,
) -> std::result::Result<fastctx_sync::replicate::Received<Vec<u8>>, Error> {
    let request =
        serde_json::from_slice(&serde_json::to_vec(request).map_err(|error| Error::Invalid(error.to_string()))?)
            .map_err(|error| Error::Invalid(error.to_string()))?;
    Box::pin(service.supply(peer, request)).await
}
pub(crate) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("replica-supply");
    fastctx_platform::ensure_private_dir(&dir.join("node"))?;
    fastctx_platform::ensure_private_dir(&dir.join("hub"))?;
    let object = conversation(42);
    let (_, _, source, target) = node(&dir.join("node"), &object)?;
    let step = source.begin_step(target.clone(), None, HostGeneration::from_bytes([42; 32])).await?;
    let bound = source.bind_target(step.clone(), target.clone()).await?;
    let bytes: Vec<Vec<u8>> = ["first accepted block", "second accepted block", "third accepted block"]
        .into_iter()
        .map(|text| text.as_bytes().to_vec())
        .collect();
    let mut content: Vec<ContentBlock> = Vec::new();
    for body in &bytes {
        let encoded = codec::encode(object.scope.clone(), body.clone(), Encoding::Raw)?;
        content.push(encoded.representation.content.clone());
        source.put_block(bound.clone(), encoded.representation, encoded.bytes).await?;
    }
    let first_leaf = page(&object.scope, ManifestPage::Blocks { offset: ByteCount(0), blocks: content[..2].to_vec() })?;
    let second_leaf = page(
        &object.scope,
        ManifestPage::Blocks {
            offset: ByteCount((bytes[0].len() + bytes[1].len()) as u64),
            blocks: content[2..].to_vec(),
        },
    )?;
    let root_page = page(
        &object.scope,
        ManifestPage::Branch {
            children: vec![
                ManifestChild {
                    first_key: "0".into(),
                    last_key: "1".into(),
                    root: first_leaf.root.clone(),
                    entries: Count(2),
                },
                ManifestChild {
                    first_key: "2".into(),
                    last_key: "2".into(),
                    root: second_leaf.root.clone(),
                    entries: Count(1),
                },
            ],
        },
    )?;
    let wide_page = |count| {
        page(
            &object.scope,
            ManifestPage::Branch {
                children: (0..count)
                    .map(|index| ManifestChild {
                        first_key: format!("{index:016x}"),
                        last_key: format!("{index:016x}"),
                        root: second_leaf.root.clone(),
                        entries: Count(1),
                    })
                    .collect(),
            },
        )
    };
    assert!(matches!(
        fastctx_sync::blocks::manifest::validate(&object.scope, wide_page(1001)?),
        Err(Error::Invalid(_))
    ));
    let (mut low, mut high) = (1_u64, 1001_u64);
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        if fastctx_sync::blocks::manifest::validate(&object.scope, wide_page(mid)?).is_ok() {
            low = mid;
        } else {
            high = mid;
        }
    }
    let wide_count = low;
    assert!(wide_count > 128);
    let wide = wide_page(wide_count)?;
    for page in [&first_leaf, &second_leaf, &root_page, &wide] {
        source.put_manifest(bound.clone(), page.clone()).await?;
    }
    let payload = codec::encode(object.scope.clone(), b"accepted fact".to_vec(), Encoding::Raw)?;
    source.put_block(bound.clone(), payload.representation.clone(), payload.bytes.clone()).await?;
    let batch = SourceBatch {
        format: 1,
        scope: object.scope.clone(),
        stream: target.stream.clone(),
        epoch: target.epoch.clone(),
        producer: target.producer.clone(),
        kind: KIND.into(),
        schema: 1,
        operation: step.operation().clone(),
        after: Sequence(0),
        through: Sequence(1),
        records: vec![SourceRecord {
            sequence: Sequence(1),
            operation: operation(1),
            kind: KIND.into(),
            schema: 1,
            payload: payload.representation.content.clone(),
            roots: vec![
                ContentRef::Manifest { root: root_page.root.clone() },
                ContentRef::Manifest { root: wide.root.clone() },
            ],
            causes: Vec::new(),
        }],
    };
    let next = SourceCheckpoint {
        source: target.source.clone(),
        generation: step.generation().clone(),
        revision: step.revision(),
        committed_offset: ByteCount(1),
        parser_version: 1,
        state: None,
    };
    source
        .ingest(IngestOperation::new(
            step,
            None,
            next,
            vec![IngestBatch {
                target: bound,
                facts: Some(IngestFacts::Local { batch: batch.clone() }),
                effects: SourceEffects::default(),
            }],
        )?)
        .await?;
    let (db, blocks) = open_hub(&dir.join("hub"), false)?;
    let (run, authority) = authority(&blocks, 1, &Arc::new(Gate(AtomicBool::new(false)))).await?;
    let directory = Arc::new(SqliteDirectory::new(blocks.clone(), run, Arc::new(Signer(true))));
    describe(&directory, &object, 214).await?;
    let visibility = Arc::new(Visibility(AtomicBool::new(false)));
    let service =
        Service::new(directory).with_sources(Arc::new(authority.clone())).with_read_policy(visibility.clone());
    let producer = Peer { device: DeviceId::from_bytes([3; 32]), accounts: vec![account().id] };
    let peer = Peer { device: DeviceId::from_bytes([4; 32]), accounts: producer.accounts.clone() };
    let period = HoldingPeriod {
        holder: Holder::Device { device: peer.device.clone() },
        publication_epoch: Revision(77),
        ..holding(&object)
    };
    let request = BlockRequest {
        holding: period.clone(),
        object: target.custody.object.clone(),
        kind: fastctx_sync::transfer::KindVersion { kind: KIND.into(), schema: 1 },
        root: ContentRef::Manifest { root: root_page.root.clone() },
        block: content[2].clone(),
    };
    for body in &bytes {
        let encoded = codec::encode(object.scope.clone(), body.clone(), Encoding::Raw)?;
        authority.put_block(upload(&batch), encoded.representation, encoded.bytes).await?;
    }
    authority.put_block(upload(&batch), payload.representation.clone(), payload.bytes).await?;
    for page in [&first_leaf, &second_leaf, &root_page, &wide] {
        authority.put_manifest(upload(&batch), page.clone()).await?;
    }
    assert!(matches!(fetch(&service, &peer, &request).await, Err(Error::Denied(_))));
    answered(&service, &batch).await?;
    // Root reachability is sufficient while descendant access is still queued.
    assert_eq!(fetch(&service, &peer, &request).await?.value(), &bytes[2]);
    assert!(matches!(
        service.answer(&peer, Message::GetDescriptor { holding: period.clone(), blob: content[0].id.clone() }).await,
        Err(Error::Unavailable(_))
    ));
    settle(&db, &blocks, None, Some(&authority)).await?;
    assert_eq!(fetch(&service, &peer, &request).await?.value(), &bytes[2]);
    let single = BlockRequest {
        root: payload.representation.content.clone().into(),
        block: payload.representation.content.clone(),
        ..request.clone()
    };
    assert_eq!(fetch(&service, &peer, &single).await?.value(), b"accepted fact");
    let cache = Database::open(&dir.join("hub/hub.sqlite3"), "")?;
    let bad = ObjectId::from_bytes([0; 32]);
    let id = content[0].id.clone();
    let other = conversation(43).scope;
    cache
        .write(4096, move |db| {
            db.execute(
                "INSERT INTO sync_asset_access(hub,scope,object,kind,hash,root_kind,decoded_bytes,expanded)
             SELECT hub,scope,?3,kind,hash,root_kind,decoded_bytes,expanded FROM sync_asset_access
             WHERE hub=?1 AND scope=?2 AND hash=?4 AND root_kind='block' LIMIT 1",
                params![id.domain.hub.to_string(), id.domain.scope.as_str(), bad.as_str(), id.digest.as_str()],
            )?;
            db.execute(
                "INSERT INTO sync_asset_access(hub,scope,object,kind,hash,root_kind,decoded_bytes,expanded)
             SELECT hub,?3,object,kind,hash,root_kind,decoded_bytes,expanded FROM sync_asset_access
             WHERE hub=?1 AND scope=?2 AND hash=?4 AND root_kind='block' LIMIT 1",
                params![id.domain.hub.to_string(), id.domain.scope.as_str(), other.scope.as_str(), id.digest.as_str()],
            )?;
            Ok(())
        })
        .await?;
    assert!(
        matches!(json(&service,&peer,Message::GetDescriptor {holding:period.clone(),blob:content[0].id.clone()}).await?,Message::Descriptor {descriptor,..} if descriptor.content==content[0])
    );
    assert!(
        matches!(json(&service,&peer,Message::GetManifest {holding:period.clone(),root:root_page.root.clone()}).await?,Message::Manifest {reply,..} if reply==root_page)
    );
    let selected = json(
        &service,
        &peer,
        Message::WantBlocks {
            holding: period.clone(),
            manifest: root_page.root.clone(),
            missing: vec![BlockRange { start: Count(1), end: Count(3) }],
        },
    )
    .await?;
    let Message::Blocks { blocks: selected, remaining, .. } = selected else {
        return Err("wrong block selection response".into());
    };
    assert_eq!(selected.iter().map(|block| block.content.clone()).collect::<Vec<_>>(), content[1..]);
    assert!(remaining.is_empty());
    let mut missing = vec![BlockRange { start: Count(0), end: Count(wide_count) }];
    let mut received = 0;
    while !missing.is_empty() {
        let answer = json(
            &service,
            &peer,
            Message::WantBlocks { holding: period.clone(), manifest: wide.root.clone(), missing },
        )
        .await?;
        let Message::Blocks { blocks: selected, remaining, .. } = answer else {
            return Err("wrong range response".into());
        };
        assert!(!selected.is_empty());
        for block in selected {
            assert_eq!(block.index.0, received);
            assert_eq!(block.content, content[2]);
            received += 1;
        }
        missing = remaining;
    }
    assert_eq!(received, wide_count);
    println!(
        "ordered ranges: {wide_count} children (maximum legal escaped page), all ordinals delivered; 1001 children exceed the existing 256KiB page contract"
    );
    for digest in [content[0].id.digest.clone(), Digest::from_bytes([199; 32])] {
        let crossed = BlobId { domain: conversation(43).scope, digest, ..content[0].id.clone() };
        assert!(
            matches!(service.recheck_message(&peer,&Message::GetDescriptor { holding:period.clone(),blob:crossed }).await,
            Err(Error::Denied(message)) if message=="descriptor is outside the requested scope")
        );
    }
    let sibling = BlockRequest { root: ContentRef::Manifest { root: first_leaf.root }, ..request.clone() };
    assert!(matches!(fetch(&service, &peer, &sibling).await, Err(Error::Denied(_))));
    let stale =
        BlockRequest { holding: HoldingPeriod { acl_revision: Revision(0), ..period.clone() }, ..request.clone() };
    assert!(matches!(fetch(&service, &peer, &stale).await, Err(Error::Revoked)));
    let stranger = Peer { accounts: Vec::new(), ..peer.clone() };
    assert!(matches!(fetch(&service, &stranger, &request).await, Err(Error::Denied(_))));
    let metadata_object = BlockRequest { object: object.clone(), ..request.clone() };
    assert!(matches!(fetch(&service, &peer, &metadata_object).await, Err(Error::Denied(_))));
    let other = BlockRequest { object: timeline(&conversation(43)), ..request.clone() };
    assert!(matches!(fetch(&service, &peer, &other).await, Err(Error::Denied(_))));
    visibility.0.store(true, Ordering::Release);
    assert!(service.recheck(&peer, &period).await.is_err());
    assert!(matches!(fetch(&service, &peer, &request).await, Err(Error::Denied(_))));
    visibility.0.store(false, Ordering::Release);
    let file = dir.join("hub/hub.sqlite3");
    let fixture = Database::open(&file, "")?;
    let key = target.custody.object.clone();
    fixture.write(4096,move |db| { db.execute("INSERT INTO sync_tombstones(hub,scope,object,authority_epoch,revision,operation) VALUES(?1,?2,?3,?4,?5,'fixture-delete')",params![key.scope.hub.to_string(),key.scope.scope.as_str(),key.object.as_str(),[1_u8;16].as_slice(),1_u64.to_be_bytes().as_slice()])?;Ok(()) }).await?;
    assert!(matches!(fetch(&service, &peer, &request).await, Err(Error::Denied(_))));
    assert_eq!(blocks.buffer_budget().reserved(), 0);
    println!(
        "replica supply: accepted roots, JSON loopback, second device, single blocks, ordered manifest ranges, closure membership, stale tenure, revocation, deleted objects and released admission passed"
    );
    Ok(())
}
