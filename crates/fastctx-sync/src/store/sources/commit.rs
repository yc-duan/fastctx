//! One step commit: the primary checkpoint, every destination's facts and effects
//! and the step's own closure land in one FULL transaction, or none of them do.
//! Every check runs before the first write, so a refusal changes nothing.
use super::{SqliteSources, effects, streams};
use crate::{
    blocks::source_commit::{self, StepState},
    catalog::SourceBatch,
    cursor::{ByteCount, Holder, Revision},
    manifest::ContentRef,
    store::{
        Error,
        access::{AccessPolicy, Decision},
        boundary::{self, Role},
        ingest::{
            IngestFacts, IngestOperation, IngestReceipt, IngestStatus, SourceCheckpoint, SourceReceipt, SourceTarget,
        },
        local_head,
        roots::RootLease,
        source_authorize::{PreparedSourceChange, SourceAuthorizer, SourcePayload},
        source_step::SourceStep,
        sql,
    },
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::sync::proof::SignedSourceBatch;
use std::sync::Arc;

/// One destination's inputs, prepared outside the writer: its signed batch and
/// the payload bytes its authorizer judges.
struct Prepared {
    signed: Option<(SourceBatch, SignedSourceBatch)>,
    payloads: Vec<Vec<u8>>,
    authorizer: Arc<dyn SourceAuthorizer>,
}
enum Outcome {
    Committed(IngestReceipt),
    Moved(Revision),
    Denied(String),
    Unavailable,
}

/// The committed checkpoint as stored, with the hold on its private state.
struct Stored {
    generation: String,
    revision: Revision,
    committed_offset: ByteCount,
    parser_version: u16,
    state: Option<ContentRef>,
    lease: Option<RootLease>,
}
impl Stored {
    fn is(&self, expected: &SourceCheckpoint) -> bool {
        self.generation == expected.generation.as_str()
            && self.revision == expected.revision
            && self.committed_offset == expected.committed_offset
            && self.parser_version == expected.parser_version
            && self.state == expected.state
    }
}

impl SqliteSources {
    pub(super) async fn commit(&self, operation: IngestOperation) -> Result<IngestReceipt, Error> {
        let mut prepared = Vec::with_capacity(operation.batches().len());
        let mut permits = Vec::new();
        for batch in operation.batches() {
            let custody = &batch.target.target().custody;
            let authorizer = self.authorizer(custody)?.clone();
            let signed = match &batch.facts {
                None => None,
                Some(IngestFacts::Local { batch: facts }) => Some((facts.clone(), self.signer.source(facts)?)),
                Some(IngestFacts::Proven { .. }) => {
                    return Err(Error::Invalid(
                        "forwarded source evidence is accepted through the authority, not a local step".into(),
                    ));
                }
            };
            let mut payloads = Vec::new();
            for record in signed.iter().flat_map(|(facts, _)| &facts.records) {
                let (bytes, permit) = self.blocks.read_supplied(operation.step(), custody, &record.payload)?;
                payloads.push(bytes);
                permits.push(permit);
            }
            prepared.push(Prepared { signed, payloads, authorizer });
        }
        let holder = self.blocks.holder().clone();
        let policy = self.blocks.policy().clone();
        let id = operation.operation().clone();
        let now = now();
        let outcome = self
            .blocks
            .database()
            .write(fastctx_types::sync::PAGE_BYTES as usize * 2, move |db| {
                let _boundary = boundary::enter_typed_write(Role::Source);
                write(db, &holder, policy.as_ref(), &operation, &prepared, now)
            })
            .await
            .map_err(|error| sql::error(error, id))?;
        drop(permits);
        match outcome {
            Outcome::Committed(receipt) => Ok(receipt),
            Outcome::Moved(current) => Err(Error::LocalConflict { current }),
            Outcome::Denied(reason) => Err(Error::Denied(reason)),
            Outcome::Unavailable => Err(Error::Unavailable("the source authorizer has no current policy".into())),
        }
    }
}

fn write(
    db: &Connection,
    holder: &Holder,
    policy: &dyn AccessPolicy,
    operation: &IngestOperation,
    prepared: &[Prepared],
    now: i64,
) -> SqlResult<Outcome> {
    let step = operation.step();
    match source_commit::step_state(db, step)? {
        StepState::Consumed(receipt) => return Ok(Outcome::Committed(*receipt)),
        StepState::Cancelled => return Err(sql::invalid("the source step was cancelled")),
        StepState::Active => {}
    }
    let stored = stored(db, step)?;
    if stored.as_ref().map(|found| found.revision) != operation.expected().map(|expected| expected.revision) {
        return Ok(Outcome::Moved(stored.map_or(Revision(0), |found| found.revision)));
    }
    if let (Some(found), Some(expected)) = (&stored, operation.expected())
        && !found.is(expected)
    {
        return Err(sql::invalid("the expected checkpoint differs from the committed one"));
    }
    for (batch, inputs) in operation.batches().iter().zip(prepared) {
        let target = batch.target.target();
        source_commit::authorize(db, holder, policy, &target.custody)?;
        source_commit::require_bound(db, step, target)?;
        if let Some((facts, _)) = &inputs.signed {
            streams::check(db, target, facts)?;
        }
        effects::check(db, target, &batch.effects)?;
        let payloads: Vec<_> = inputs
            .signed
            .iter()
            .flat_map(|(facts, _)| &facts.records)
            .zip(&inputs.payloads)
            .map(|(record, bytes)| SourcePayload { record, bytes })
            .collect();
        let change = PreparedSourceChange {
            target,
            batch: inputs.signed.as_ref().map(|(facts, _)| facts),
            payloads: &payloads,
            checkpoint: operation.next(),
            effects: &batch.effects,
        };
        match inputs.authorizer.authorize(change) {
            Decision::Allowed { .. } => {}
            Decision::Denied { reason, .. } => return Ok(Outcome::Denied(reason)),
            Decision::Unavailable => return Ok(Outcome::Unavailable),
        }
    }
    let mut leases = Vec::new();
    advance(db, holder, step, stored, operation.next(), now, &mut leases)?;
    let local = local_head::advance(db)?;
    let mut sources = Vec::with_capacity(prepared.len());
    for (batch, inputs) in operation.batches().iter().zip(prepared) {
        let target = batch.target.target();
        let status = match &inputs.signed {
            None => IngestStatus::CheckpointOnly,
            Some((facts, signed)) => {
                IngestStatus::Applied { progress: streams::append(db, step, target, facts, signed)? }
            }
        };
        effects::apply(db, holder, step, target, &batch.effects, local, now, &mut leases)?;
        sources.push(receipt(target, operation.next(), status));
    }
    let receipt = IngestReceipt { operation: step.operation().clone(), local, sources, leases };
    source_commit::consume(db, step, &receipt)?;
    Ok(Outcome::Committed(receipt))
}

fn receipt(target: &SourceTarget, next: &SourceCheckpoint, status: IngestStatus) -> SourceReceipt {
    SourceReceipt {
        source: target.source.clone(),
        scope: target.custody.object.scope.clone(),
        checkpoint: next.revision,
        status,
    }
}

fn stored(db: &Connection, step: &SourceStep) -> SqlResult<Option<Stored>> {
    let primary = step.primary();
    let scope = &primary.custody.object.scope;
    let row = db
        .prepare_cached("SELECT generation,revision,committed_offset,parser_version,state,state_lease FROM sync_sources WHERE hub=?1 AND scope=?2 AND source=?3")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), primary.source.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                Revision(sql::counter(row, 1)?),
                ByteCount(sql::counter(row, 2)?),
                row.get::<_, u16>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
            ))
        })
        .optional()?;
    row.map(|(generation, revision, committed_offset, parser_version, state, lease)| {
        Ok(Stored {
            generation,
            revision,
            committed_offset,
            parser_version,
            state: state.map(|bytes| decode(&bytes)).transpose()?,
            lease: lease.map(|bytes| decode(&bytes)).transpose()?,
        })
    })
    .transpose()
}

/// Writes the next checkpoint. A changed state root is held under this step and
/// the superseded hold is released; an unchanged root keeps its existing hold.
fn advance(
    db: &Connection,
    holder: &Holder,
    step: &SourceStep,
    stored: Option<Stored>,
    next: &SourceCheckpoint,
    now: i64,
    leases: &mut Vec<RootLease>,
) -> SqlResult<()> {
    let primary = step.primary();
    let previous = stored.and_then(|found| found.state.zip(found.lease));
    let lease = match (&next.state, &previous) {
        (None, _) => None,
        (Some(root), Some((held, lease))) if held == root => Some(lease.clone()),
        (Some(root), _) => {
            let lease = source_commit::hold_checkpoint(db, step, root, now)?;
            leases.push(lease.clone());
            Some(lease)
        }
    };
    if let Some((_, old)) = &previous
        && lease.as_ref().is_none_or(|kept| kept.id != old.id)
    {
        source_commit::release(db, holder, &primary.custody, old)?;
    }
    let scope = &primary.custody.object.scope;
    db.prepare_cached("INSERT INTO sync_sources(hub,scope,source,generation,revision,committed_offset,parser_version,state,state_lease) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(hub,scope,source) DO UPDATE SET generation=excluded.generation,revision=excluded.revision,committed_offset=excluded.committed_offset,parser_version=excluded.parser_version,state=excluded.state,state_lease=excluded.state_lease")?
        .execute(params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            primary.source.as_str(),
            next.generation.as_str(),
            next.revision.0.to_be_bytes().as_slice(),
            next.committed_offset.0.to_be_bytes().as_slice(),
            next.parser_version,
            next.state.as_ref().map(encode).transpose()?,
            lease.as_ref().map(encode).transpose()?
        ])?;
    Ok(())
}

pub(super) fn encode(value: &impl serde::Serialize) -> SqlResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| sql::invalid(error.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> SqlResult<T> {
    serde_json::from_slice(bytes).map_err(|error| sql::invalid(error.to_string()))
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}
