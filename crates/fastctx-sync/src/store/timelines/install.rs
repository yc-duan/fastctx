//! One bounded installation pass over a conversation's newly committed records:
//! decode them, plan the next TimelineV1 snapshot with T2, publish its pages and
//! proof blocks, then commit the head, index and watermarks together.
use super::{
    SqliteTimelines,
    commit::{self, Installed, Next},
    pages::Loaded,
    state::{self, Head, Record},
};
use crate::{
    blocks::codec,
    cursor::{LocalSequence, Revision, Sequence, StreamCursor},
    identity::{Digest, ObjectId, ObjectKey, OperationId},
    manifest::{BlockStore, Encoding, ManifestPage, ManifestRoot},
    scheduler::{Lease, Outcome},
    store::{
        Error,
        access::Custody,
        boundary::{self, Role},
        sql,
    },
    timeline::{
        TimelineChange,
        pages::{Budget, EncodedPage, Outcome as Planned},
        update::{Projection, SourceEvidence, Update, UpdatePlan, plan_update},
    },
};
use fastctx_types::sync::{
    conversation_timeline::{TimelineEvent, TimelineFact},
    metadata::ProofReference,
};
use std::collections::BTreeMap;

/// Records one pass reads; a plan that runs out of budget takes fewer.
const RECORDS: u32 = 64;
/// T2's cumulative preparation allowance for one pass.
const WORK_BYTES: usize = 64 * 1024 * 1024;

pub(super) async fn process(timelines: &SqliteTimelines, lease: Lease) -> Result<Outcome, Error> {
    let blocks = &timelines.blocks;
    let conversation = conversation(&lease)?;
    let holder = blocks.holder().clone();
    let read = conversation.clone();
    let snapshot = blocks
        .database()
        .read(move |db| {
            let head = state::head(db, &read)?;
            let Some(stream) = state::stream(db, &read)? else {
                return Ok(None);
            };
            if let Some(found) = &head
                && (found.through.stream != stream.stream || found.through.epoch != stream.epoch)
            {
                return Err(sql::invalid("the installed timeline follows another stream"));
            }
            let after = head.as_ref().map_or(Sequence(0), |found| found.through.committed);
            let records = state::records(db, &read, &stream, after, RECORDS)?;
            Ok(Some((head, stream, records, state::custody(db, &holder, &read)?)))
        })
        .await
        .map_err(|error| sql::error(error, blocks.worker().clone()))?;
    let Some((head, stream, records, custody)) = snapshot else {
        return Ok(Outcome::Completed { through: lease.task.wanted });
    };
    if records.is_empty() {
        return Ok(Outcome::Completed { through: lease.task.wanted });
    }
    let prepared = decode(timelines, &records)?;
    let projection = Revision(head.as_ref().map_or(0, |found| found.projection.0) + 1);
    let after = head.as_ref().map_or(Sequence(0), |found| found.through.committed);
    let operation = operation(&conversation, projection, after);
    let proofs = proofs(timelines, &custody, &operation, &prepared).await?;
    let previous = head.as_ref().and_then(|found| found.root.clone());
    let mut loaded = Loaded::default();
    let mut take = prepared.len();
    let plan = loop {
        let slice = &prepared[..take];
        let updates = updates(slice, &proofs);
        if updates.is_empty() {
            break None;
        }
        let through = cursor(&conversation, &stream, slice);
        let target = Projection { conversation: conversation.clone(), revision: projection, through };
        let budget = Budget { work_bytes: WORK_BYTES, max_pages: 4096, max_changes: updates.len() };
        match plan_update(&loaded, previous.as_ref(), &target, &updates, budget) {
            Ok(Planned::Ready(plan)) => break Some(plan),
            Ok(Planned::Pending { missing }) => loaded.load(blocks, &missing)?,
            Err(Error::Busy) if take > 1 => take /= 2,
            Err(error) => return Err(error),
        }
    };
    let slice = &prepared[..take];
    if let Some(plan) = &plan {
        publish(timelines, &custody, &operation, plan).await?;
    }
    let staged = plan.is_some() || !proofs.is_empty();
    let through = cursor(&conversation, &stream, slice);
    let caught_up = through.committed == stream.sequence;
    let next = next(&head, plan.as_ref(), projection, through, caught_up.then_some(stream.local));
    let changes = plan.map(|plan| plan.indexes).unwrap_or_default();
    let commit_custody = custody.clone();
    let commit_operation = operation.clone();
    let installed = blocks
        .database()
        .write(fastctx_types::sync::PAGE_BYTES as usize * 2, move |db| {
            let _boundary = boundary::enter_typed_write(Role::Projection);
            commit::install(db, &commit_custody, &commit_operation, &head, &next, &changes)
        })
        .await
        .map_err(|error| sql::error(error, operation.clone()))?;
    if matches!(installed, Installed::Committed) {
        timelines.wake.notify_waiters();
    }
    // The commit closed the staging either way; its pins go now rather than at
    // the next collection.
    if staged {
        while blocks.cancel_upload(custody.clone(), operation.clone(), 128).await? != 0 {}
    }
    Ok(match installed {
        Installed::Moved => Outcome::Retry { next_due_millis: 0, reason: "the timeline head moved".into() },
        Installed::Committed if !caught_up => {
            Outcome::Retry { next_due_millis: 0, reason: "more committed records to install".into() }
        }
        Installed::Committed => Outcome::Completed { through: lease.task.wanted },
    })
}

fn conversation(lease: &Lease) -> Result<ObjectKey, Error> {
    let key = &lease.task.key;
    if key.kind != "sync.projection" {
        return Err(Error::Invalid("the timeline installer received another handler's lease".into()));
    }
    let object = key
        .key
        .strip_prefix("derived.timeline/")
        .ok_or_else(|| Error::Invalid("the projection task is not a conversation timeline".into()))?;
    Ok(ObjectKey { scope: key.scope.clone(), object: ObjectId::try_from(object.to_owned()).map_err(Error::Invalid)? })
}

/// Each record with its canonical change, if it carries one. A branch switch
/// is installed alone, so the pass stops before it or right after it.
fn decode(timelines: &SqliteTimelines, records: &[Record]) -> Result<Vec<(Record, Option<TimelineChange>)>, Error> {
    let mut prepared = Vec::with_capacity(records.len());
    for record in records {
        let (bytes, _permit) = timelines
            .blocks
            .stored_block(&record.payload)?
            .ok_or_else(|| Error::Unavailable("a committed timeline record's payload is not stored here".into()))?;
        let event: TimelineEvent = serde_json::from_slice(&bytes)
            .map_err(|error| Error::Corrupt(format!("a committed timeline record is not a timeline event: {error}")))?;
        let change = match event.fact {
            TimelineFact::ItemRevision { change } => Some(change),
            _ => None,
        };
        let switch = matches!(change, Some(TimelineChange::SelectBranch { .. }));
        if switch && !prepared.is_empty() {
            break;
        }
        prepared.push((
            Record {
                reference: record.reference.clone(),
                payload: record.payload.clone(),
                proof: record.proof.clone(),
            },
            change,
        ));
        if switch {
            break;
        }
    }
    Ok(prepared)
}

/// Publishes each distinct source proof behind a Put once, under this pass.
async fn proofs(
    timelines: &SqliteTimelines,
    custody: &Custody,
    operation: &OperationId,
    prepared: &[(Record, Option<TimelineChange>)],
) -> Result<BTreeMap<Digest, ProofReference>, Error> {
    let blocks = &timelines.blocks;
    let mut proofs = BTreeMap::new();
    for (record, change) in prepared {
        if !matches!(change, Some(TimelineChange::Put { .. })) || proofs.contains_key(&record.proof) {
            continue;
        }
        let conversation = custody.object.clone();
        let id = record.proof.clone();
        let signed = blocks
            .database()
            .read_now(|db| state::proof(db, &conversation, &id))
            .map_err(|error| sql::error(error, operation.clone()))?;
        let bytes = serde_json::to_vec(&signed).map_err(|error| Error::Invalid(error.to_string()))?;
        let content = codec::encode(custody.object.scope.clone(), bytes, Encoding::Raw)?;
        let payload = content.representation.content.clone();
        blocks.put(custody.clone(), operation.clone(), content.representation, content.bytes).await?;
        proofs.insert(record.proof.clone(), ProofReference { id: record.proof.clone(), payload });
    }
    Ok(proofs)
}

fn updates(slice: &[(Record, Option<TimelineChange>)], proofs: &BTreeMap<Digest, ProofReference>) -> Vec<Update> {
    slice
        .iter()
        .filter_map(|(record, change)| {
            let change = change.clone()?;
            let evidence = matches!(change, TimelineChange::Put { .. }).then(|| SourceEvidence {
                record: record.reference.clone(),
                payload: record.payload.clone(),
                proof: proofs[&record.proof].clone(),
            });
            Some(Update { change, evidence })
        })
        .collect()
}

fn cursor(
    conversation: &ObjectKey,
    stream: &state::Stream,
    slice: &[(Record, Option<TimelineChange>)],
) -> StreamCursor {
    StreamCursor {
        scope: conversation.scope.clone(),
        stream: stream.stream.clone(),
        epoch: stream.epoch.clone(),
        committed: slice.last().map_or(Sequence(0), |(record, _)| record.reference.sequence),
    }
}

/// New pages go out children first, each payload before its page, so every
/// page is complete when stored and the root is complete when retained.
async fn publish(
    timelines: &SqliteTimelines,
    custody: &Custody,
    operation: &OperationId,
    plan: &UpdatePlan,
) -> Result<(), Error> {
    let blocks = &timelines.blocks;
    for page in children_first(plan)? {
        if let Some((block, bytes)) = &page.payload {
            let content = codec::encode(custody.object.scope.clone(), bytes.clone(), Encoding::Raw)?;
            if content.representation.content != *block {
                return Err(Error::Corrupt("a planned page payload differs from its identity".into()));
            }
            blocks.put(custody.clone(), operation.clone(), content.representation, content.bytes).await?;
        }
        blocks.put_manifest(custody.clone(), operation.clone(), page.manifest.clone()).await?;
    }
    Ok(())
}

/// Orders the plan's pages so each follows the new pages it references.
fn children_first(plan: &UpdatePlan) -> Result<Vec<&EncodedPage>, Error> {
    let pages: BTreeMap<&ManifestRoot, &EncodedPage> =
        plan.pages.iter().map(|page| (&page.manifest.root, page)).collect();
    let mut ordered = Vec::with_capacity(pages.len());
    let mut placed = std::collections::BTreeSet::new();
    let mut stack = vec![(&plan.root, false)];
    while let Some((root, expanded)) = stack.pop() {
        let Some(page) = pages.get(root) else { continue };
        if placed.contains(root) {
            continue;
        }
        if expanded {
            placed.insert(root);
            ordered.push(*page);
            continue;
        }
        stack.push((root, true));
        let children: Vec<ManifestRoot> = match serde_json::from_str(&page.manifest.page_json)
            .map_err(|error| Error::Corrupt(format!("a planned page is not a manifest page: {error}")))?
        {
            ManifestPage::Metadata { references, .. } => references
                .into_iter()
                .filter_map(|reference| match reference {
                    crate::manifest::ContentRef::Manifest { root } => Some(root),
                    crate::manifest::ContentRef::Single { .. } => None,
                })
                .collect(),
            ManifestPage::Branch { children } => children.into_iter().map(|child| child.root).collect(),
            ManifestPage::Blocks { .. } | ManifestPage::Directory { .. } => Vec::new(),
        };
        for child in children {
            if let Some((key, _)) = pages.get_key_value(&child)
                && !placed.contains(*key)
            {
                stack.push((key, false));
            }
        }
    }
    if ordered.len() != pages.len() {
        return Err(Error::Corrupt("a planned page is not reachable from the planned root".into()));
    }
    Ok(ordered)
}

fn next(
    head: &Option<Head>,
    plan: Option<&UpdatePlan>,
    projection: Revision,
    through: StreamCursor,
    applied: Option<LocalSequence>,
) -> Next {
    let previous = head.as_ref();
    let applied = applied.or(previous.map(|found| found.applied)).unwrap_or(LocalSequence(0));
    match plan {
        None => Next {
            projection: previous.map_or(Revision(0), |found| found.projection),
            through,
            root: previous.and_then(|found| found.root.clone()),
            indexed: previous.map_or(Some(Revision(0)), |found| found.indexed),
            applied,
        },
        Some(plan) => {
            let indexed = previous.is_none_or(|found| found.indexed == Some(found.projection));
            Next {
                projection,
                through,
                root: Some(plan.root.clone()),
                indexed: (indexed && !plan.rebuild_index).then_some(projection),
                applied,
            }
        }
    }
}

/// A pass is identified by the head it plans from: a pass retried after a crash
/// reopens its own staging even when it now takes more records, and the commit
/// that closes that staging releases whatever the crash left. Any committed
/// pass moves the head, so a closed staging is never reopened.
fn operation(conversation: &ObjectKey, projection: Revision, after: Sequence) -> OperationId {
    let digest = Digest::of(
        format!(
            "fastctx-sync/timeline-install/v2\0{}\0{}\0{}\0{}",
            conversation.scope.scope.as_str(),
            conversation.object.as_str(),
            projection.0,
            after.0
        )
        .as_bytes(),
    );
    OperationId::from_bytes(digest.bytes())
}
