//! Retained codec/read buffers share one resource-derived admission budget.
use crate::store::Error;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub(crate) struct Budget {
    limit: usize,
    used: AtomicUsize,
}
pub(crate) struct Permit {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Budget {
    pub(crate) fn new(limit: usize) -> Result<Arc<Self>, Error> {
        if limit < 2 * fastctx_types::sync::BLOCK_BYTES as usize + 65536 {
            return Err(Error::Invalid("content budget cannot hold one bounded encoding operation".into()));
        }
        Ok(Arc::new(Self { limit, used: AtomicUsize::new(0) }))
    }
    pub(crate) fn acquire(self: &Arc<Self>, bytes: usize) -> Result<Permit, Error> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|total| *total <= self.limit)
            })
            .map_err(|_| Error::Busy)?;
        Ok(Permit { budget: self.clone(), bytes })
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
