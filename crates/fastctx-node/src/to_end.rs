//! Work that must reach its end once it has begun, whoever waits for it:
//! keeping words, handing them over, settling a control. It runs as a task
//! of its own with the admission budget of the request it serves
//! (`gate_waits`) and the identity record controls act with
//! (`record_access`), so a caller that goes away leaves it going. Each read
//! or write it makes of records goes where that may block; its waits for
//! gates and hosts keep no thread.

use std::future::Future;

/// Starts `work` as a task of its own and waits for what it returns; a
/// panic in it goes on in the task that waits, as it would have there. A
/// task kept too long at once is named by where this was called.
#[track_caller]
pub(crate) fn run<F>(work: F) -> impl Future<Output = F::Output> + Send + 'static
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (deadline, access) = (crate::gate_waits::carried(), crate::record_access::carried());
    // What the asker said it does is said by the task doing it.
    let label = fastctx_platform::doing::label();
    // Boxed before it is wrapped, so each wrapper holds a pointer.
    let work = Box::pin(work);
    let task = fastctx_platform::task::spawn(async move {
        let _doing = fastctx_platform::doing::doing(label);
        crate::record_access::carry(access, crate::gate_waits::carry(deadline, work)).await
    });
    async move {
        match task.await {
            Ok(value) => value,
            Err(error) => match error.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(_) => panic!("the runtime stopped before the work reached its end"),
            },
        }
    }
}
