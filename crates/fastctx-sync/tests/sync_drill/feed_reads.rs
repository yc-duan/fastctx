//! A discovered scope's feed and stream cross the same JSON transport as discovery.
use super::*;
use fastctx_sync::{
    catalog::{FeedEntry, HeadEvidence},
    cursor::{ScopeCursor, StreamCursor},
    proof::EpochLookup,
    store::ingest::StreamProgress,
    timeline::{Direction, StreamPosition, StreamRead},
    transfer::ViewDefinition,
};

fn changes(holding: &HoldingPeriod, after: &ScopeCursor, max_bytes: u32) -> Message {
    Message::Changes { holding: holding.clone(), after: after.clone(), view: None, max_bytes }
}
fn range(holding: &HoldingPeriod, stream: &StreamCursor, position: StreamPosition, max_records: u32) -> Message {
    Message::StreamRange {
        holding: holding.clone(),
        request: StreamRead {
            stream: stream.clone(),
            position,
            through: Some(stream.committed),
            max_records,
            max_bytes: 256 * 1024,
        },
    }
}
fn bytes(message: &Message) -> usize {
    serde_json::to_vec(message).unwrap().len()
}

pub(super) async fn run(
    upstream: &Loopback,
    identity: &HubSigningIdentity,
    end: &ScopeCursor,
    deleted: &ScopeKey,
) -> Result<()> {
    let holding = HoldingPeriod {
        scope: end.scope.clone(),
        holder: Holder::Device { device: upstream.peer.device.clone() },
        acl_revision: end.acl_revision,
        publication_epoch: Revision(17),
    };
    let beginning =
        ScopeCursor { through: Sequence(0), prefix_digest: Some(feed::genesis(&end.scope, &end.epoch)), ..end.clone() };
    assert!(beginning.view.is_some());
    let all = upstream.exchange(changes(&holding, &beginning, 256 * 1024)).await?;
    let Message::ChangesPage { page, .. } = all.value() else {
        return Err("missing changes page".into());
    };
    assert_eq!(page.entries.len(), 4);
    assert_eq!(&page.through, end);
    assert!(!page.more);
    let mut one = page.clone();
    one.entries.truncate(1);
    let FeedEntry::Changed { change, prefix_digest } = &one.entries[0] else {
        return Err("expected a visible change".into());
    };
    one.through.through = change.sequence;
    one.through.prefix_digest = Some(prefix_digest.clone());
    one.more = true;
    let budget = (bytes(&Message::ChangesPage { holding: holding.clone(), page: one }) + 32) as u32;
    let mut after = beginning.clone();
    let mut count = 0;
    let mut last_head = None;
    loop {
        let request = changes(&holding, &after, budget);
        let answer = upstream.exchange(request.clone()).await?;
        assert_eq!(upstream.exchange(request).await?.value(), answer.value());
        assert!(bytes(&answer) <= budget as usize);
        let Message::ChangesPage { holding: echoed, page } = answer.value() else {
            return Err("expected changes".into());
        };
        assert_eq!(echoed, &holding);
        assert_eq!(page.after, after);
        assert_eq!(page.entries.len(), 1);
        let mut prefix = after.prefix_digest.clone().unwrap();
        for entry in &page.entries {
            let FeedEntry::Changed { change, prefix_digest } = entry else {
                return Err("unexpected skipped change".into());
            };
            count += 1;
            assert_eq!(change.sequence, Sequence(count));
            prefix = feed::advance(&prefix, change)?.1;
            assert_eq!(&prefix, prefix_digest);
            last_head = Some(change.head.clone());
        }
        assert_eq!(page.through.prefix_digest, Some(prefix));
        after = page.through.clone();
        if !page.more {
            break;
        }
    }
    assert_eq!(&after, end);
    assert_eq!(count, 4);
    let head = last_head.unwrap();
    let HeadEvidence::Stream { epoch: epoch_id, through, prefix_digest: final_prefix } = head.evidence else {
        return Err("directory is not a stream".into());
    };
    let request = Message::GetEpoch { scope: end.scope.clone(), lookup: EpochLookup::Proof { id: epoch_id.clone() } };
    let epoch_reply = upstream.exchange(request.clone()).await?;
    let Message::Epoch { descriptor } = epoch_reply.value() else {
        return Err("missing epoch".into());
    };
    let epoch = Verifier.epoch(descriptor, identity)?;
    assert_eq!(descriptor.proof.id(), epoch_id);
    let by_stream = Message::GetEpoch {
        scope: end.scope.clone(),
        lookup: EpochLookup::Stream { stream: epoch.stream.clone(), epoch: epoch.epoch.clone() },
    };
    assert_eq!(upstream.exchange(by_stream).await?.value(), &Message::Epoch { descriptor: descriptor.clone() });
    let stream = StreamCursor {
        scope: end.scope.clone(),
        stream: epoch.stream.clone(),
        epoch: epoch.epoch.clone(),
        committed: through,
    };
    let mut position = Sequence(0);
    let mut prefix = feed::stream_genesis(&epoch_id);
    let mut sequences = Vec::new();
    loop {
        let answer = upstream.exchange(range(&holding, &stream, StreamPosition::After(position), 1)).await?;
        assert!(bytes(&answer) <= 256 * 1024);
        let Message::StreamPage { holding: echoed, page } = answer.value() else {
            return Err("missing stream page".into());
        };
        assert_eq!(echoed, &holding);
        assert_eq!(page.direction, Direction::Forward);
        assert_eq!(
            page.fence,
            StreamProgress::Confirmed {
                cursor: stream.clone(),
                epoch_proof: epoch_id.clone(),
                prefix_digest: final_prefix.clone()
            }
        );
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.proofs.len(), 1);
        let proof = &page.proofs[0];
        let batch = Verifier.source(proof, &epoch)?;
        for record in &page.records {
            assert_eq!(record.batch_proof, proof.proof.id());
            assert_eq!(&record.record, &batch.records[(record.record.sequence.0 - batch.after.0 - 1) as usize]);
            assert_eq!(record.record.sequence, Sequence(position.0 + 1));
            prefix = feed::stream_advance(&prefix, &record.record)?.1;
            position = record.record.sequence;
            sequences.push(position);
        }
        match page.next {
            Some(next) => assert_eq!(next, position),
            None => break,
        }
    }
    assert_eq!(position, through);
    assert_eq!(prefix, final_prefix);
    let mut anchor = StreamPosition::Latest;
    let mut reverse = Vec::new();
    loop {
        let answer = upstream.exchange(range(&holding, &stream, anchor, 2)).await?;
        let Message::StreamPage { page, .. } = answer.value() else {
            return Err("missing backwards page".into());
        };
        assert_eq!(page.direction, Direction::Backward);
        reverse.splice(0..0, page.records.iter().map(|value| value.record.sequence));
        match page.next {
            Some(next) => anchor = StreamPosition::Before(next),
            None => break,
        }
    }
    assert_eq!(reverse, sequences);

    // The view covers filtered sequence numbers; changing it never silently
    // reuses the old cursor. A forged anchor only resets this request.
    let view = ViewDefinition {
        scope: end.scope.clone(),
        acl_revision: end.acl_revision,
        projection_schema: 1,
        kinds: Vec::new(),
    };
    let hidden = ScopeCursor { view: Some(view.id()?), ..beginning.clone() };
    let filtered = upstream
        .exchange(Message::Changes { holding: holding.clone(), after: hidden, view: Some(view), max_bytes: 256 * 1024 })
        .await?;
    let Message::ChangesPage { page, .. } = filtered.value() else {
        return Err("missing filtered page".into());
    };
    assert!(matches!(page.entries.as_slice(), [FeedEntry::Skipped { after: Sequence(0), through: Sequence(4), .. }]));
    assert_eq!(page.through.prefix_digest, end.prefix_digest);
    let mut wrong = beginning.clone();
    wrong.prefix_digest = Some(Digest::from_bytes([199; 32]));
    assert!(
        matches!(upstream.exchange(changes(&holding, &wrong, 256 * 1024)).await?.value(), Message::Reset { epoch, .. } if epoch == &end.epoch)
    );
    assert!(matches!(upstream.exchange(changes(&holding, &wrong, 1)).await, Err(Error::Invalid(_))));
    wrong = beginning.clone();
    wrong.view = None;
    assert!(matches!(upstream.exchange(changes(&holding, &wrong, 256 * 1024)).await?.value(), Message::Reset { .. }));
    let answer = upstream.exchange(changes(&holding, end, 256 * 1024)).await?;
    let Message::ChangesPage { page, .. } = answer.value() else {
        return Err("valid cursor was disturbed".into());
    };
    assert!(page.entries.is_empty());
    assert_eq!(&page.through, end);

    let stranger = Loopback {
        service: upstream.service.clone(),
        budget: upstream.budget.clone(),
        peer: Peer { device: upstream.peer.device.clone(), accounts: vec![AccountId::from_bytes([201; 32])] },
    };
    for request in
        [changes(&holding, &beginning, 256 * 1024), range(&holding, &stream, StreamPosition::Latest, 1), request]
    {
        assert!(matches!(stranger.exchange(request).await, Err(Error::Denied(_))));
    }
    let absent = Message::GetEpoch {
        scope: end.scope.clone(),
        lookup: EpochLookup::Proof { id: Digest::from_bytes([200; 32]) },
    };
    assert!(matches!(upstream.exchange(absent).await, Err(Error::Unavailable(_))));
    let revoked = Message::GetEpoch { scope: deleted.clone(), lookup: EpochLookup::Proof { id: epoch_id } };
    assert!(matches!(upstream.exchange(revoked).await, Err(Error::Revoked)));
    println!(
        "hub reads: JSON discovery handoff, byte-bounded changes, explicit skips, anchor reset, forward/backward source pages, original proofs and owner isolation passed"
    );
    Ok(())
}
