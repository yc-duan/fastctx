//! Producer confirmation resumes in bounded pages and keeps ingesting the same epoch.
use super::{
    source_ingest::{Gate, Signer, checkpoint},
    source_steps::target,
    *,
};
use fastctx_sync::{
    catalog::{Authority, SourceBatch, SourceRecord},
    cursor::Sequence,
    proof::{
        Bytes, CommittedFragment, ConfirmedEpoch, EpochDescriptor, HandoverBasis, HubKeyRotation, HubSigningIdentity,
        ProofPurpose, ProofVerifier, Signature, SignedAcceptanceFragment, SignedSourceBatch, SignedStatement,
        SigningKey,
    },
    scheduler::{Outcome, TaskHandler},
    store::{
        Error,
        ingest::{
            IngestBatch, IngestFacts, IngestOperation, SourceCheckpoint, SourceIngest, SourceTarget, StreamProgress,
        },
        ingest_effects::SourceEffects,
        ingest_memory::IngestBudget,
        source_authorize::SourceAuthorizer,
        sources::SqliteSources,
    },
};

struct Verifier;
impl ProofVerifier for Verifier {
    fn accepted(&self, _: &SignedAcceptanceFragment, _: &Authority) -> std::result::Result<CommittedFragment, Error> {
        Err(Error::Unavailable("unused drill proof".into()))
    }
    fn source(&self, _: &SignedSourceBatch, _: &EpochDescriptor) -> std::result::Result<SourceBatch, Error> {
        Err(Error::Unavailable("unused drill proof".into()))
    }
    fn epoch(
        &self,
        signed: &ConfirmedEpoch,
        identity: &HubSigningIdentity,
    ) -> std::result::Result<EpochDescriptor, Error> {
        if signed.proof.purpose != ProofPurpose::EpochHandoverV1
            || signed.proof.signer != identity.key
            || signed.proof.signature.0 != [19; 64]
        {
            return Err(Error::Denied("drill rejected epoch evidence".into()));
        }
        let value: EpochDescriptor =
            serde_json::from_slice(&signed.proof.data.0).map_err(|e| Error::Invalid(e.to_string()))?;
        if value.scope.hub != identity.hub {
            return Err(Error::Denied("wrong drill hub".into()));
        }
        Ok(value)
    }
    fn rotation(&self, _: &SignedStatement, _: &HubSigningIdentity) -> std::result::Result<HubKeyRotation, Error> {
        Err(Error::Unavailable("unused drill proof".into()))
    }
}
fn identity() -> HubSigningIdentity {
    HubSigningIdentity { hub: scope().hub, key: SigningKey([18; 32]), revision: Revision(1), rotations: Vec::new() }
}
fn writer(blocks: SqliteBlocks) -> Result<SqliteSources> {
    let installed = vec![Arc::new(Gate(AtomicUsize::new(0))) as Arc<dyn SourceAuthorizer>];
    Ok(SqliteSources::new(blocks, Arc::new(Signer), installed, IngestBudget::for_machine(None)?)?
        .with_hub(identity(), Arc::new(Verifier))?)
}
fn confirmation(primary: &SourceTarget) -> Result<ConfirmedEpoch> {
    let value = EpochDescriptor {
        scope: scope(),
        stream: primary.stream.clone(),
        kind: "conversation.timeline".into(),
        schema: 1,
        epoch: primary.epoch.clone(),
        producer: primary.producer.clone(),
        predecessor: None,
        closed_through: None,
        operation: operation(181),
        control_sequence: Sequence(1),
        basis: HandoverBasis::Initial { registration: operation(181) },
    };
    Ok(ConfirmedEpoch {
        proof: SignedStatement {
            purpose: ProofPurpose::EpochHandoverV1,
            signer: identity().key,
            data: Bytes(serde_json::to_vec(&value)?),
            signature: Signature([19; 64]),
        },
    })
}
async fn append(
    sources: &SqliteSources,
    primary: &SourceTarget,
    expected: Option<SourceCheckpoint>,
    after: u64,
    n: u8,
) -> Result<SourceCheckpoint> {
    let generation = HostGeneration::from_bytes([180; 32]);
    let step = sources.begin_step(primary.clone(), expected.as_ref().map(|p| p.revision), generation).await?;
    let bound = sources.bind_target(step.clone(), primary.clone()).await?;
    let payload = super::source_ingest::block(sources, &bound, b"confirmed source payload").await?;
    let records = (1..=n)
        .map(|i| SourceRecord {
            sequence: Sequence(after + u64::from(i)),
            operation: operation((after + u64::from(i)) as u8),
            kind: "conversation.timeline".into(),
            schema: 1,
            payload: payload.clone(),
            roots: Vec::new(),
            causes: Vec::new(),
        })
        .collect();
    let batch = SourceBatch {
        format: 1,
        scope: scope(),
        stream: primary.stream.clone(),
        producer: primary.producer.clone(),
        epoch: primary.epoch.clone(),
        operation: step.operation().clone(),
        kind: "conversation.timeline".into(),
        schema: 1,
        after: Sequence(after),
        through: Sequence(after + u64::from(n)),
        records,
    };
    let next = checkpoint(primary, &step, after + u64::from(n), None);
    sources
        .ingest(IngestOperation::new(
            step,
            expected,
            next.clone(),
            vec![IngestBatch {
                target: bound,
                facts: Some(IngestFacts::Local { batch }),
                effects: SourceEffects::default(),
            }],
        )?)
        .await?;
    Ok(next)
}
async fn one(db: &Database, sources: &SqliteSources) -> Result<Outcome> {
    let scheduler = SqliteScheduler::new(db.clone(), operation(182), holder(), Duration::from_secs(10))?;
    let lease = scheduler
        .due(now_millis(), 32)
        .await?
        .into_iter()
        .find(|l| sources.takes(&l.task.key))
        .ok_or("no prefix task")?;
    let outcome = sources.build_prefix(lease.clone()).await?;
    scheduler.finish(lease, outcome.clone()).await?;
    Ok(outcome)
}
fn prefixes(db: &Database) -> Result<Vec<String>> {
    Ok(db.read_now(|db| {
        Ok(db
            .prepare("SELECT prefix_digest FROM sync_stream_records ORDER BY sequence")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })?)
}

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("source-confirmation");
    fastctx_platform::ensure_private_dir(&dir)?;
    let path = dir.join("replica.sqlite3");
    let db = open(&path)?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(180), 1 << 24)?;
    let sources = writer(blocks)?;
    let primary = target(&custody(180, "conversation.timeline"), 180);
    let next = append(&sources, &primary, None, 0, 130).await?;
    let signed = confirmation(&primary)?;
    let context = primary.custody.context.clone();
    // Refusal before the transaction leaves neither evidence nor build progress.
    let mut wrong = signed.clone();
    wrong.proof.signer = SigningKey([20; 32]);
    assert!(matches!(sources.confirm_epoch(context.clone(), wrong).await, Err(Error::Denied(_))));
    assert_eq!(super::source_ingest::count(&db, "SELECT COUNT(*) FROM sync_epoch_descriptors")?, 0);
    sources.confirm_epoch(context.clone(), signed.clone()).await?;
    sources.confirm_epoch(context.clone(), signed.clone()).await?;
    assert_eq!(super::source_ingest::count(&db, "SELECT COUNT(*) FROM sync_epoch_descriptors")?, 1);
    assert!(matches!(sources.progress(primary.clone()).await?, Some(StreamProgress::LocalPending { .. })));
    assert!(matches!(one(&db, &sources).await?, Outcome::Retry { .. }));
    assert_eq!(
        super::source_ingest::count(&db, "SELECT COUNT(*) FROM sync_stream_records WHERE prefix_digest IS NOT NULL")?,
        64
    );
    // Ingesting while the prefix is behind remains legal and leaves work queued.
    let next = append(&sources, &primary, Some(next), 130, 1).await?;
    drop((sources, db));
    super::super::scheduler::closed(&path).await?;
    let db = open(&path)?;
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(180), 1 << 24)?;
    let sources = writer(blocks)?;
    assert!(matches!(one(&db, &sources).await?, Outcome::Retry { .. }));
    assert_eq!(
        super::source_ingest::count(&db, "SELECT COUNT(*) FROM sync_stream_records WHERE prefix_digest IS NOT NULL")?,
        128
    );
    assert!(matches!(one(&db, &sources).await?, Outcome::Completed { .. }));
    let Some(StreamProgress::Confirmed { cursor, prefix_digest, .. }) = sources.progress(primary.clone()).await? else {
        return Err("source confirmation did not finish".into());
    };
    assert_eq!(cursor.committed, Sequence(131));
    assert_eq!(prefixes(&db)?.last(), Some(&prefix_digest.as_str().to_owned()));
    // Once caught up, the same producer epoch advances prefixes inline.
    append(&sources, &primary, Some(next), 131, 2).await?;
    let Some(StreamProgress::Confirmed { cursor, .. }) = sources.progress(primary.clone()).await? else {
        return Err("continued source lost confirmation".into());
    };
    assert_eq!(cursor.committed, Sequence(133));
    assert_eq!(prefixes(&db)?.len(), 133);
    let expected = db.read_now(|db| {
        let mut digest = fastctx_types::sync::feed::stream_genesis(&signed.proof.id());
        let mut proofs = db.prepare("SELECT signed_bytes FROM sync_source_proofs ORDER BY first_sequence")?;
        let all = proofs.query_map([], |r| r.get::<_, Vec<u8>>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        let mut expected = Vec::new();
        for proof in all {
            let batch: SourceBatch = serde_json::from_slice(&proof).unwrap();
            for record in batch.records {
                digest = fastctx_types::sync::feed::stream_advance(&digest, &record).unwrap().1;
                expected.push(digest.as_str().to_owned());
            }
        }
        Ok(expected)
    })?;
    assert_eq!(prefixes(&db)?, expected);
    println!(
        "source confirmation: original evidence, refusal atomicity, 64-row pages, restart continuation, concurrent local append and inline confirmed prefixes passed"
    );
    Ok(())
}
