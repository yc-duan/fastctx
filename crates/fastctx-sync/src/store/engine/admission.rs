//! FIFO byte admission bounds simultaneous semantic preparation, not stored assets.
use crate::admission::{ByteBudget, BytePermit};
use crate::{
    manifest::ContentRef,
    store::{
        Error,
        mutation::{OriginalIntent, Submission},
    },
};
use std::time::Duration;
use tokio::time::Instant;

pub(super) struct Admission {
    budget: ByteBudget,
    semantic_bytes: usize,
}
impl Admission {
    pub(super) fn new(semantic_bytes: usize) -> Result<Self, Error> {
        let capacity = cost(semantic_bytes)?;
        Ok(Self { budget: ByteBudget::new(capacity as usize)?, semantic_bytes })
    }
    pub(super) async fn acquire(&self, request: &Submission) -> Result<BytePermit, Error> {
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
        // A handler's sequential edit can temporarily grow far beyond both the
        // initial and final bodies. Reserve its bounded intermediate workspace.
        if request.objects.iter().any(|object| matches!(object.original, OriginalIntent::Edit { .. })) {
            bodies = self.semantic_bytes;
        }
        self.budget.acquire_until(cost(bodies)? as usize, Instant::now() + Duration::from_secs(30)).await
    }
}
fn cost(bytes: usize) -> Result<u32, Error> {
    // Bodies, normalized text and CRLF offset mapping coexist during planning;
    // replacement output is emitted incrementally, not stored once per match.
    bytes
        .checked_mul(10)
        .and_then(|n| n.checked_add(fastctx_types::sync::PAGE_BYTES as usize * 4))
        .and_then(|n| n.checked_add(fastctx_types::sync::BLOCK_BYTES as usize + 65536))
        .and_then(|n| n.checked_add(crate::blocks::codec::decode_workspace(crate::manifest::Encoding::Zstd)))
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Error::Invalid("semantic admission budget exceeds its counter range".into()))
}
