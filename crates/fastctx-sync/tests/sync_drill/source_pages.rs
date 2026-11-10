//! A producer's committed proofs must fit the hub's real JSON stream reader.
use super::*;
use fastctx_sync::{
    authority::{Peer, Service},
    catalog::RecordRef,
    proof::ProofSigner,
    replicate::Upstream,
    timeline::{StreamPosition, StreamRead},
    transfer::Message,
};

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("source-pages");
    let (node_dir, hub_dir) = (dir.join("node"), dir.join("hub"));
    fastctx_platform::ensure_private_dir(&node_dir)?;
    fastctx_platform::ensure_private_dir(&hub_dir)?;
    let object = conversation(42);
    let (db, _, sources, target) = node(&node_dir, &object)?;
    let step = sources.begin_step(target.clone(), None, HostGeneration::from_bytes([42; 32])).await?;
    let bound = sources.bind_target(step.clone(), target.clone()).await?;
    let mut records = Vec::new();
    for sequence in 1..=2 {
        let encoded =
            codec::encode(object.scope.clone(), format!("source fact {sequence}").into_bytes(), Encoding::Raw)?;
        let payload = encoded.representation.content.clone();
        sources.put_block(bound.clone(), encoded.representation, encoded.bytes).await?;
        records.push(SourceRecord {
            sequence: Sequence(sequence),
            operation: operation(sequence as u8),
            kind: KIND.into(),
            schema: 1,
            payload,
            roots: Vec::new(),
            causes: (1..=350)
                .map(|sequence| RecordRef {
                    stream: StreamId::from_bytes([80; 32]),
                    epoch: ProducerEpoch::from_bytes([81; 32]),
                    sequence: Sequence(sequence),
                })
                .collect(),
        });
    }
    let batch = SourceBatch {
        format: 1,
        scope: object.scope.clone(),
        stream: target.stream.clone(),
        producer: target.producer.clone(),
        epoch: target.epoch.clone(),
        operation: step.operation().clone(),
        kind: KIND.into(),
        schema: 1,
        after: Sequence(0),
        through: Sequence(2),
        records: records.clone(),
    };
    let next = SourceCheckpoint {
        source: target.source.clone(),
        generation: step.generation().clone(),
        revision: step.revision(),
        committed_offset: ByteCount(2),
        parser_version: 1,
        state: None,
    };
    let request = IngestOperation::new(
        step,
        None,
        next,
        vec![IngestBatch {
            target: bound,
            facts: Some(IngestFacts::Local { batch: batch.clone() }),
            effects: SourceEffects::default(),
        }],
    )?;
    let receipt = sources.ingest(request.clone()).await?;
    assert_eq!(serde_json::to_vec(&sources.ingest(request).await?)?, serde_json::to_vec(&receipt)?);
    let stored = db.read_now(|db| {
        Ok(db
            .prepare("SELECT signed_bytes,proof FROM sync_source_proofs ORDER BY first_sequence")?
            .query_map([], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })?;
    assert!(stored.len() > 1, "the page budget must split this step before signing");
    let gate = Arc::new(Gate(AtomicBool::new(false)));
    let (_, blocks) = open_hub(&hub_dir, false)?;
    let (run, authority) = authority(&blocks, 1, &gate).await?;
    let directory = Arc::new(SqliteDirectory::new(blocks, run, Arc::new(Signer(true))));
    describe(&directory, &object, 214).await?;
    let upstream = super::super::directory::Loopback {
        service: Arc::new(Service::new(directory).with_sources(Arc::new(authority.clone()))),
        peer: Peer { device: DeviceId::from_bytes([3; 32]), accounts: vec![account().id] },
    };
    let mut received = Vec::new();
    for (data, id) in stored {
        let part: SourceBatch = serde_json::from_slice(&data)?;
        let signed = Signer(false).source(&part)?;
        assert_eq!(signed.proof.data.0, data);
        assert_eq!(signed.proof.id().as_str(), id);
        upload_payloads(&authority, &part).await?;
        accepted(&authority, &part).await?;
        let answer = upstream
            .exchange(Message::StreamRange {
                holding: holding(&object),
                request: StreamRead {
                    stream: StreamCursor {
                        scope: part.scope.clone(),
                        stream: part.stream.clone(),
                        epoch: part.epoch.clone(),
                        committed: part.through,
                    },
                    position: StreamPosition::After(part.after),
                    through: Some(part.through),
                    max_records: 256,
                    max_bytes: fastctx_types::sync::PAGE_BYTES,
                },
            })
            .await?;
        assert!(serde_json::to_vec(&answer)?.len() <= fastctx_types::sync::PAGE_BYTES as usize);
        let Message::StreamPage { page, .. } = answer else { return Err("missing source page".into()) };
        assert_eq!(page.proofs, vec![signed]);
        received.extend(page.records.into_iter().map(|r| r.record));
    }
    assert_eq!(received, records);
    println!(
        "source pages: one atomic step splits before signing and every original proof reads back over JSON loopback"
    );
    Ok(())
}
