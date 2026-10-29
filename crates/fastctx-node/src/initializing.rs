//! Admission for filesystem initialization, owned by the actual blocking work.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use fastctx_types::ipc::{CapacityReason, Failure};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

static PLACES: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(32)));

/// How long a file source waits its turn: within the minute it has to say
/// it is open (`fastctx_types::contract::OPENED_WITHIN`).
const WAITS: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct Guard(Arc<OwnedSemaphorePermit>);

/// A place for initializing a file source, once one is free.
pub(crate) async fn take() -> Result<Guard, Failure> {
    match tokio::time::timeout(WAITS, PLACES.clone().acquire_owned()).await {
        Ok(Ok(place)) => Ok(Guard(Arc::new(place))),
        _ => Err(Failure::not_run(format!(
            "this machine was too busy initializing file sources to start this one within {} s",
            WAITS.as_secs()
        ))
        .capacity(CapacityReason::Waiting { seconds: WAITS.as_secs() })),
    }
}

impl Guard {
    /// Cancelling the waiter leaves the place with the queued or running closure.
    pub(crate) async fn run<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> Result<T, Failure> {
        let place = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let _place = place;
            work()
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))
    }
}
