//! Source roots finish their bounded checks before the atomic checkpoint handoff.
use super::{
    SqliteBlocks,
    supply::{self, SupplyBudget, SupplyOwner, SupplyProgress, SupplyRequest, SupplyTicket},
};
use crate::{
    identity::RootLeaseId,
    manifest::ContentRef,
    store::{
        Error,
        ingest::{IngestFacts, IngestOperation, SourceTarget},
        roots::{RootChange, RootLease, RootUse, SupplyBasis},
        sql,
    },
};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub(crate) struct SourceSupplies {
    records: BTreeMap<Vec<u8>, SupplyTicket>,
    pub checkpoint: Option<SupplyTicket>,
    uses: BTreeMap<RootLeaseId, SupplyTicket>,
}
impl SourceSupplies {
    pub(crate) fn record(&self, target: &SourceTarget, root: &ContentRef) -> fastctx_storage::Result<&SupplyTicket> {
        self.records
            .get(&record_key(target, root)?)
            .ok_or_else(|| sql::invalid("source record was not prepared for adoption"))
    }
    pub(crate) fn usage(&self, id: &RootLeaseId) -> fastctx_storage::Result<&SupplyTicket> {
        self.uses.get(id).ok_or_else(|| sql::invalid("source root use was not prepared for acquisition"))
    }
}
fn record_key(target: &SourceTarget, root: &ContentRef) -> fastctx_storage::Result<Vec<u8>> {
    serde_json::to_vec(&(&target.custody.object, target.custody.kind.registration().id, &target.source, root))
        .map_err(|e| sql::invalid(e.to_string()))
}

/// What one prepared root is for, so its ticket lands where the commit looks.
enum Slot {
    Checkpoint,
    Record(Vec<u8>),
    Use(RootLeaseId),
}

pub(crate) async fn prepare(blocks: &SqliteBlocks, operation: &IngestOperation) -> Result<SourceSupplies, Error> {
    let step = operation.step();
    let primary = step.primary();
    let previous: Option<RootLease> = blocks
        .database()
        .read_now(|db| {
            let scope = &primary.custody.object.scope;
            let encoded = db
                .query_row(
                    "SELECT state_lease FROM sync_sources WHERE hub=?1 AND scope=?2 AND source=?3",
                    params![scope.hub.to_string(), scope.scope.as_str(), primary.source.as_str()],
                    |row| row.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()?
                .flatten();
            encoded.map(|bytes| serde_json::from_slice(&bytes).map_err(|e| sql::invalid(e.to_string()))).transpose()
        })
        .map_err(|e| sql::error(e, step.operation().clone()))?;
    let bases: Vec<_> = previous.iter().cloned().map(|lease| SupplyBasis::Held { lease: Box::new(lease) }).collect();
    let mut slots = Vec::new();
    let mut requests = Vec::new();
    if let Some(root) = &operation.next().state
        && previous.as_ref().is_none_or(|old| &old.root != root)
    {
        slots.push(Slot::Checkpoint);
        requests.push(SupplyRequest {
            operation: step.operation().clone(),
            custody: primary.custody.clone(),
            root: root.clone(),
            bases: bases.clone(),
            owner: SupplyOwner::Use { usage: RootUse::Checkpoint { source: primary.source.clone() } },
        });
    }
    let mut records = std::collections::BTreeSet::new();
    for batch in operation.batches() {
        let target = batch.target.target();
        if let Some(IngestFacts::Local { batch: facts }) = &batch.facts {
            for root in facts.records.iter().flat_map(|record| {
                std::iter::once(ContentRef::from(record.payload.clone())).chain(record.roots.iter().cloned())
            }) {
                let key = record_key(target, &root).map_err(|e| sql::error(e, step.operation().clone()))?;
                if !records.insert(key.clone()) {
                    continue;
                }
                let basis = if target.source == primary.source
                    && target.custody.object == primary.custody.object
                    && target.custody.kind.registration().id == primary.custody.kind.registration().id
                {
                    bases.clone()
                } else {
                    Vec::new()
                };
                slots.push(Slot::Record(key));
                requests.push(SupplyRequest {
                    operation: step.operation().clone(),
                    custody: target.custody.clone(),
                    root,
                    bases: basis,
                    owner: SupplyOwner::Source { source: target.source.clone() },
                });
            }
        }
        for change in batch.effects.roots() {
            let RootChange::Acquire(request) = change else {
                continue;
            };
            if matches!(request.usage, RootUse::Timeline { .. }) {
                continue;
            }
            slots.push(Slot::Use(request.lease.clone()));
            requests.push(SupplyRequest {
                operation: step.operation().clone(),
                custody: request.custody.clone(),
                root: request.root.clone(),
                bases: request.bases.clone(),
                owner: SupplyOwner::Use { usage: request.usage.clone() },
            });
        }
    }
    let tickets = check_step(blocks, requests, Instant::now() + Duration::from_secs(5)).await?;
    let mut supplies = SourceSupplies { records: BTreeMap::new(), checkpoint: None, uses: BTreeMap::new() };
    for (slot, ticket) in slots.into_iter().zip(tickets) {
        match slot {
            Slot::Checkpoint => supplies.checkpoint = Some(ticket),
            Slot::Record(key) => {
                supplies.records.insert(key, ticket);
            }
            Slot::Use(lease) => {
                supplies.uses.insert(lease, ticket);
            }
        }
    }
    Ok(supplies)
}

/// How long the checks of one step keep the writer before others take a turn.
const SHARE: Duration = Duration::from_millis(2);

enum Check {
    Asked(Box<SupplyRequest>),
    Walking(SupplyTicket),
    Ready(SupplyTicket),
    Rejected(String),
    Blocked,
}

/// Checks every root one source step is about to commit, in the order given.
/// The checks share writer transactions instead of taking two each, and none
/// waits for the disk: they are the step's own working state, which the
/// step's commit reads in its durable transaction and a restarted step asks
/// for again.
async fn check_step(
    blocks: &SqliteBlocks,
    requests: Vec<SupplyRequest>,
    deadline: Instant,
) -> Result<Vec<SupplyTicket>, Error> {
    let Some(operation) = requests.first().map(|request| request.operation.clone()) else {
        return Ok(Vec::new());
    };
    let bytes = (2 * 1024 * 1024 + requests.len() * 4096).min(8 * 1024 * 1024);
    let mut checks: Vec<Check> = requests.into_iter().map(|request| Check::Asked(Box::new(request))).collect();
    loop {
        let holder = blocks.holder().clone();
        let policy = blocks.policy().clone();
        let mut sharing = std::mem::take(&mut checks);
        checks = blocks
            .database()
            .write_relaxed(bytes, move |db| {
                let writer = supply::writer(db, &holder, policy.as_ref(), now())?;
                let started = Instant::now();
                for check in &mut sharing {
                    if let Check::Asked(request) = check {
                        let ticket = writer.begin(request)?;
                        *check = Check::Walking(ticket);
                    }
                    let Check::Walking(ticket) = check else {
                        continue;
                    };
                    match writer.step(ticket, SupplyBudget::default())? {
                        SupplyProgress::Ready | SupplyProgress::Consumed => *check = Check::Ready(ticket.clone()),
                        SupplyProgress::Pending { missing: None } => {}
                        SupplyProgress::Rejected { node, reason } => {
                            *check = Check::Rejected(format!(
                                "source content {} has no qualified supply: {reason:?}",
                                node.id().digest.as_str()
                            ));
                        }
                        _ => *check = Check::Blocked,
                    }
                    if started.elapsed() >= SHARE {
                        break;
                    }
                }
                Ok(sharing)
            })
            .await
            .map_err(|e| sql::error(e, operation.clone()))?;
        let mut walking = false;
        for check in &mut checks {
            match check {
                Check::Rejected(reason) => return Err(Error::Invalid(std::mem::take(reason))),
                Check::Blocked => return Err(Error::Busy),
                Check::Asked(_) | Check::Walking(_) => walking = true,
                Check::Ready(_) => {}
            }
        }
        if !walking {
            return Ok(checks
                .into_iter()
                .filter_map(|check| match check {
                    Check::Ready(ticket) => Some(ticket),
                    _ => None,
                })
                .collect());
        }
        if Instant::now() >= deadline {
            return Err(Error::Busy);
        }
    }
}

pub(super) async fn check(
    blocks: &SqliteBlocks,
    request: SupplyRequest,
    deadline: Instant,
) -> Result<SupplyTicket, Error> {
    let operation = request.operation.clone();
    let holder = blocks.holder().clone();
    let policy = blocks.policy().clone();
    let ticket = blocks
        .database()
        .write(fastctx_types::sync::PAGE_BYTES as usize, move |db| {
            supply::writer(db, &holder, policy.as_ref(), now())?.begin(&request)
        })
        .await
        .map_err(|e| sql::error(e, operation.clone()))?;
    loop {
        let holder = blocks.holder().clone();
        let policy = blocks.policy().clone();
        let current = ticket.clone();
        let progress = blocks
            .database()
            .write(2 * 1024 * 1024, move |db| {
                supply::writer(db, &holder, policy.as_ref(), now())?.step(&current, SupplyBudget::default())
            })
            .await
            .map_err(|e| sql::error(e, operation.clone()))?;
        match progress {
            SupplyProgress::Ready | SupplyProgress::Consumed => return Ok(ticket),
            SupplyProgress::Pending { missing: None } if Instant::now() < deadline => {}
            SupplyProgress::Rejected { node, reason } => {
                return Err(Error::Invalid(format!(
                    "source content {} has no qualified supply: {reason:?}",
                    node.id().digest.as_str()
                )));
            }
            _ => return Err(Error::Busy),
        }
    }
}
pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX))
}
