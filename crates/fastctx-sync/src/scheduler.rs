//! Persisted coalescing tasks; a wake never substitutes for the due index.

use crate::{
    cursor::{LocalSequence, Revision},
    identity::{OperationId, ScopeKey},
    store::FutureResult,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskKey {
    pub kind: String,
    pub scope: ScopeKey,
    pub key: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Control,
    Interactive,
    Visible,
    Background,
}
#[derive(Clone, Debug)]
pub struct Task {
    pub key: TaskKey,
    pub wanted: LocalSequence,
    pub completed: LocalSequence,
    pub priority: Priority,
    pub next_due_millis: i64,
    pub dependency: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Lease {
    pub task: Task,
    pub generation: Revision,
    pub owner: OperationId,
}
#[derive(Clone, Debug)]
pub enum Outcome {
    Completed { through: LocalSequence },
    Retry { next_due_millis: i64, reason: String },
    Blocked { dependency: String, reason: String },
    Superseded,
    Failed { reason: String },
}

/// Claiming and finishing are short transactions. Work between them holds no
/// database transaction or shared lock; a newer wanted version is never erased.
pub trait Scheduler: Send + Sync {
    fn dirty(&self, task: Task) -> FutureResult<'_, ()>;
    fn due(&self, now_millis: i64, max_tasks: u32) -> FutureResult<'_, Vec<Lease>>;
    fn finish(&self, lease: Lease, outcome: Outcome) -> FutureResult<'_, ()>;
    fn dependency_changed(&self, dependency: String) -> FutureResult<'_, ()>;
}
