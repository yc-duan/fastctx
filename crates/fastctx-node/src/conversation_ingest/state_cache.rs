//! A read-through cache for one fixed checkpoint revision. These bytes have
//! passed read_state; they are never advertised as current-step uploads.
use fastctx_sync::manifest::{BlobId, ContentBlock};
use std::{collections::BTreeMap, sync::Arc};

pub(super) const BYTES: usize = 512 * 1024;
const ENTRY_BYTES: usize = 1024;

#[derive(Default)]
pub(super) struct StateCache {
    blocks: BTreeMap<BlobId, Arc<[u8]>>,
    bytes: usize,
}
impl StateCache {
    pub fn get(&self, block: &ContentBlock) -> Option<Arc<[u8]>> {
        self.blocks.get(&block.id).filter(|bytes| bytes.len() == block.decoded_bytes as usize).cloned()
    }
    pub fn put(&mut self, block: &ContentBlock, bytes: &[u8]) {
        let cost = bytes.len().saturating_add(ENTRY_BYTES);
        if cost > BYTES || self.blocks.contains_key(&block.id) {
            return;
        }
        if self.bytes + cost > BYTES {
            self.blocks.clear();
            self.bytes = 0;
        }
        self.blocks.insert(block.id.clone(), Arc::from(bytes));
        self.bytes += cost;
    }
}
