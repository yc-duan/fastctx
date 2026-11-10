//! Local stream pages retain original proofs within their complete JSON budget.
use super::*;
use fastctx_types::sync::PAGE_BYTES;

pub(super) async fn run(sources: &SqliteSources, timelines: &SqliteTimelines, context: &AccessContext) -> Result<()> {
    let primary = target(&custody(239, "conversation.timeline"), 239);
    let step = sources.begin_step(primary.clone(), None, HostGeneration::from_bytes([239; 32])).await?;
    let bound = sources.bind_target(step.clone(), primary.clone()).await?;
    let mut records = Vec::new();
    for number in 1..=2 {
        let sequence = Sequence(number);
        let operation = OperationId::from_bytes([number as u8; 32]);
        let event = TimelineEvent {
            evidence: Evidence {
                record: RecordRef { stream: primary.stream.clone(), epoch: primary.epoch.clone(), sequence },
                operation: operation.clone(),
                accepted: sequence,
            },
            fact: TimelineFact::ItemRevision {
                change: TimelineChange::Retract {
                    item: primary.custody.object.object.clone(),
                    revision: Revision(number),
                },
            },
        };
        let payload = block(sources, &bound, &serde_json::to_vec(&event)?).await?;
        records.push(SourceRecord {
            sequence,
            operation,
            kind: "conversation.timeline".into(),
            schema: 1,
            payload,
            roots: Vec::new(),
            causes: (1..=350)
                .map(|number| RecordRef {
                    stream: StreamId::from_bytes([80; 32]),
                    epoch: ProducerEpoch::from_bytes([81; 32]),
                    sequence: Sequence(number),
                })
                .collect(),
        });
    }
    let batch = SourceBatch {
        format: 1,
        scope: scope(),
        stream: primary.stream.clone(),
        epoch: primary.epoch.clone(),
        producer: primary.producer.clone(),
        operation: step.operation().clone(),
        kind: "conversation.timeline".into(),
        schema: 1,
        after: Sequence(0),
        through: Sequence(2),
        records: records.clone(),
    };
    sources
        .ingest(IngestOperation::new(
            step.clone(),
            None,
            checkpoint(&primary, &step, 2, None),
            vec![IngestBatch {
                target: bound,
                facts: Some(IngestFacts::Local { batch }),
                effects: SourceEffects::default(),
            }],
        )?)
        .await?;
    let request = |position, bytes| StreamRead {
        stream: StreamCursor {
            scope: scope(),
            stream: primary.stream.clone(),
            epoch: primary.epoch.clone(),
            committed: Sequence(0),
        },
        position,
        through: Some(Sequence(2)),
        max_records: 256,
        max_bytes: bytes,
    };
    let wide = timelines.stream(context.clone(), request(StreamPosition::After(Sequence(0)), u32::MAX)).await?;
    assert_eq!((wide.records.len(), wide.proofs.len()), (2, 2));
    let old_estimate = wide
        .records
        .iter()
        .map(|record| serde_json::to_vec(&record.record).map(|bytes| bytes.len()))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .sum::<usize>()
        + wide.proofs.iter().map(|proof| proof.proof.data.0.len()).sum::<usize>();
    let wide_bytes = serde_json::to_vec(&wide)?.len();
    assert!(
        old_estimate <= PAGE_BYTES as usize && wide_bytes > PAGE_BYTES as usize,
        "fixture must expose the raw-proof budget undercount"
    );
    let mut received = Vec::new();
    let mut after = Sequence(0);
    let mut first = None;
    loop {
        let page = timelines.stream(context.clone(), request(StreamPosition::After(after), PAGE_BYTES)).await?;
        assert!(serde_json::to_vec(&page)?.len() <= PAGE_BYTES as usize);
        assert_eq!((page.records.len(), page.proofs.len()), (1, 1));
        assert!(page.records[0].record.sequence > after);
        received.push(page.records[0].record.clone());
        if first.is_none() {
            first = Some(page.clone());
        }
        let Some(next) = page.next else { break };
        after = next;
    }
    assert_eq!(received, records);
    let mut first = first.unwrap();
    first.next = Some(Sequence(u64::MAX));
    let exact = u32::try_from(serde_json::to_vec(&first)?.len())?;
    let page = timelines.stream(context.clone(), request(StreamPosition::After(Sequence(0)), exact)).await?;
    assert_eq!(page.records.len(), 1);
    assert!(serde_json::to_vec(&page)?.len() <= exact as usize);
    assert!(matches!(
        timelines.stream(context.clone(), request(StreamPosition::After(Sequence(0)), exact - 1)).await,
        Err(fastctx_sync::store::Error::Invalid(_))
    ));
    let latest = timelines.stream(context.clone(), request(StreamPosition::Latest, PAGE_BYTES)).await?;
    assert!(serde_json::to_vec(&latest)?.len() <= PAGE_BYTES as usize);
    assert_eq!(latest.records[0].record.sequence, Sequence(2));
    let prior =
        timelines.stream(context.clone(), request(StreamPosition::Before(latest.next.unwrap()), PAGE_BYTES)).await?;
    assert!(serde_json::to_vec(&prior)?.len() <= PAGE_BYTES as usize);
    assert_eq!(prior.records[0].record.sequence, Sequence(1));
    assert!(prior.next.is_none());
    println!(
        "local stream JSON budget: raw estimate {old_estimate}, complete two-proof page {wide_bytes}, one-record bound {exact}; forward/backward continuation and exact budget passed"
    );
    Ok(())
}
