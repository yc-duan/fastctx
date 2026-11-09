//! Retained codec/read buffers share one resource-derived admission budget.
use crate::{admission::ByteBudget, store::Error};
use std::sync::Arc;

pub(crate) use crate::admission::BytePermit as Permit;

pub(crate) struct Budget(ByteBudget);
impl Budget {
    pub(crate) fn new(limit: usize) -> Result<Arc<Self>, Error> {
        if limit
            < fastctx_types::sync::BLOCK_BYTES as usize
                + 65536
                + super::codec::decode_workspace(crate::manifest::Encoding::Zstd)
        {
            return Err(Error::Invalid("content budget cannot hold one bounded encoding operation".into()));
        }
        Ok(Arc::new(Self(ByteBudget::new(limit)?)))
    }
    pub(crate) fn acquire(&self, bytes: usize) -> Result<Permit, Error> {
        self.0.try_acquire(bytes)
    }
}
