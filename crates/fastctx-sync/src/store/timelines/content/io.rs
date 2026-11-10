//! Content fetches keep the existing shared IO and byte admission bounds.
use crate::{
    blocks::SqliteBlocks,
    conversation::body_tree::BodyTreeIo,
    manifest::{ContentBlock, ManifestPage, ManifestReply, ManifestRoot},
    metrics::{Cause, Counter, Metrics, PlatformMetrics},
    store::{Error, FutureResult, access::Custody},
};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

pub(super) const MAX_READS: usize = 1024;

pub(super) struct Io<'a> {
    pub blocks: &'a SqliteBlocks,
    pub custody: Custody,
    reads: AtomicUsize,
    ownership: AtomicUsize,
    permits: Mutex<Vec<crate::admission::BytePermit>>,
}
impl<'a> Io<'a> {
    pub fn new(blocks: &'a SqliteBlocks, custody: Custody) -> Self {
        Self {
            blocks,
            custody,
            reads: AtomicUsize::new(0),
            ownership: AtomicUsize::new(usize::MAX),
            permits: Mutex::new(Vec::new()),
        }
    }
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }
    pub fn finish_ownership(&self) {
        self.ownership.store(self.reads(), Ordering::Relaxed);
    }
    fn charge(&self) -> Result<(), Error> {
        self.reads
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |reads| (reads < MAX_READS).then_some(reads + 1))
            .map(|_| ())
            .map_err(|_| Error::Busy)
    }
    pub fn block(&self, block: &ContentBlock) -> Result<Vec<u8>, Error> {
        self.charge()?;
        let (bytes, permit) = self.blocks.content_block(&self.custody, block)?;
        self.permits.lock().expect("never poisoned").push(permit);
        Ok(bytes)
    }
    pub fn page(&self, root: &ManifestRoot) -> Result<ManifestPage, Error> {
        self.charge()?;
        let reply = self.blocks.content_manifest(&self.custody, root)?;
        crate::blocks::manifest::validate(&self.custody.object.scope, reply.clone())?;
        serde_json::from_str(&reply.page_json).map_err(|error| Error::Corrupt(error.to_string()))
    }
    pub fn metadata(&self, root: &ManifestRoot, page: &ManifestPage) -> Result<Option<Vec<u8>>, Error> {
        let ManifestPage::Metadata { codec, payload, references } = page else {
            return Ok(None);
        };
        let bytes = self.block(payload)?;
        crate::blocks::metadata::validate(&root.0.domain, *codec, payload, &bytes, references)?;
        Ok(Some(bytes))
    }
    /// An ownership witness retains only the selected reference, not its parent's payload.
    pub fn validate_metadata(&self, root: &ManifestRoot, page: &ManifestPage) -> Result<(), Error> {
        let ManifestPage::Metadata { codec, payload, references } = page else {
            return Ok(());
        };
        self.charge()?;
        let (bytes, _permit) = self.blocks.content_block(&self.custody, payload)?;
        crate::blocks::metadata::validate(&root.0.domain, *codec, payload, &bytes, references)
    }
}
impl Drop for Io<'_> {
    fn drop(&mut self) {
        let total = self.reads();
        let ownership = self.ownership.load(Ordering::Relaxed).min(total);
        PlatformMetrics.add(Counter::ContentOwnershipReads, Cause::Ordinary, ownership as u64);
        PlatformMetrics.add(Counter::ContentBodyReads, Cause::Ordinary, (total - ownership) as u64);
    }
}
impl BodyTreeIo for Io<'_> {
    fn read_manifest(&self, root: ManifestRoot) -> FutureResult<'_, ManifestReply> {
        Box::pin(async move {
            self.charge()?;
            self.blocks.content_manifest(&self.custody, &root)
        })
    }
    fn read_block(&self, block: ContentBlock) -> FutureResult<'_, Vec<u8>> {
        Box::pin(async move { self.block(&block) })
    }
    fn put_manifest(&self, _: ManifestReply) -> FutureResult<'_, ()> {
        Box::pin(async { Err(Error::Denied("a timeline reader cannot publish content".into())) })
    }
    fn put_block(&self, _: Vec<u8>) -> FutureResult<'_, ContentBlock> {
        Box::pin(async { Err(Error::Denied("a timeline reader cannot publish content".into())) })
    }
}
