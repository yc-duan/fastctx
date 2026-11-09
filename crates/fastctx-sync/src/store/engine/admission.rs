//! FIFO byte admission bounds simultaneous semantic preparation, not stored assets.
use crate::{
    manifest::ContentRef,
    store::{Error, mutation::Submission},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(super) struct Admission {
    semaphore: Arc<Semaphore>,
    semantic_bytes: usize,
}
impl Admission {
    pub(super) fn new(semantic_bytes: usize) -> Result<Self, Error> {
        let capacity = cost(semantic_bytes)?;
        Ok(Self { semaphore: Arc::new(Semaphore::new(capacity as usize)), semantic_bytes })
    }
    pub(super) async fn acquire(&self, request: &Submission) -> Result<OwnedSemaphorePermit, Error> {
        let mut bodies = 0_usize;
        for reference in
            request.objects.iter().flat_map(|object| [object.before.as_ref(), object.candidate.as_ref()]).flatten()
        {
            let bytes = match reference {
                ContentRef::Single { decoded_bytes, .. } => *decoded_bytes as usize,
                ContentRef::Manifest { .. } => self.semantic_bytes,
            };
            bodies = bodies.saturating_add(bytes).min(self.semantic_bytes);
        }
        // Tokio's multi-permit queue is FIFO, including a large request ahead
        // of smaller ones. Cancellation removes its queued reservation.
        tokio::time::timeout(Duration::from_secs(30), self.semaphore.clone().acquire_many_owned(cost(bodies)?))
            .await
            .map_err(|_| Error::Busy)?
            .map_err(|_| Error::Unavailable("semantic admission closed".into()))
    }
}
fn cost(bytes: usize) -> Result<u32, Error> {
    // Bodies, normalized text and CRLF offset mapping coexist during planning;
    // replacement output is emitted incrementally, not stored once per match.
    bytes
        .checked_mul(8)
        .and_then(|n| n.checked_add(fastctx_types::sync::PAGE_BYTES as usize * 4))
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Error::Invalid("semantic admission budget exceeds its counter range".into()))
}
