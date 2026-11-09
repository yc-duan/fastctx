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

mod dependencies;
mod due;
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
    /// The earliest time any task becomes claimable. It is a read, unlike
    /// claiming, so an idle worker sleeps until then instead of polling.
    pub fn next_due(&self) -> FutureResult<'_, Option<i64>> {
        Box::pin(async move {
            self.db
                .read(|db| {
                    let mut earliest: Option<i64> = None;
                    let mut query = db.prepare_cached(
                        "SELECT next_due FROM sync_tasks WHERE state=?1 AND priority=?2 ORDER BY next_due LIMIT 1",
                    )?;
                    for state in ["ready", "leased", "blocked"] {
                        for priority in 0..4_u8 {
                            let found: Option<i64> =
                                query.query_row(params![state, priority], |row| row.get(0)).optional()?;
                            earliest = match (earliest, found) {
                                (Some(known), Some(found)) => Some(known.min(found)),
                                (known, found) => known.or(found),
                            };
                        }
                    }
                    Ok(earliest)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
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
                    if task.dependency_sequence > dependencies::sequence(db)? {
                        return Err(sql::invalid("dependency observation claims a future sequence"));
                    }
                    update::dirty(db, &mut row, task)?;
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
            self.db
                .write(max_tasks as usize * 16 * 1024, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Scheduling);
                    due::claim(db, &owner, &holder, now_millis, expires, max_tasks)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
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
                        update::finish(db, &mut row, outcome)?;
                    }
                    save(db, &row)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    fn dependency_sequence(&self) -> FutureResult<'_, LocalSequence> {
        Box::pin(async move {
            self.db.read_now(dependencies::sequence).map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    fn dependency_changed(&self, dependency: String) -> FutureResult<'_, ()> {
        Box::pin(async move {
            if dependency.is_empty() || dependency.len() > 1024 {
                return Err(Error::Invalid("invalid task dependency".into()));
            }
            let mut first = true;
            loop {
                let dependency = dependency.clone();
                let changed = self
                    .db
                    .write(256 * 4096, move |db| {
                        let _boundary = boundary::enter_typed_write(Role::Scheduling);
                        if first {
                            dependencies::changed(db, &dependency)?;
                        }
                        dependencies::wake_page(db, &dependency)
                    })
                    .await
                    .map_err(|error| sql::error(error, self.worker.clone()))?;
                first = false;
                if changed < 256 {
                    return Ok(());
                }
            }
        })
    }
}

/// Content publishers enqueue this under their existing SQL-only write boundary.
/// Repeated changes coalesce until a lease has actually claimed the old version.
pub(crate) fn content_changed(db: &Connection, holding: &HoldingPeriod, key: String) -> Result<()> {
    let key = TaskKey { kind: "sync.manifest".into(), scope: holding.scope.clone(), key };
    let existing = get(db, &key)?;
    if existing.as_ref().is_some_and(|row| {
        row.state == "ready" && row.task.wanted > row.task.completed && row.task.holding.as_ref() == Some(holding)
    }) {
        return Ok(());
    }
    let wanted = existing
        .as_ref()
        .map_or(0, |row| row.task.wanted.0)
        .checked_add(1)
        .ok_or_else(|| sql::invalid("content work version overflow"))?;
    let task = Task {
        key,
        wanted: LocalSequence(wanted),
        completed: LocalSequence(0),
        priority: Priority::Background,
        next_due_millis: 0,
        dependency: None,
        dependency_revision: LocalSequence(wanted),
        dependency_sequence: dependencies::sequence(db)?,
        holding: Some(holding.clone()),
    };
    validate(&task).map_err(|error| sql::invalid(error.to_string()))?;
    let mut row = existing.unwrap_or_else(|| Row::new(task.clone()));
    update::dirty(db, &mut row, task)?;
    save(db, &row)
}

/// A source commit marks a derived projection stale at its own local sequence.
/// The projection worker brings it to `wanted`; later commits only raise it.
pub(crate) fn projection_changed(
    db: &Connection,
    kind: &str,
    object: &crate::identity::ObjectKey,
    local: LocalSequence,
) -> Result<()> {
    let key = TaskKey {
        kind: "sync.projection".into(),
        scope: object.scope.clone(),
        key: format!("{kind}/{}", object.object.as_str()),
    };
    let existing = get(db, &key)?;
    if existing.as_ref().is_some_and(|row| row.task.wanted >= local) {
        return Ok(());
    }
    let task = Task {
        key,
        wanted: local,
        completed: LocalSequence(0),
        priority: Priority::Visible,
        next_due_millis: 0,
        dependency: None,
        dependency_revision: local,
        dependency_sequence: dependencies::sequence(db)?,
        holding: None,
    };
    validate(&task).map_err(|error| sql::invalid(error.to_string()))?;
    let mut row = existing.unwrap_or_else(|| Row::new(task.clone()));
    update::dirty(db, &mut row, task)?;
    save(db, &row)
}

/// Cleanup survives the holding period it retires. Its owner is rechecked by the
/// supply worker instead of borrowing an invalidated transport lease.
pub(crate) fn supply_cleanup(db: &Connection, scope: &ScopeKey, key: String) -> Result<()> {
    let key = TaskKey { kind: "sync.gc".into(), scope: scope.clone(), key };
    let existing = get(db, &key)?;
    if existing.as_ref().is_some_and(|row| row.state == "ready" && row.task.wanted > row.task.completed) {
        return Ok(());
    }
    let wanted = existing
        .as_ref()
        .map_or(0, |row| row.task.wanted.0)
        .checked_add(1)
        .ok_or_else(|| sql::invalid("supply cleanup version overflow"))?;
    let task = Task {
        key,
        wanted: LocalSequence(wanted),
        completed: LocalSequence(0),
        priority: Priority::Background,
        next_due_millis: 0,
        dependency: None,
        dependency_revision: LocalSequence(wanted),
        dependency_sequence: dependencies::sequence(db)?,
        holding: None,
    };
    validate(&task).map_err(|error| sql::invalid(error.to_string()))?;
    let mut row = existing.unwrap_or_else(|| Row::new(task.clone()));
    update::dirty(db, &mut row, task)?;
    save(db, &row)
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
