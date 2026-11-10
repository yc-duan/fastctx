//! T2 plans against pages loaded on demand into a bounded memory view.
use crate::{
    blocks::SqliteBlocks,
    manifest::{BlobId, ContentBlock, ContentRef, ManifestRoot},
    store::Error,
    timeline::pages::PageSource,
};
use std::collections::BTreeMap;

/// Bytes one installation may hold in loaded pages and payloads.
const LOADED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Loaded {
    manifests: BTreeMap<ManifestRoot, Vec<u8>>,
    payloads: BTreeMap<BlobId, Vec<u8>>,
    bytes: usize,
    count_item_reads: bool,
}
impl PageSource for Loaded {
    fn manifest(&self, root: &ManifestRoot) -> Result<Option<&[u8]>, Error> {
        Ok(self.manifests.get(root).map(Vec::as_slice))
    }
    fn payload(&self, block: &ContentBlock) -> Result<Option<&[u8]>, Error> {
        Ok(self.payloads.get(&block.id).map(Vec::as_slice))
    }
}
impl Loaded {
    pub(super) fn for_item() -> Self {
        Self { count_item_reads: true, ..Self::default() }
    }
    /// Loads the dependency a plan reported missing; an absent page here means
    /// the local snapshot is incomplete, which installation cannot repair.
    pub(super) fn load(&mut self, blocks: &SqliteBlocks, missing: &ContentRef) -> Result<(), Error> {
        if self.count_item_reads {
            use crate::metrics::{Cause, Counter, Metrics, PlatformMetrics};
            PlatformMetrics.add(Counter::TimelineItemReads, Cause::Ordinary, 1);
        }
        let bytes = match missing {
            ContentRef::Manifest { root } => {
                let page = blocks
                    .stored_page(root)?
                    .ok_or_else(|| Error::Unavailable("a page of the installed timeline is not stored here".into()))?;
                let bytes = page.page_json.into_bytes();
                self.charge(bytes.len())?;
                self.manifests.insert(root.clone(), bytes);
                return Ok(());
            }
            ContentRef::Single { block, decoded_bytes } => {
                let content = ContentBlock { id: block.clone(), decoded_bytes: *decoded_bytes };
                blocks
                    .stored_block(&content)?
                    .ok_or_else(|| Error::Unavailable("a payload of the installed timeline is not stored here".into()))?
                    .0
            }
        };
        self.charge(bytes.len())?;
        self.payloads.insert(missing.id().clone(), bytes);
        Ok(())
    }
    fn charge(&mut self, bytes: usize) -> Result<(), Error> {
        self.bytes = self.bytes.checked_add(bytes).filter(|total| *total <= LOADED_BYTES).ok_or(Error::Busy)?;
        Ok(())
    }
}
