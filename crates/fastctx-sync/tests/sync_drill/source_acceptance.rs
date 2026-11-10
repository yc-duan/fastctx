//! The hub accepts exact device evidence and confirms the producer without
//! confusing durable facts with missing attachments or changing local identities.
use super::*;
#[path = "source_acceptance/fixture.rs"]
mod fixture;
#[path = "source_acceptance/follow.rs"]
pub(crate) mod follow;
#[path = "source_pages.rs"]
mod source_pages;
#[path = "source_acceptance/upload/mod.rs"]
pub(crate) mod upload;
#[path = "source_acceptance/supply.rs"]
mod supply;
use fastctx_sync::{
    catalog::{Lifecycle, SourceBatch, SourceRecord},
    cursor::{Sequence, StreamCursor},
    manifest::ContentRef,
    proof::{ConfirmedEpoch, EpochDescriptor, HubSigningIdentity, ProofSigner, SignedSourceBatch},
    scheduler::TaskHandler,
    scope::{AccountIdentity, ScopeSubject, ScopeVisibility},
    store::{
        Error,
        authority::{SourceAccepted, SourceReceipt, SourceUpload, SqliteAuthority},
        control::AuthorityRun,
        directory::{ScopeDescription, SqliteDirectory},
        ingest::{
            IngestBatch, IngestFacts, IngestOperation, SourceCheckpoint, SourceIngest, SourceTarget, StreamProgress,
        },
        ingest_effects::SourceEffects,
        ingest_memory::IngestBudget,
        source_authorize::{PreparedSourceChange, ReceivedSourceBatch, SourceAuthorizer},
        sources::SqliteSources,
    },
};
use fixture::{Gate, Policy, Signer, Verifier, entropy, identity};
const KIND: &str = "conversation.timeline";
fn hub() -> HubId {
    HubId::from_bytes([41; 8])
}
fn conversation(n: u8) -> ObjectKey {
    let object = ObjectId::from_bytes([n; 32]);
    ObjectKey {
        scope: ScopeKey { hub: hub(), scope: fastctx_types::sync::conversation_identity::conversation_scope(&object) },
        object,
    }
}
fn timeline(conversation: &ObjectKey) -> ObjectKey {
    ObjectKey {
        scope: conversation.scope.clone(),
        object: fastctx_types::sync::conversation_identity::source_object(&conversation.object, KIND).unwrap(),
    }
}
fn account() -> AccountIdentity {
    let name = "alice".parse().unwrap();
    let opened = serde_json::from_str("\"2023-11-14T22:13:20Z\"").unwrap();
    AccountIdentity { id: AccountId::from_legacy(&hub(), &name, opened), name, opened }
}
fn context(object: &ObjectKey) -> AccessContext {
    AccessContext {
        principal: Principal::Account { account: account().id, device: DeviceId::from_bytes([3; 32]) },
        scope: object.scope.clone(),
        observed_acl: Revision(1),
    }
}
fn holding(object: &ObjectKey) -> HoldingPeriod {
    HoldingPeriod {
        holder: holder(),
        scope: object.scope.clone(),
        acl_revision: Revision(1),
        publication_epoch: Revision(1),
    }
}
fn open_hub(dir: &Path, fail: bool) -> Result<(Database, SqliteBlocks)> {
    let db = Database::open_guarded(&dir.join("hub.sqlite3"), "", boundary::guard, |db| {
        schema::initialize(db)?;
        db.execute_batch("DROP TRIGGER IF EXISTS acceptance_fault;")?;
        if fail {
            db.execute_batch("CREATE TRIGGER acceptance_fault BEFORE INSERT ON sync_operations WHEN NEW.actor LIKE 'source/%' BEGIN SELECT RAISE(ABORT,'injected acceptance rollback'); END;")?;
        }
        Ok(())
    })?;
    let blocks = SqliteBlocks::new(
        db.clone(),
        dir.join("files"),
        Holder::Hub { hub: hub() },
        Arc::new(Policy(account().id)),
        operation(211),
        1 << 24,
    )?;
    Ok((db, blocks))
}
async fn authority(blocks: &SqliteBlocks, seed: u8, gate: &Arc<Gate>) -> Result<(AuthorityRun, SqliteAuthority)> {
    let run = AuthorityRun::begin(blocks, hub(), entropy(seed)).await?;
    let service = SqliteAuthority::new(
        blocks.clone(),
        run.clone(),
        Arc::new(Signer(true)),
        Arc::new(Verifier),
        identity(),
        vec![gate.clone()],
        8 * 1024 * 1024,
    )?;
    Ok((run, service))
}
async fn describe(directory: &SqliteDirectory, object: &ObjectKey, n: u8) -> Result<()> {
    directory
        .publish(
            ScopeDescription {
                scope: object.scope.clone(),
                owner: account(),
                subject: ScopeSubject::Conversation {
                    conversation: object.object.clone(),
                    shared_on: None,
                    born_seal: None,
                },
                visibility: ScopeVisibility::Current,
                acl_revision: Revision(1),
                lifecycle: Lifecycle::Live,
            },
            operation(n),
        )
        .await?;
    Ok(())
}
fn node(dir: &Path, object: &ObjectKey) -> Result<(Database, SqliteBlocks, SqliteSources, SourceTarget)> {
    let key = object.scope.clone();
    let origin = serde_json::to_vec(&serde_json::json!({
        "origin": "hosted",
        "subject": { "subject": "conversation", "conversation": object.object, "shared_on": null, "born_seal": null },
        "device": DeviceId::from_bytes([3; 32]),
    }))?;
    let db = Database::open_guarded(&dir.join("node.sqlite3"), "", boundary::guard, move |db| {
        schema::initialize(db)?;
        db.execute("INSERT INTO sync_scopes(hub,scope,origin,acl_revision,enabled,publication_epoch) VALUES(?1,?2,?3,?4,1,?4) ON CONFLICT DO NOTHING",params![key.hub.to_string(),key.scope.as_str(),origin,1_u64.to_be_bytes().as_slice()])?;
        Ok(())
    })?;
    let blocks = SqliteBlocks::new(
        db.clone(),
        dir.join("files"),
        holder(),
        Arc::new(Policy(account().id)),
        operation(212),
        1 << 24,
    )?;
    let sources = SqliteSources::new(
        blocks.clone(),
        Arc::new(Signer(false)),
        vec![Arc::new(Gate(AtomicBool::new(false)))],
        IngestBudget::for_machine(None)?,
    )?
    .with_hub(identity(), Arc::new(Verifier))?;
    let epoch = ProducerEpoch::from_bytes([42; 32]);
    let custody = Custody {
        context: context(object),
        kind: REGISTRY.asset(KIND)?.registered(),
        object: timeline(object),
        holding: holding(object),
        purpose: HoldingUse::RuntimeSource {
            host: fastctx_sync::store::access::HostBinding {
                conversation: object.clone(),
                account: account().id,
                host: DeviceId::from_bytes([3; 32]),
                epoch: epoch.clone(),
            },
        },
    };
    let primary = SourceTarget {
        source: SourceId::from_bytes([42; 32]),
        custody,
        stream: StreamId::of(&timeline(object), KIND),
        epoch,
        producer: Producer::Device { device: DeviceId::from_bytes([3; 32]) },
    };
    Ok((db, blocks, sources, primary))
}
async fn append(
    sources: &SqliteSources,
    target: &SourceTarget,
    expected: Option<SourceCheckpoint>,
    after: u64,
    n: u8,
    attachment: bool,
) -> Result<(SourceCheckpoint, SourceBatch, Option<Vec<u8>>)> {
    let step = sources
        .begin_step(target.clone(), expected.as_ref().map(|c| c.revision), HostGeneration::from_bytes([42; 32]))
        .await?;
    let bound = sources.bind_target(step.clone(), target.clone()).await?;
    let mut roots = Vec::new();
    let attachment_bytes = attachment.then(|| b"attachment retained by the original producer".to_vec());
    if let Some(bytes) = &attachment_bytes {
        let encoded = codec::encode(target.custody.object.scope.clone(), bytes.clone(), Encoding::Raw)?;
        roots.push(ContentRef::from(encoded.representation.content.clone()));
        sources.put_block(bound.clone(), encoded.representation, encoded.bytes).await?;
    }
    let mut records = Vec::new();
    for i in 1..=n {
        let bytes = format!("source fact {}", after + u64::from(i)).into_bytes();
        let encoded = codec::encode(target.custody.object.scope.clone(), bytes, Encoding::Raw)?;
        let payload = encoded.representation.content.clone();
        sources.put_block(bound.clone(), encoded.representation, encoded.bytes).await?;
        records.push(SourceRecord {
            sequence: Sequence(after + u64::from(i)),
            operation: operation((after + u64::from(i)) as u8),
            kind: KIND.into(),
            schema: 1,
            payload,
            roots: roots.clone(),
            causes: Vec::new(),
        });
    }
    let batch = SourceBatch {
        format: 1,
        scope: target.custody.object.scope.clone(),
        stream: target.stream.clone(),
        producer: target.producer.clone(),
        epoch: target.epoch.clone(),
        operation: step.operation().clone(),
        kind: KIND.into(),
        schema: 1,
        after: Sequence(after),
        through: Sequence(after + u64::from(n)),
        records,
    };
    let next = SourceCheckpoint {
        source: target.source.clone(),
        generation: step.generation().clone(),
        revision: step.revision(),
        committed_offset: ByteCount(after + u64::from(n)),
        parser_version: 1,
        state: None,
    };
    sources
        .ingest(IngestOperation::new(
            step,
            expected,
            next.clone(),
            vec![IngestBatch {
                target: bound,
                facts: Some(IngestFacts::Local { batch: batch.clone() }),
                effects: SourceEffects::default(),
            }],
        )?)
        .await?;
    Ok((next, batch, attachment_bytes))
}
fn upload(batch: &SourceBatch) -> SourceUpload {
    SourceUpload {
        context: context(&conversation(42)),
        holding: holding(&conversation(42)),
        kind: KIND.into(),
        epoch: batch.epoch.clone(),
        operation: batch.operation.clone(),
    }
}
async fn upload_payloads(authority: &SqliteAuthority, batch: &SourceBatch) -> Result<()> {
    for record in &batch.records {
        let encoded = codec::encode(
            batch.scope.clone(),
            format!("source fact {}", record.sequence.0).into_bytes(),
            Encoding::Raw,
        )?;
        assert_eq!(encoded.representation.content, record.payload);
        authority.put_block(upload(batch), encoded.representation, encoded.bytes).await?;
    }
    Ok(())
}
async fn accepted(authority: &SqliteAuthority, batch: &SourceBatch) -> Result<(SourceReceipt, bool)> {
    let signed = Signer(false).source(batch)?;
    match authority.accept(context(&conversation(42)), holding(&conversation(42)), signed).await? {
        SourceAccepted::Accepted { receipt, content_complete } => Ok((*receipt, content_complete)),
        _ => Err("source batch stayed waiting".into()),
    }
}
fn count_scope(db: &Database, table: &str, object: &ObjectKey) -> Result<i64> {
    Ok(db.read_now(|db| {
        Ok(db.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE hub=?1 AND scope=?2"),
            params![object.scope.hub.to_string(), object.scope.scope.as_str()],
            |r| r.get(0),
        )?)
    })?)
}
fn prefixes(db: &Database, object: &ObjectKey) -> Result<Vec<String>> {
    Ok(db.read_now(|db| {
        Ok(db
            .prepare("SELECT prefix_digest FROM sync_stream_records WHERE hub=?1 AND scope=?2 ORDER BY sequence")?
            .query_map(params![object.scope.hub.to_string(), object.scope.scope.as_str()], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })?)
}
async fn settle(
    db: &Database,
    blocks: &SqliteBlocks,
    source: Option<&SqliteSources>,
    authority: Option<&SqliteAuthority>,
) -> Result<()> {
    let scheduler = SqliteScheduler::new(
        db.clone(),
        operation(213),
        if source.is_some() { holder() } else { Holder::Hub { hub: hub() } },
        Duration::from_secs(10),
    )?;
    for _ in 0..256 {
        let leases = scheduler.due(now_millis(), 32).await?;
        if leases.is_empty() {
            return Ok(());
        }
        for lease in leases {
            let outcome = if let Some(source) = source.filter(|s| s.takes(&lease.task.key)) {
                source.build_prefix(lease.clone()).await?
            } else if let Some(authority) = authority.filter(|a| a.takes(&lease.task.key)) {
                authority.process(lease.clone()).await?
            } else {
                blocks.process(lease.clone()).await?
            };
            scheduler.finish(lease, outcome).await?;
        }
    }
    Err("source acceptance tasks did not settle".into())
}
async fn answered(server: &fastctx_sync::authority::Service, batch: &SourceBatch) -> Result<(SourceReceipt, bool)> {
    let peer = fastctx_sync::authority::Peer { device: DeviceId::from_bytes([3; 32]), accounts: vec![account().id] };
    let request = fastctx_sync::transfer::Message::Source {
        holding: holding(&conversation(42)),
        batch: Signer(false).source(batch)?,
    };
    let request = serde_json::from_slice(&serde_json::to_vec(&request)?)?;
    let response = server.answer(&peer, request).await?;
    let response = serde_json::from_slice(&serde_json::to_vec(&response)?)?;
    match response {
        fastctx_sync::transfer::Message::SourceResult {
            result: SourceAccepted::Accepted { receipt, content_complete },
        } => Ok((*receipt, content_complete)),
        _ => Err("source metadata did not return an accepted result".into()),
    }
}
pub(super) async fn run(root: &Path) -> Result<()> {
    Box::pin(follow::run(root)).await?;
    let dir = root.join("source-acceptance");
    let hub_dir = dir.join("hub");
    let node_dir = dir.join("node");
    fastctx_platform::ensure_private_dir(&hub_dir)?;
    fastctx_platform::ensure_private_dir(&node_dir)?;
    let object = conversation(42);
    let gate = Arc::new(Gate(AtomicBool::new(false)));
    let (node_db, node_blocks, sources, primary) = node(&node_dir, &object)?;
    let (next, first, attachment) = append(&sources, &primary, None, 0, 2, true).await?;
    let (next, second, _) = append(&sources, &primary, Some(next), 2, 1, false).await?;
    let (db, blocks) = open_hub(&hub_dir, true)?;
    let (run, service) = authority(&blocks, 1, &gate).await?;
    let directory = SqliteDirectory::new(blocks.clone(), run, Arc::new(Signer(true)));
    describe(&directory, &object, 214).await?;
    let mut metadata_stream = first.clone();
    metadata_stream.stream = StreamId::of(&object, KIND);
    assert!(matches!(
        service.accept(context(&object), holding(&object), Signer(false).source(&metadata_stream)?).await,
        Err(Error::Invalid(reason)) if reason.contains("does not derive from the described object")
    ));
    upload_payloads(&service, &second).await?;
    let result = service.accept(context(&object), holding(&object), Signer(false).source(&second)?).await?;
    assert!(matches!(result, SourceAccepted::AwaitingEpoch { .. }));
    assert_eq!(count_scope(&db, "sync_stream_records", &object)?, 0);
    upload_payloads(&service, &first).await?;
    // A failure after records, epoch and feed writes must roll back all of them.
    assert!(accepted(&service, &first).await.is_err());
    assert_eq!(count_scope(&db, "sync_stream_records", &object)?, 0);
    assert_eq!(count_scope(&db, "sync_epoch_descriptors", &object)?, 0);
    assert_eq!(count_scope(&db, "sync_changes", &object)?, 0);
    drop((directory, service, blocks, db));
    super::super::scheduler::closed(&hub_dir.join("hub.sqlite3")).await?;
    let (db, blocks) = open_hub(&hub_dir, false)?;
    let (run, service) = authority(&blocks, 100, &gate).await?;
    let metadata = fastctx_sync::authority::Service::new(Arc::new(SqliteDirectory::new(
        blocks.clone(),
        run,
        Arc::new(Signer(true)),
    )))
    .with_sources(Arc::new(service.clone()));
    let (receipt, complete) = answered(&metadata, &first).await?;
    assert!(!complete, "accepting facts never claims the missing attachment");
    assert_eq!(receipt.head.key, timeline(&object));
    assert_ne!(receipt.head.key, object);
    assert_eq!(receipt.head.completeness, fastctx_sync::cursor::Completeness::MetadataOnly);
    assert_eq!(answered(&metadata, &first).await?.0, receipt);
    settle(&db, &blocks, None, Some(&service)).await?;
    assert_eq!(count_scope(&db, "sync_stream_records", &object)?, 3);
    assert_eq!(count_scope(&db, "sync_authority_waiting", &object)?, 0);
    assert_eq!(count_scope(&db, "sync_epoch_descriptors", &object)?, 1);
    sources.confirm_epoch(context(&object), receipt.epoch.clone()).await?;
    settle(&node_db, &node_blocks, Some(&sources), None).await?;
    assert_eq!(prefixes(&node_db, &object)?, prefixes(&db, &object)?);
    // Same operation with another signed statement is retained, never applied.
    let mut changed = first.clone();
    changed.records[0].operation = operation(200);
    assert!(matches!(
        service.accept(context(&object), holding(&object), Signer(false).source(&changed)?).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(count_scope(&db, "sync_conflicts", &object)?, 1);
    assert_eq!(count_scope(&db, "sync_stream_records", &object)?, 3);
    // The producer continues locally under the confirmed epoch.
    let (_, third, _) = append(&sources, &primary, Some(next), 3, 1, false).await?;
    assert!(matches!(
        sources.progress(primary.clone()).await?,
        Some(StreamProgress::Confirmed { cursor: StreamCursor { committed: Sequence(4), .. }, .. })
    ));
    upload_payloads(&service, &third).await?;
    gate.0.store(true, Ordering::Release);
    assert!(answered(&metadata, &third).await.is_err());
    assert_eq!(count_scope(&db, "sync_stream_records", &object)?, 3);
    gate.0.store(false, Ordering::Release);
    answered(&metadata, &third).await?;
    assert_eq!(prefixes(&node_db, &object)?, prefixes(&db, &object)?);
    // Completing content later preserves the original fact receipt and feed count.
    let data = attachment.unwrap();
    let encoded = codec::encode(object.scope.clone(), data, Encoding::Raw)?;
    let peer = fastctx_sync::authority::Peer { device: DeviceId::from_bytes([3; 32]), accounts: vec![account().id] };
    metadata
        .put_source_block(
            &peer,
            fastctx_sync::authority::PeerUpload {
                holding: holding(&object),
                kind: KIND.into(),
                epoch: first.epoch.clone(),
                operation: first.operation.clone(),
            },
            encoded.representation,
            encoded.bytes,
        )
        .await?;
    let unserved = fastctx_sync::authority::Peer { accounts: Vec::new(), ..peer };
    assert!(matches!(
        metadata
            .answer(
                &unserved,
                fastctx_sync::transfer::Message::Source {
                    holding: holding(&object),
                    batch: Signer(false).source(&first)?
                }
            )
            .await,
        Err(Error::Denied(_))
    ));
    let (replay, complete) = answered(&metadata, &first).await?;
    assert!(complete);
    assert_eq!(replay, receipt);
    assert_eq!(answered(&metadata, &first).await?, (receipt.clone(), true));
    assert!(answered(&metadata, &second).await?.1);
    assert_eq!(count_scope(&db, "sync_changes", &object)?, 3);
    source_pages::run(root).await?;
    Box::pin(upload::run(root)).await?;
    Box::pin(supply::run(&dir)).await?;
    println!(
        "hub source acceptance: out-of-order persistence, FULL rollback, restart, initial epoch confirmation, exact replay, conflicting proof retention, missing attachment, hub semantic refusal and continued producer prefixes passed"
    );
    Ok(())
}
