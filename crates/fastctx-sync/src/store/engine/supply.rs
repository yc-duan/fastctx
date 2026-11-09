//! Root verification is durable before the pending mutation's atomic handoff.
use super::{SqliteBackend, prepare::Prepared};
use crate::{
    blocks::supply::{self, SupplyBudget, SupplyOwner, SupplyProgress, SupplyRequest, SupplyTicket},
    manifest::ContentRef,
    store::{Error, access::Custody, mutation::BaseVersion, roots::SupplyBasis, sql},
};
use fastctx_storage::{Result, rusqlite::Connection};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub(super) async fn prepare(
    store: &SqliteBackend,
    prepared: &Prepared,
) -> std::result::Result<Vec<Vec<SupplyTicket>>, Error> {
    let mut result = Vec::with_capacity(prepared.objects.len());
    let deadline = Instant::now() + Duration::from_secs(5);
    for object in &prepared.objects {
        let base = match &object.proposal.base {
            BaseVersion::Absent => None,
            BaseVersion::Version { root, .. } => root.as_ref(),
        };
        let mut roots = BTreeMap::new();
        for root in [base, object.proposal.before.as_ref(), object.proposal.candidate.as_ref()]
            .into_iter()
            .flatten()
            .chain(object.references.iter())
        {
            roots.insert(key(root)?, root.clone());
        }
        let mut bases = BTreeMap::new();
        for root in [base, object.proposal.before.as_ref()].into_iter().flatten() {
            bases.insert(key(root)?, SupplyBasis::Published { root: root.clone() });
        }
        let bases: Vec<_> = bases.into_values().collect();
        let mut tickets = Vec::with_capacity(roots.len());
        for root in roots.into_values() {
            let request = SupplyRequest {
                operation: prepared.request.operation.clone(),
                custody: object.custody.clone(),
                root,
                bases: bases.clone(),
                owner: SupplyOwner::Mutation,
            };
            let holder = store.holder.clone();
            let policy = store.policy.clone();
            let operation = request.operation.clone();
            let ticket = store
                .db
                .write(fastctx_types::sync::PAGE_BYTES as usize, move |db| {
                    supply::writer(db, &holder, policy.as_ref(), now())?.begin(&request)
                })
                .await
                .map_err(|e| sql::error(e, operation.clone()))?;
            loop {
                let holder = store.holder.clone();
                let policy = store.policy.clone();
                let current = ticket.clone();
                let progress = store
                    .db
                    .write(2 * 1024 * 1024, move |db| {
                        supply::writer(db, &holder, policy.as_ref(), now())?.step(&current, SupplyBudget::default())
                    })
                    .await
                    .map_err(|e| sql::error(e, operation.clone()))?;
                match progress {
                    SupplyProgress::Ready => break,
                    SupplyProgress::Pending { missing: None } if Instant::now() < deadline => {}
                    SupplyProgress::Rejected { node, reason } => {
                        return Err(Error::Invalid(format!(
                            "content {} has no qualified supply: {reason:?}",
                            node.id().digest.as_str()
                        )));
                    }
                    _ => return Err(Error::Busy),
                }
            }
            tickets.push(ticket);
        }
        result.push(tickets);
    }
    Ok(result)
}
pub(super) fn retain(
    db: &Connection,
    store: &SqliteBackend,
    custody: &Custody,
    tickets: &[SupplyTicket],
) -> Result<()> {
    let writer = supply::writer(db, &store.holder, store.policy.as_ref(), now())?;
    for ticket in tickets {
        let ready = writer.recheck(custody, ticket)?;
        writer.retain_supplied(ready)?;
    }
    Ok(())
}
fn key(root: &ContentRef) -> std::result::Result<Vec<u8>, Error> {
    serde_json::to_vec(root).map_err(|e| Error::Invalid(e.to_string()))
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX))
}
