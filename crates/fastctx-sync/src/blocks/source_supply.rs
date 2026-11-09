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
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut supplies = SourceSupplies { records: BTreeMap::new(), checkpoint: None, uses: BTreeMap::new() };
    if let Some(root) = &operation.next().state
        && previous.as_ref().is_none_or(|old| &old.root != root)
    {
        let request = SupplyRequest {
            operation: step.operation().clone(),
            custody: primary.custody.clone(),
            root: root.clone(),
            bases: bases.clone(),
            owner: SupplyOwner::Use { usage: RootUse::Checkpoint { source: primary.source.clone() } },
        };
        supplies.checkpoint = Some(check(blocks, request, deadline).await?);
    }
    for batch in operation.batches() {
        let target = batch.target.target();
        if let Some(IngestFacts::Local { batch: facts }) = &batch.facts {
            for root in facts.records.iter().flat_map(|record| {
                std::iter::once(ContentRef::from(record.payload.clone())).chain(record.roots.iter().cloned())
            }) {
                let key = record_key(target, &root).map_err(|e| sql::error(e, step.operation().clone()))?;
                if supplies.records.contains_key(&key) {
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
                let request = SupplyRequest {
                    operation: step.operation().clone(),
                    custody: target.custody.clone(),
                    root,
                    bases: basis,
                    owner: SupplyOwner::Source { source: target.source.clone() },
                };
                let ticket = check(blocks, request, deadline).await?;
                supplies.records.insert(key, ticket);
            }
        }
        for change in batch.effects.roots() {
            let RootChange::Acquire(request) = change else {
                continue;
            };
            if matches!(request.usage, RootUse::Timeline { .. }) {
                continue;
            }
            let supply = SupplyRequest {
                operation: step.operation().clone(),
                custody: request.custody.clone(),
                root: request.root.clone(),
                bases: request.bases.clone(),
                owner: SupplyOwner::Use { usage: request.usage.clone() },
            };
            let ticket = check(blocks, supply, deadline).await?;
            supplies.uses.insert(request.lease.clone(), ticket);
        }
    }
    Ok(supplies)
}
async fn check(blocks: &SqliteBlocks, request: SupplyRequest, deadline: Instant) -> Result<SupplyTicket, Error> {
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
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX))
}
