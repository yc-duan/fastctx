//! Retiring one holding period never borrows that period's task eligibility.
use super::SqliteBlocks;
use crate::{
    cursor::{Holder, HoldingPeriod, Revision},
    identity::ScopeKey,
    scheduler::{Lease, Outcome, TaskKey},
    store::{
        Error, FutureResult,
        boundary::{self, Role},
        scopes::{self, ScopeState},
        sql,
    },
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension, params},
};
mod state;
mod steps;
mod uploads;
mod uses;
pub(crate) use state::begin;

pub(crate) fn task_key(scope: &ScopeKey) -> String {
    format!("holding/{}", scope.scope.as_str())
}
impl SqliteBlocks {
    pub fn takes_reconciliation(key: &TaskKey) -> bool {
        key.kind == "sync.reconcile" && (key.key == task_key(&key.scope) || key.key.starts_with("source-step/"))
    }
    /// Runs one SQL-only page under the latest committed scope target. The
    /// common worker owns leases, retries, concurrency and restart scheduling.
    pub fn reconcile_scope(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        Box::pin(async move {
            if !Self::takes_reconciliation(&lease.task.key) || lease.task.holding.is_some() {
                return Err(Error::Invalid("not a holding-independent scope reconciliation task".into()));
            }
            if lease.task.key.key.starts_with("source-step/") {
                return steps::process(self, lease).await;
            }
            let holder = self.holder().clone();
            let policy = self.policy().clone();
            self.database()
                .write(2 * 1024 * 1024, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Reconcile);
                    let scope = &lease.task.key.scope;
                    let Some(mut state) = state::get(db, scope)? else {
                        return Ok(Outcome::Superseded);
                    };
                    if state.generation.0 != lease.task.wanted.0 {
                        return Ok(Outcome::Completed { through: lease.task.wanted });
                    }
                    if scopes::current(db, scope)? != Some(state.target) {
                        return Err(sql::invalid("scope fence changed outside its reconciliation transaction"));
                    }
                    let now = now();
                    let holding = state.holding(scope, &holder);
                    state.progress = match state.progress {
                        state::Progress::Supply { after } => {
                            let writer = crate::blocks::supply::writer(db, &holder, policy.as_ref(), now)?;
                            let page = writer.invalidate(scope, None, None, after.as_ref(), 64)?;
                            match page.next {
                                Some(after) => state::Progress::Supply { after: Some(after) },
                                None => state::Progress::Uses { after: None },
                            }
                        }
                        state::Progress::Uses { after } => {
                            match uses::page(db, &holding, state.target.enabled, policy.as_ref(), after.as_ref(), now)?
                            {
                                Some(after) => state::Progress::Uses { after: Some(after) },
                                None => state::Progress::Steps { after: None },
                            }
                        }
                        state::Progress::Steps { after } => steps::invalidate_page(db, &holding, after.as_ref())?,
                        state::Progress::Uploads { after } => uploads::page(db, &holding, after)?,
                        state::Progress::Pins { after } => {
                            match super::uploads::retire_old(db, &holding, after.as_ref(), 64)? {
                                Some(after) => state::Progress::Pins { after: Some(after) },
                                None => state::Progress::Done,
                            }
                        }
                        state::Progress::Done => state::Progress::Done,
                    };
                    state::save(db, scope, &state)?;
                    Ok(if matches!(state.progress, state::Progress::Done) {
                        Outcome::Completed { through: lease.task.wanted }
                    } else {
                        Outcome::Retry { next_due_millis: now, reason: "remaining scope holding reconciliation".into() }
                    })
                })
                .await
                .map_err(|error| sql::error(error, self.worker().clone()))
        })
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX))
}
