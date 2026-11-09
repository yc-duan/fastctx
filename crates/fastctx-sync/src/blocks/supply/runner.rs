//! The common scheduler drives both resumable checking and holding-independent cleanup.
use super::SqliteBlocks;
use crate::{
    blocks::supply::{self, SupplyBudget, SupplyProgress, SupplyTicket},
    cursor::Revision,
    identity::Digest,
    scheduler::{Lease, Outcome},
    store::{Error, sql},
};

pub(super) async fn process(store: &SqliteBlocks, lease: Lease) -> Result<Outcome, Error> {
    if let Some(operation) = lease.task.key.key.strip_prefix("supply-cancel:") {
        if lease.task.key.kind != "sync.gc" {
            return Err(Error::Invalid("supply cancellation requires a cleanup task".into()));
        }
        let (operation, object) =
            operation.split_once(':').ok_or_else(|| Error::Invalid("invalid cancelled supply coordinates".into()))?;
        let object = crate::identity::ObjectId::try_from(object.to_owned()).map_err(Error::Invalid)?;
        let operation = crate::identity::OperationId::try_from(operation.to_owned()).map_err(Error::Invalid)?;
        let holder = store.holder.clone();
        let policy = store.policy.clone();
        return store
            .db
            .write(2 * 1024 * 1024, move |db| {
                let writer = supply::writer(db, &holder, policy.as_ref(), now())?;
                let page = writer.invalidate(&lease.task.key.scope, Some(&object), Some(&operation), None, 64)?;
                Ok(if page.next.is_some() {
                    Outcome::Retry { next_due_millis: now(), reason: "remaining cancelled supply work".into() }
                } else {
                    Outcome::Completed { through: lease.task.wanted }
                })
            })
            .await
            .map_err(|error| sql::error(error, store.worker.clone()));
    }
    let parts: Vec<_> = lease.task.key.key.split(':').collect();
    if parts.len() != 3 || parts[0] != "supply" || !matches!(lease.task.key.kind.as_str(), "sync.gc" | "sync.manifest")
    {
        return Err(Error::Invalid("invalid supply task coordinates".into()));
    }
    let id = Digest::try_from(parts[1].to_owned()).map_err(Error::Invalid)?;
    let generation = parts[2].parse::<u64>().map_err(|e| Error::Invalid(e.to_string()))?;
    if generation == 0 {
        return Err(Error::Invalid("supply task has a zero generation".into()));
    }
    let ticket = SupplyTicket { scope: lease.task.key.scope.clone(), id, generation: Revision(generation) };
    let cleanup = lease.task.key.kind == "sync.gc";
    if !cleanup
        && lease
            .task
            .holding
            .as_ref()
            .is_none_or(|period| period.holder != store.holder || period.scope != ticket.scope)
    {
        return Err(Error::Denied("supply task differs from its actual holder".into()));
    }
    let holder = store.holder.clone();
    let policy = store.policy.clone();
    store
        .db
        .write(2 * 1024 * 1024, move |db| {
            let now = now();
            let writer = supply::writer(db, &holder, policy.as_ref(), now)?;
            if cleanup {
                let progress = writer.cleanup(&ticket, 64)?;
                return Ok(if progress.done {
                    Outcome::Completed { through: lease.task.wanted }
                } else {
                    Outcome::Retry { next_due_millis: now, reason: "remaining private supply protection".into() }
                });
            }
            let progress = writer.step(&ticket, SupplyBudget::default())?;
            Ok(match progress {
                SupplyProgress::Pending { missing: None } => {
                    Outcome::Retry { next_due_millis: now, reason: "remaining supply frontier".into() }
                }
                SupplyProgress::Pending { missing: Some(root) } => Outcome::Blocked {
                    dependency: format!(
                        "manifest:{}:{}:{}",
                        root.id().domain.hub,
                        root.id().domain.scope.as_str(),
                        root.id().digest.as_str()
                    ),
                    reason: "waiting for supplied content to become durable and validated".into(),
                },
                SupplyProgress::Ready | SupplyProgress::Consumed => Outcome::Completed { through: lease.task.wanted },
                SupplyProgress::Paused { .. } | SupplyProgress::Rejected { .. } | SupplyProgress::Cancelled => {
                    Outcome::Superseded
                }
            })
        })
        .await
        .map_err(|error| sql::error(error, store.worker.clone()))
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX))
}
