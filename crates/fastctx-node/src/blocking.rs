//! Asynchronous work that reaches storage and files throughout, as a host's
//! session does: it runs to its end as a task of its own (`crate::to_end`),
//! each read and write blocking in place (`fastctx_platform::roles::in_place`),
//! and its waits for the host keep no thread.

use std::future::Future;

/// Runs `work` to its end as `crate::to_end::run` does, under the budget and
/// identity of the task that asks.
#[track_caller]
pub(crate) fn on<F>(work: F) -> impl Future<Output = F::Output> + Send + 'static
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    crate::to_end::run(work)
}
