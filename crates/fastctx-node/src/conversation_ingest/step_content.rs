//! Current-step bytes stay in the admitted working set until checkpoint commit.
//! Private prior state is read at one fixed revision; public content has a
//! separate object-authorized read path. Neither path opens staged upload files.
use fastctx_sync::{
    blocks::{codec, manifest},
    cursor::Revision,
    identity::Digest,
    manifest::{BlobId, ContentBlock, Encoding, ManifestReply, ManifestRoot},
    store::{
        Error,
        ingest::{SourceIngest, StateRead, StateReply},
        ingest_memory::{MAX_SOURCE_BYTES, SourceAdmission, SourceWindow},
        source_step::SourceStepTarget,
    },
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Decoder buffers, codec copies, one read page and a bounded planning scratch
/// remain outside the cache, but inside the same complete source window.
const SCRATCH_BYTES: usize = 6 * 1024 * 1024;
const DECODER_BUFFERS: usize = 3 * 1024 * 1024;
/// Conservative map, identifier and reference-count allocation cost per entry.
const ENTRY_BYTES: usize = 1024;

#[derive(Default)]
struct Cache {
    blocks: BTreeMap<BlobId, Arc<[u8]>>,
    pages: BTreeMap<ManifestRoot, Arc<ManifestReply>>,
    bytes: usize,
}
pub struct StepContent<'a> {
    source: &'a dyn SourceIngest,
    target: SourceStepTarget,
    checkpoint: Option<Revision>,
    window: Arc<SourceWindow>,
    cache: Mutex<Cache>,
}
/// Keeps the source window alive while the canonical merger borrows new content.
pub struct SuppliedContent {
    cache: Cache,
    _window: Arc<SourceWindow>,
}
impl SuppliedContent {
    /// Caller reserves its input events, checkpoint and commit metadata before
    /// assigning the remainder to the canonical merger's bounded work meter.
    pub fn work_bytes(&self, reserved: usize) -> usize {
        self._window.bytes().saturating_sub(self.cache.bytes).saturating_sub(reserved)
    }
    pub fn block(&self, block: &ContentBlock) -> Option<&[u8]> {
        self.cache.blocks.get(&block.id).filter(|b| b.len() == block.decoded_bytes as usize).map(AsRef::as_ref)
    }
    pub fn manifest(&self, root: &ManifestRoot) -> Option<&ManifestReply> {
        self.cache.pages.get(root).map(AsRef::as_ref)
    }
}
impl<'a> StepContent<'a> {
    pub async fn open(
        source: &'a dyn SourceIngest,
        target: SourceStepTarget,
        checkpoint: Option<Revision>,
    ) -> Result<Self, Error> {
        let memory = source.memory(target.step())?;
        let window =
            match memory.acquire(MAX_SOURCE_BYTES, tokio::time::Instant::now() + Duration::from_secs(30)).await? {
                SourceAdmission::Granted(window) => window,
                SourceAdmission::ReleaseRequired => return Err(Error::Busy),
            };
        Ok(Self { source, target, checkpoint, window: Arc::new(window), cache: Mutex::new(Cache::default()) })
    }
    pub fn target(&self) -> &SourceStepTarget {
        &self.target
    }
    /// Ends the mutable upload phase. Consuming it prevents multiple retained
    /// cache generations from escaping under one source memory permit.
    pub fn snapshot(self) -> SuppliedContent {
        SuppliedContent { cache: self.cache.into_inner().expect("never poisoned"), _window: self.window }
    }
    /// Flush before the three decoder tails would exceed the soft working bound.
    pub fn should_flush(&self) -> bool {
        self.cache.lock().expect("never poisoned").bytes.saturating_add(DECODER_BUFFERS)
            >= self.window.flush_threshold()
    }
    pub fn cached_bytes(&self) -> usize {
        self.cache.lock().expect("never poisoned").bytes
    }
    pub async fn put_block(&self, bytes: Vec<u8>) -> Result<ContentBlock, Error> {
        let encoded = codec::encode(self.target.target().custody.object.scope.clone(), bytes, Encoding::Raw)?;
        let content = encoded.representation.content.clone();
        {
            let mut cache = self.cache.lock().expect("never poisoned");
            if !cache.blocks.contains_key(&content.id) {
                let cost = encoded.bytes.len().checked_add(ENTRY_BYTES).ok_or(Error::Busy)?;
                self.reserve(&mut cache, cost)?;
                // Raw encoding preserves bytes. Cache decoded content, never a
                // representation header or a compression-dependent coordinate.
                cache.blocks.insert(content.id.clone(), Arc::from(encoded.bytes.clone()));
            }
        }
        self.source.put_block(self.target.clone(), encoded.representation, encoded.bytes).await?;
        Ok(content)
    }
    pub async fn put_manifest(&self, page: ManifestReply) -> Result<(), Error> {
        manifest::validate(&self.target.target().custody.object.scope, page.clone())?;
        {
            let mut cache = self.cache.lock().expect("never poisoned");
            if !cache.pages.contains_key(&page.root) {
                let cost = page.page_json.len().checked_add(ENTRY_BYTES).ok_or(Error::Busy)?;
                self.reserve(&mut cache, cost)?;
                cache.pages.insert(page.root.clone(), Arc::new(page.clone()));
            }
        }
        self.source.put_manifest(self.target.clone(), page).await?;
        Ok(())
    }
    fn reserve(&self, cache: &mut Cache, bytes: usize) -> Result<(), Error> {
        let used = cache.bytes.checked_add(bytes).ok_or(Error::Busy)?;
        if used > self.window.bytes().saturating_sub(SCRATCH_BYTES) {
            return Err(Error::Busy);
        }
        cache.bytes = used;
        Ok(())
    }
    fn cached_block(&self, block: &ContentBlock) -> Option<Arc<[u8]>> {
        self.cache
            .lock()
            .expect("never poisoned")
            .blocks
            .get(&block.id)
            .filter(|bytes| bytes.len() == block.decoded_bytes as usize)
            .cloned()
    }
    fn cached_manifest(&self, root: &ManifestRoot) -> Option<Arc<ManifestReply>> {
        self.cache.lock().expect("never poisoned").pages.get(root).cloned()
    }
    pub async fn state_block(&self, block: ContentBlock) -> Result<Vec<u8>, Error> {
        if let Some(bytes) = self.cached_block(&block) {
            return Ok(bytes.to_vec());
        }
        let checkpoint =
            self.checkpoint.ok_or_else(|| Error::Invalid("there is no previous parser checkpoint".into()))?;
        let reply = self
            .source
            .read_state(self.target.target().clone(), StateRead::Block { checkpoint, id: block.id.clone() })
            .await?;
        let bytes = match reply {
            StateReply::Bytes(bytes) => bytes,
            StateReply::Superseded { current } => {
                return Err(Error::LocalConflict { current: current.unwrap_or(Revision(0)) });
            }
            _ => return Err(Error::Corrupt("state reader returned a non-block reply".into())),
        };
        checked_bytes(&block, bytes)
    }
    pub async fn state_manifest(&self, root: ManifestRoot) -> Result<ManifestReply, Error> {
        if let Some(page) = self.cached_manifest(&root) {
            return Ok((*page).clone());
        }
        let checkpoint =
            self.checkpoint.ok_or_else(|| Error::Invalid("there is no previous parser checkpoint".into()))?;
        let reply = self
            .source
            .read_state(self.target.target().clone(), StateRead::Manifest { checkpoint, root: root.clone() })
            .await?;
        match reply {
            StateReply::Manifest(page) if page.root == root => Ok(page),
            StateReply::Superseded { current } => Err(Error::LocalConflict { current: current.unwrap_or(Revision(0)) }),
            _ => Err(Error::Corrupt("state reader returned a different manifest".into())),
        }
    }
    pub async fn public_block(&self, block: ContentBlock) -> Result<Vec<u8>, Error> {
        if let Some(bytes) = self.cached_block(&block) {
            return Ok(bytes.to_vec());
        }
        self.source.read_block(self.target.target().clone(), block).await
    }
    pub async fn public_manifest(&self, root: ManifestRoot) -> Result<ManifestReply, Error> {
        if let Some(page) = self.cached_manifest(&root) {
            return Ok((*page).clone());
        }
        self.source.read_manifest(self.target.target().clone(), root).await
    }
    /// Re-supply prior private content without caching a second full copy. The
    /// previous checkpoint remains the read authority until this step commits.
    pub(super) async fn supply_block(&self, bytes: Vec<u8>) -> Result<(), Error> {
        let encoded = codec::encode(self.target.target().custody.object.scope.clone(), bytes, Encoding::Raw)?;
        self.source.put_block(self.target.clone(), encoded.representation, encoded.bytes).await?;
        Ok(())
    }
    pub(super) async fn supply_manifest(&self, page: ManifestReply) -> Result<(), Error> {
        self.source.put_manifest(self.target.clone(), page).await?;
        Ok(())
    }
}
fn checked_bytes(block: &ContentBlock, bytes: Vec<u8>) -> Result<Vec<u8>, Error> {
    if bytes.len() != block.decoded_bytes as usize || Digest::of(&bytes) != block.id.digest {
        return Err(Error::Corrupt("source state block differs from its declared extent".into()));
    }
    Ok(bytes)
}

impl fastctx_sync::conversation::producer::SuppliedContent for SuppliedContent {
    fn block(&self, block: &ContentBlock) -> Option<&[u8]> {
        self.block(block)
    }
    fn manifest(&self, root: &ManifestRoot) -> Option<&ManifestReply> {
        self.manifest(root)
    }
}
