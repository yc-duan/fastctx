//! Asynchronous work that reaches storage, moved to a thread that may block
//! with what the task it leaves carries: the admission budget of the request
//! it serves (`gate_waits`) and the identity record controls act with
//! (`record_access`).

use std::future::Future;

/// Runs `work` to its end on a thread that may block, as
/// `fastctx_platform::task::on_blocking` does, under the budget and identity
/// of the task that asks.
pub(crate) async fn on<F>(work: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (deadline, access) = (crate::gate_waits::carried(), crate::record_access::carried());
    fastctx_platform::task::on_blocking(async move {
        crate::record_access::carry(access, crate::gate_waits::carry(deadline, work)).await
    })
    .await
}
