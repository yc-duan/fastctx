//! Admission for filesystem initialization, owned by the actual blocking work.

use std::sync::{Arc, LazyLock};

use fastctx_types::ipc::Failure;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

static PLACES: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(32)));

#[derive(Clone)]
pub(crate) struct Guard(Arc<OwnedSemaphorePermit>);

pub(crate) fn take() -> Result<Guard, Failure> {
    PLACES
        .clone()
        .try_acquire_owned()
        .map(|place| Guard(Arc::new(place)))
        .map_err(|_| Failure::new("this machine is busy initializing file sources; try again"))
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
