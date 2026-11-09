//! Cancelling an asynchronous reader cannot release capacity still occupied by disk I/O.

use super::Permit;
use crate::outq::Outq;
use std::{future::Future, sync::Arc};
use tokio::{task::JoinError, time::Instant};

tokio::task_local! {
    static PERMIT: Arc<Permit>;
}

pub(crate) async fn run<T>(
    permit: Permit,
    out: &Outq,
    deadline: Instant,
    work: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    PERMIT
        .scope(Arc::new(permit), async {
            tokio::select! {
                biased;
                () = out.closed() => Err("the reader's connection closed".into()),
                () = tokio::time::sleep_until(deadline) => Err("the hub read did not finish within 55 seconds".into()),
                answer = work => answer,
            }
        })
        .await
}

pub(crate) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, JoinError> {
    let Ok(permit) = PERMIT.try_with(Arc::clone) else {
        return fastctx_platform::task::spawn_blocking(work).await;
    };
    let task = fastctx_platform::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    });
    let _abort = Abort(task.abort_handle());
    task.await
}

struct Abort(tokio::task::AbortHandle);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}
