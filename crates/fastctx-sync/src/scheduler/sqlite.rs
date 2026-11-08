//! Dirty coalescing and expiring leases share the service's FULL-commit writer.
use super::{Lease, Outcome, Priority, Scheduler, Task, TaskKey};
use crate::{
    cursor::{Holder, HoldingPeriod, LocalSequence, Revision},
    identity::{HubId, OperationId, ScopeId, ScopeKey},
    store::{
        Error, FutureResult,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::{
    Database, Result,
    rusqlite::{self, Connection, OptionalExtension, params},
};
use std::time::Duration;

mod rows;
mod update;
use rows::{Row, get, save};

/// Only service assembly receives the database. Business modules use Scheduler.
/// Install store::boundary::guard before constructing this queue. Each running
/// worker supplies a unique operation identity and a finite lease duration.
#[derive(Clone)]
pub struct SqliteScheduler {
    db: Database,
    worker: OperationId,
    holder: Holder,
    lease_millis: i64,
}
impl SqliteScheduler {
    pub fn new(db: Database, worker: OperationId, holder: Holder, lease: Duration) -> std::result::Result<Self, Error> {
        let lease_millis =
            i64::try_from(lease.as_millis()).map_err(|_| Error::Invalid("lease duration overflow".into()))?;
        if lease_millis == 0 {
            return Err(Error::Invalid("a worker lease must be nonzero".into()));
        }
        Ok(Self { db, worker, holder, lease_millis })
    }
}

/// These are scheduler handlers, not arbitrary asset namespaces or SQL tables.
const KINDS: &[&str] = &[
    "sync.transfer",
    "sync.manifest",
    "sync.reconcile",
    "sync.projection",
    "sync.gc",
    "sync.exposure",
    "sync.outbox",
    "sync.ingest",
];

impl Scheduler for SqliteScheduler {
    fn dirty(&self, task: Task) -> FutureResult<'_, ()> {
        Box::pin(async move {
            validate(&task)?;
            let holder = self.holder.clone();
            self.db
                .write(4096 + task.key.key.len(), move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Scheduling);
                    if !current_holding(db, &holder, task.holding.as_ref())? {
                        return Err(sql::invalid("task holding period is no longer current"));
                    }
                    let mut row = get(db, &task.key)?.unwrap_or_else(|| Row::new(task.clone()));
                    update::dirty(&mut row, task)?;
                    save(db, &row)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    fn due(&self, now_millis: i64, max_tasks: u32) -> FutureResult<'_, Vec<Lease>> {
        Box::pin(async move {
            if max_tasks == 0 || max_tasks > 256 {
                return Err(Error::Invalid("a lease page must contain 1 to 256 tasks".into()));
            }
            let owner = self.worker.clone();
            let holder = self.holder.clone();
            let expires = now_millis
                .checked_add(self.lease_millis)
                .ok_or_else(|| Error::Invalid("lease deadline overflow".into()))?;
            self.db.write(256 * 4096, move |db| {
                let _boundary = boundary::enter_typed_write(Role::Scheduling);
                let mut candidates = Vec::new();
                for priority in 0..4 {
                    for state in ["ready", "leased"] {
                        let mut query = db.prepare_cached("SELECT kind,hub,scope,task_key,wanted,completed,state,priority,next_due,lease_generation,lease_owner,attempts,dependency,reason,holding,lease_wanted FROM sync_tasks WHERE state=?1 AND priority=?2 AND next_due<=?3 ORDER BY next_due,hub,scope,kind,task_key LIMIT ?4")?;
                        let rows = query.query_map(params![state,priority,now_millis,max_tasks], rows::read)?;
                        candidates.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
                    }
                    if candidates.len() >= max_tasks as usize { break; }
                }
                candidates.sort_by(|a,b| (number(a.task.priority),a.task.next_due_millis,&a.task.key.scope,&a.task.key.kind,&a.task.key.key).cmp(&(number(b.task.priority),b.task.next_due_millis,&b.task.key.scope,&b.task.key.kind,&b.task.key.key)));
                let mut leases = Vec::new();
                for mut row in candidates.into_iter().take(max_tasks as usize) {
                    if !current_holding(db, &holder, row.task.holding.as_ref())? {
                        row.state = "superseded".into(); row.owner = None;
                        save(db,&row)?;
                        continue;
                    }
                    row.generation = next(row.generation)?;
                    row.owner = Some(owner.clone());
                    row.lease_wanted = Some(row.task.wanted);
                    row.state = "leased".into();
                    row.task.next_due_millis = expires;
                    row.attempts = row.attempts.saturating_add(1);
                    save(db,&row)?;
                    leases.push(Lease { task:row.task, generation:row.generation, owner:owner.clone() });
                }
                Ok(leases)
            }).await.map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    fn finish(&self, lease: Lease, outcome: Outcome) -> FutureResult<'_, ()> {
        Box::pin(async move {
            validate(&lease.task)?;
            update::validate_outcome(&outcome)?;
            let holder = self.holder.clone();
            self.db
                .write(8192, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Scheduling);
                    let Some(mut row) = get(db, &lease.task.key)? else { return Ok(()) };
                    if row.state != "leased"
                        || row.owner.as_ref() != Some(&lease.owner)
                        || row.generation != lease.generation
                        || row.task.holding != lease.task.holding
                    {
                        return Ok(());
                    }
                    if !current_holding(db, &holder, row.task.holding.as_ref())? {
                        row.state = "superseded".into();
                        row.owner = None;
                    } else {
                        update::finish(&mut row, outcome)?;
                    }
                    save(db, &row)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    fn dependency_changed(&self, dependency: String) -> FutureResult<'_, ()> {
        Box::pin(async move {
            if dependency.is_empty() || dependency.len() > 1024 {
                return Err(Error::Invalid("invalid task dependency".into()));
            }
            loop {
                let dependency = dependency.clone();
                let changed = self.db.write(256 * 4096, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Scheduling);
                    let mut query = db.prepare_cached("SELECT kind,hub,scope,task_key,wanted,completed,state,priority,next_due,lease_generation,lease_owner,attempts,dependency,reason,holding,lease_wanted FROM sync_tasks WHERE dependency=?1 AND state='blocked' ORDER BY next_due LIMIT 256")?;
                    let rows = query.query_map([dependency], rows::read)?.collect::<rusqlite::Result<Vec<_>>>()?;
                    let changed = rows.len();
                    for mut row in rows {
                        row.task.dependency = None; row.task.next_due_millis = 0;
                        row.state = if row.task.completed < row.task.wanted { "ready" } else { "complete" }.into();
                        row.reason = None;
                        save(db,&row)?;
                    }
                    Ok(changed)
                }).await.map_err(|error| sql::error(error, self.worker.clone()))?;
                if changed < 256 {
                    return Ok(());
                }
            }
        })
    }
}

fn validate(task: &Task) -> std::result::Result<(), Error> {
    if !KINDS.contains(&task.key.kind.as_str())
        || task.key.key.is_empty()
        || task.key.key.len() > 4096
        || task.dependency.as_ref().is_some_and(|key| key.is_empty() || key.len() > 1024)
    {
        return Err(Error::Invalid("unregistered or oversized scheduler task".into()));
    }
    if task.holding.as_ref().is_some_and(|holding| holding.scope != task.key.scope) {
        return Err(Error::Invalid("task and holding refer to different scopes".into()));
    }
    if matches!(task.key.kind.as_str(), "sync.transfer" | "sync.manifest") && task.holding.is_none() {
        return Err(Error::Invalid("content work requires a holding period".into()));
    }
    Ok(())
}
fn number(priority: Priority) -> u8 {
    match priority {
        Priority::Control => 0,
        Priority::Interactive => 1,
        Priority::Visible => 2,
        Priority::Background => 3,
    }
}
fn next(revision: Revision) -> Result<Revision> {
    revision.0.checked_add(1).map(Revision).ok_or_else(|| sql::invalid("task lease generation overflow"))
}
fn current_holding(db: &Connection, holder: &Holder, holding: Option<&HoldingPeriod>) -> Result<bool> {
    let Some(holding) = holding else { return Ok(true) };
    if &holding.holder != holder {
        return Ok(false);
    }
    let found = db
        .query_row(
            "SELECT enabled,acl_revision,publication_epoch FROM sync_scopes WHERE hub=?1 AND scope=?2",
            params![holding.scope.hub.to_string(), holding.scope.scope.as_str()],
            |row| Ok((row.get::<_, bool>(0)?, sql::counter(row, 1)?, sql::counter(row, 2)?)),
        )
        .optional()?;
    Ok(found == Some((true, holding.acl_revision.0, holding.publication_epoch.0)))
}
