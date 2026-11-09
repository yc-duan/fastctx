//! One loop runs a service's durable tasks. It sleeps until the earliest due
//! task or a wake, claims a bounded page, hands each task to the handler that
//! takes it and records the outcome. A task no handler takes is parked on
//! `handler:<kind>` until a worker that handles the kind starts.
use super::{Lease, Outcome, Scheduler, SqliteScheduler, TaskKey};
use crate::store::{Error, FutureResult};
use std::{
    collections::BTreeSet,
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::Notify, task::JoinSet};

/// Tasks one claim takes; they run concurrently, sharing the service's writer.
const PAGE: u32 = 8;
/// The longest an idle worker sleeps without being woken.
const IDLE: Duration = Duration::from_secs(30);
/// How long a failed claim or read waits before the loop tries again.
const BACKOFF: Duration = Duration::from_secs(1);

pub trait TaskHandler: Send + Sync {
    /// The scheduler kind this handler serves.
    fn kind(&self) -> &'static str;
    /// Handlers sharing a kind split it by key; the first that takes a task runs it.
    fn takes(&self, key: &TaskKey) -> bool {
        key.kind == self.kind()
    }
    fn handle(&self, lease: Lease) -> FutureResult<'_, Outcome>;
}

/// Wakes the worker after a commit that made work due now.
#[derive(Clone)]
pub struct Wake(Arc<Notify>);
impl Wake {
    pub fn notify(&self) {
        self.0.notify_one();
    }
}

pub struct Worker {
    scheduler: SqliteScheduler,
    handlers: Vec<Arc<dyn TaskHandler>>,
    wake: Arc<Notify>,
    report: Arc<dyn Fn(String) + Send + Sync>,
}
impl Worker {
    /// `report` receives failures the loop recovers from by itself.
    pub fn new(
        scheduler: SqliteScheduler,
        handlers: Vec<Arc<dyn TaskHandler>>,
        report: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        Self { scheduler, handlers, wake: Arc::new(Notify::new()), report }
    }
    pub fn wake(&self) -> Wake {
        Wake(self.wake.clone())
    }
    /// Runs until `stop` completes. Tasks parked for a kind this worker handles
    /// are released first, so a restart with a new handler resumes them.
    pub async fn run(self, stop: impl Future<Output = ()>) {
        tokio::pin!(stop);
        let kinds: BTreeSet<&'static str> = self.handlers.iter().map(|handler| handler.kind()).collect();
        for kind in kinds {
            while let Err(error) = self.scheduler.dependency_changed(format!("handler:{kind}")).await {
                (self.report)(format!("releasing parked {kind} tasks: {error}"));
                tokio::select! {
                    _ = &mut stop => return,
                    _ = tokio::time::sleep(BACKOFF) => {}
                }
            }
        }
        loop {
            let wait = match self.round().await {
                Ok(wait) => wait,
                Err(error) => {
                    (self.report)(format!("running sync tasks: {error}"));
                    BACKOFF
                }
            };
            tokio::select! {
                _ = &mut stop => return,
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }
    /// Runs every task due now, then says how long to sleep.
    async fn round(&self) -> Result<Duration, Error> {
        loop {
            let leases = self.scheduler.due(now(), PAGE).await?;
            if leases.is_empty() {
                break;
            }
            self.page(leases).await;
        }
        Ok(match self.scheduler.next_due().await? {
            Some(at) => Duration::from_millis(u64::try_from(at.saturating_sub(now())).unwrap_or(0)).min(IDLE),
            None => IDLE,
        })
    }
    async fn page(&self, leases: Vec<Lease>) {
        let mut running = JoinSet::new();
        for lease in leases {
            let handler = self.handlers.iter().find(|handler| handler.takes(&lease.task.key)).cloned();
            let scheduler = self.scheduler.clone();
            let report = self.report.clone();
            running.spawn(async move {
                let outcome = match handler {
                    Some(handler) => handler.handle(lease.clone()).await.unwrap_or_else(outcome),
                    None => Outcome::Blocked {
                        dependency: format!("handler:{}", lease.task.key.kind),
                        reason: "no handler for this task kind runs here".into(),
                    },
                };
                let key = lease.task.key.clone();
                if let Err(error) = scheduler.finish(lease, outcome).await {
                    report(format!("recording {} task {}: {error}", key.kind, key.key));
                }
            });
        }
        while running.join_next().await.is_some() {}
    }
}

/// A handler error becomes a durable outcome: contention and missing inputs
/// retry later, a moved precondition retries at once, refusals and corrupt
/// inputs stop the task until something marks it dirty again.
fn outcome(error: Error) -> Outcome {
    let later = |millis: i64, error: &Error| Outcome::Retry {
        next_due_millis: now().saturating_add(millis),
        reason: bounded(error.to_string()),
    };
    match error {
        Error::Busy | Error::Full => later(1_000, &error),
        Error::Unavailable(_) | Error::OutcomeUnknown { .. } => later(5_000, &error),
        Error::Conflict { .. } | Error::LocalConflict { .. } => later(0, &error),
        Error::Revoked => Outcome::Superseded,
        Error::Invalid(_) | Error::Denied(_) | Error::Corrupt(_) => {
            Outcome::Failed { reason: bounded(error.to_string()) }
        }
    }
}
fn bounded(mut reason: String) -> String {
    if reason.len() > 4096 {
        let mut end = 4096;
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason.truncate(end);
    }
    reason
}
fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |value| i64::try_from(value.as_millis()).unwrap_or(i64::MAX))
}
