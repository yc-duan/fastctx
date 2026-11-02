//! Asynchronous work that reaches storage, moved to a thread that may block
//! with what the task it leaves carries: the admission budget of the request
//! it serves (`gate_waits`) and the identity record controls act with
//! (`record_access`).

use std::future::Future;

/// Runs `work` to its end on a thread that may block, as
/// `fastctx_platform::task::on_blocking` does, under the budget and identity
/// of the task that asks; the work is put on the heap as this is called.
pub(crate) fn on<F>(work: F) -> impl Future<Output = F::Output> + Send + 'static
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (deadline, access) = (crate::gate_waits::carried(), crate::record_access::carried());
    // Boxed before it is wrapped, so each wrapper holds a pointer.
    let work = Box::pin(work);
    fastctx_platform::task::on_blocking(crate::record_access::carry(access, crate::gate_waits::carry(deadline, work)))
}
