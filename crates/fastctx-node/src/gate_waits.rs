//! One admission budget for all conversation gates of an external request.
//! Host reports and durable background work have no admission deadline.

use std::future::Future;
use std::time::Duration;

use fastctx_platform::doing::{Busy, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use tokio::time::Instant;

use crate::Node;

const ASKED_WAITS: Duration = Duration::from_secs(30);

tokio::task_local! {
    static DEADLINE: Option<Instant>;
}

/// Starts an admission budget, preserving an enclosing request or internal scope.
/// Only gate acquisition observes it; mutations already started finish normally.
pub(crate) fn asked<F: Future>(work: F) -> impl Future<Output = F::Output> {
    asked_since(Instant::now(), work)
}

pub(crate) fn asked_since<F: Future>(began: Instant, work: F) -> impl Future<Output = F::Output> {
    let deadline = DEADLINE.try_with(|deadline| *deadline).unwrap_or_else(|_| Some(began + ASKED_WAITS));
    DEADLINE.scope(deadline, Box::pin(work))
}

/// Runs already accepted work without borrowing the request's admission budget.
pub(crate) fn internal<F: Future>(work: F) -> impl Future<Output = F::Output> {
    DEADLINE.scope(None, Box::pin(work))
}

pub(crate) fn remaining() -> Option<Duration> {
    DEADLINE.try_with(|deadline| deadline.map(|until| until.saturating_duration_since(Instant::now()))).ok().flatten()
}

pub(crate) async fn action<'a>(
    node: &Node,
    id: &ResidentId,
    gate: &'a Mutex<()>,
) -> Result<MutexGuard<'a, ()>, Failure> {
    match remaining() {
        Some(within) if within.is_zero() => Err(told(node, id, Busy { doing: None })),
        Some(within) => gate.lock_within(within).await.map_err(|busy| told(node, id, busy)),
        None => Ok(gate.lock().await),
    }
}

pub(crate) async fn shared<'a>(
    node: &Node,
    id: &ResidentId,
    gate: &'a RwLock<()>,
) -> Result<RwLockReadGuard<'a, ()>, Failure> {
    match remaining() {
        Some(within) if within.is_zero() => Err(told(node, id, Busy { doing: None })),
        Some(within) => gate.read_within(within).await.map_err(|busy| told(node, id, busy)),
        None => Ok(gate.read().await),
    }
}

pub(crate) async fn alone<'a>(
    node: &Node,
    id: &ResidentId,
    gate: &'a RwLock<()>,
) -> Result<RwLockWriteGuard<'a, ()>, Failure> {
    match remaining() {
        Some(within) if within.is_zero() => Err(told(node, id, Busy { doing: None })),
        Some(within) => gate.write_within(within).await.map_err(|busy| told(node, id, busy)),
        None => Ok(gate.write().await),
    }
}

fn told(node: &Node, id: &ResidentId, busy: Busy) -> Failure {
    if node.residents.is_deleting(id) {
        return Failure::new(format!("{id} is being deleted"));
    }
    Failure::new(match busy.doing {
        Some(doing) => format!("{id} is busy ({doing}); try again in a moment"),
        None => format!("{id} is busy; try again in a moment"),
    })
}
