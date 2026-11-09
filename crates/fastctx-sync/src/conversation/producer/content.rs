use super::api::*;
use crate::{
    conversation::body_tree::BodyTreeIo,
    identity::Digest,
    manifest::{BlobId, ContentBlock, ContentRef, DigestAlgorithm, ManifestPage, ManifestReply, ManifestRoot},
    store::{self, FutureResult, ingest::SourceIngest, source_step::SourceStepTarget},
};
use fastctx_types::sync::{conversation_timeline::ItemDisplayV1, metadata::MetadataCodec};
use std::sync::{
    Mutex,
    atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering},
};

#[derive(Clone)]
pub(super) enum Publication {
    Block { block: ContentBlock, bytes: Vec<u8> },
    Manifest(ManifestReply),
}
pub(super) struct Content<'a> {
    source: &'a dyn SourceIngest,
    target: SourceStepTarget,
    supplied: &'a dyn SuppliedContent,
    publications: Mutex<Vec<Publication>>,
    reads: AtomicU32,
    transient: AtomicUsize,
    max_reads: u32,
    max_bytes: usize,
    limit: AtomicU8,
}
impl<'a> Content<'a> {
    pub fn new(
        source: &'a dyn SourceIngest,
        target: SourceStepTarget,
        supplied: &'a dyn SuppliedContent,
        budget: &MergeBudget,
    ) -> Self {
        Self {
            source,
            target,
            supplied,
            publications: Mutex::new(Vec::new()),
            reads: AtomicU32::new(0),
            transient: AtomicUsize::new(0),
            max_reads: budget.max_content_reads,
            max_bytes: budget.max_work_bytes,
            limit: AtomicU8::new(0),
        }
    }
    fn lock(&self) -> std::result::Result<std::sync::MutexGuard<'_, Vec<Publication>>, store::Error> {
        self.publications.lock().map_err(|_| store::Error::Corrupt("publication cache lock poisoned".into()))
    }
    pub fn checkpoint(&self) -> Result<(usize, usize)> {
        Ok((self.lock()?.len(), self.transient.load(Ordering::Relaxed)))
    }
    pub fn rollback(&self, state: (usize, usize)) -> Result<()> {
        self.lock()?.truncate(state.0);
        self.transient.store(state.1, Ordering::Relaxed);
        self.limit.store(0, Ordering::Relaxed);
        Ok(())
    }
    pub fn into_publications(self) -> Result<Vec<Publication>> {
        self.publications.into_inner().map_err(|_| MergeError::InvalidInput("publication cache lock poisoned".into()))
    }
    pub fn reads(&self) -> u32 {
        self.reads.load(Ordering::Relaxed)
    }
    pub fn limit(&self) -> Option<BudgetKind> {
        match self.limit.load(Ordering::Relaxed) {
            1 => Some(BudgetKind::WorkBytes),
            2 => Some(BudgetKind::ContentReads),
            _ => None,
        }
    }
    pub fn bytes(&self) -> Result<usize> {
        Ok(Self::size(&self.lock()?) + self.transient.load(Ordering::Relaxed))
    }
    fn size(values: &[Publication]) -> usize {
        values
            .iter()
            .map(|p| match p {
                Publication::Block { bytes, .. } => bytes.capacity() + 512,
                Publication::Manifest(p) => p.page_json.capacity() + 512,
            })
            .sum()
    }
    fn charge_read(&self) -> std::result::Result<(), store::Error> {
        if self
            .reads
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| (n < self.max_reads).then(|| n + 1))
            .is_err()
        {
            self.limit.store(2, Ordering::Relaxed);
            return Err(store::Error::Busy);
        }
        Ok(())
    }
    fn charge_transient(&self, bytes: usize) -> std::result::Result<(), store::Error> {
        self.transient.fetch_max(bytes, Ordering::Relaxed);
        if (Self::size(&self.lock()?) + bytes).saturating_mul(2) > self.max_bytes {
            self.limit.store(1, Ordering::Relaxed);
            return Err(store::Error::Busy);
        }
        Ok(())
    }
    pub fn has_block(&self, block: &ContentBlock) -> Result<bool> {
        Ok(self.lock()?.iter().any(|p| matches!(p, Publication::Block { block: old, .. } if old == block)))
    }
    pub fn block_identity(&self, bytes: &[u8]) -> Result<ContentBlock> {
        if bytes.len() > fastctx_types::sync::BLOCK_BYTES as usize {
            return Err(MergeError::Limit(BudgetKind::PayloadBytes));
        }
        Ok(ContentBlock {
            id: BlobId {
                domain: self.target.target().custody.object.scope.clone(),
                algorithm: DigestAlgorithm::Sha256,
                digest: Digest::of(bytes),
            },
            decoded_bytes: bytes.len() as u32,
        })
    }
    pub fn stage_block(&self, bytes: Vec<u8>) -> Result<ContentBlock> {
        let bytes = bytes.into_boxed_slice().into_vec();
        let block = self.block_identity(&bytes)?;
        let mut values = self.lock()?;
        if values.iter().any(|p| matches!(p, Publication::Block { block: old, .. } if old == &block)) {
            return Ok(block);
        }
        if Self::size(&values)
            .saturating_add(self.transient.load(Ordering::Relaxed))
            .saturating_add(bytes.capacity() + 512)
            .saturating_mul(2)
            > self.max_bytes
        {
            self.limit.store(1, Ordering::Relaxed);
            return Err(MergeError::Limit(BudgetKind::WorkBytes));
        }
        values.push(Publication::Block { block: block.clone(), bytes });
        Ok(block)
    }
    pub fn stage_page(&self, page: ManifestPage) -> Result<ManifestRoot> {
        let page_json = serde_json::to_string(&page).map_err(|e| MergeError::InvalidInput(e.to_string()))?;
        let root = ManifestRoot(BlobId {
            domain: self.target.target().custody.object.scope.clone(),
            algorithm: DigestAlgorithm::Sha256,
            digest: Digest::of(page_json.as_bytes()),
        });
        self.stage_reply(ManifestReply { root: root.clone(), page_json })?;
        Ok(root)
    }
    fn stage_reply(&self, mut reply: ManifestReply) -> Result<()> {
        reply.page_json = String::from_utf8(reply.page_json.into_bytes().into_boxed_slice().into_vec())
            .map_err(|e| MergeError::InvalidInput(e.to_string()))?;
        crate::blocks::manifest::validate(&self.target.target().custody.object.scope, reply.clone())?;
        let mut values = self.lock()?;
        if values.iter().any(|p| matches!(p, Publication::Manifest(old) if old.root == reply.root)) {
            return Ok(());
        }
        if Self::size(&values)
            .saturating_add(self.transient.load(Ordering::Relaxed))
            .saturating_add(reply.page_json.capacity() + 512)
            .saturating_mul(2)
            > self.max_bytes
        {
            self.limit.store(1, Ordering::Relaxed);
            return Err(MergeError::Limit(BudgetKind::WorkBytes));
        }
        values.push(Publication::Manifest(reply));
        Ok(())
    }
    pub fn metadata(&self, codec: MetadataCodec, bytes: Vec<u8>, references: Vec<ContentRef>) -> Result<ContentRef> {
        let payload = self.stage_block(bytes)?;
        Ok(ContentRef::Manifest { root: self.stage_page(ManifestPage::Metadata { codec, payload, references })? })
    }
    pub async fn metadata_bytes(&self, content: &ContentRef, expected: MetadataCodec) -> Result<Vec<u8>> {
        let ContentRef::Manifest { root } = content else {
            return Err(MergeError::InvalidInput("descriptor must be an explicit Metadata manifest".into()));
        };
        match self.page(root.clone()).await? {
            ManifestPage::Metadata { codec, payload, references } if codec == expected => {
                let bytes = self.read_block(payload.clone()).await?;
                crate::blocks::metadata::validate(&root.0.domain, codec, &payload, &bytes, &references)?;
                Ok(bytes)
            }
            _ => Err(MergeError::InvalidInput("descriptor metadata codec differs".into())),
        }
    }
    pub async fn display(&self, content: &ContentRef) -> Result<ItemDisplayV1> {
        ItemDisplayV1::decode(&self.metadata_bytes(content, MetadataCodec::DisplayV1).await?)
            .map_err(MergeError::InvalidInput)
    }
    pub async fn page(&self, root: ManifestRoot) -> Result<ManifestPage> {
        let reply = self.read_manifest(root.clone()).await?;
        crate::blocks::manifest::validate(&root.0.domain, reply.clone())?;
        serde_json::from_str(&reply.page_json).map_err(|e| MergeError::InvalidInput(e.to_string()))
    }
}
impl BodyTreeIo for Content<'_> {
    fn read_manifest(&self, root: ManifestRoot) -> FutureResult<'_, ManifestReply> {
        Box::pin(async move {
            self.charge_read()?;
            if root.0.domain != self.target.target().custody.object.scope {
                return Err(store::Error::Invalid("content read crosses scope".into()));
            }
            let cached = self.lock()?.iter().find_map(|p| match p {
                Publication::Manifest(p) if p.root == root => Some(p.clone()),
                _ => None,
            });
            self.charge_transient(cached.as_ref().map_or(0, |p| p.page_json.len() * 3))?;
            let reply = if let Some(reply) = cached {
                reply
            } else if let Some(reply) = self.supplied.manifest(&root) {
                reply.clone()
            } else {
                self.source.read_manifest(self.target.target().clone(), root.clone()).await?
            };
            self.charge_transient(reply.page_json.len() * 3)?;
            if reply.root != root {
                return Err(store::Error::Corrupt("manifest reader changed identity".into()));
            }
            crate::blocks::manifest::validate(&root.0.domain, reply.clone())?;
            Ok(reply)
        })
    }
    fn read_block(&self, block: ContentBlock) -> FutureResult<'_, Vec<u8>> {
        Box::pin(async move {
            self.charge_read()?;
            if block.id.domain != self.target.target().custody.object.scope {
                return Err(store::Error::Invalid("content read crosses scope".into()));
            }
            self.charge_transient(block.decoded_bytes as usize * 2 + 4096)?;
            let cached = self.lock()?.iter().find_map(|p| match p {
                Publication::Block { block: b, bytes } if *b == block => Some(bytes.clone()),
                _ => None,
            });
            let bytes = if let Some(bytes) = cached {
                bytes
            } else if let Some(bytes) = self.supplied.block(&block) {
                bytes.to_vec()
            } else {
                self.source.read_block(self.target.target().clone(), block.clone()).await?
            };
            if bytes.len() != block.decoded_bytes as usize || Digest::of(&bytes) != block.id.digest {
                return Err(store::Error::Corrupt("block reader changed length or digest".into()));
            }
            Ok(bytes)
        })
    }
    fn put_manifest(&self, page: ManifestReply) -> FutureResult<'_, ()> {
        Box::pin(async move {
            self.stage_reply(page).map_err(|e| match e {
                MergeError::Store(e) => e,
                MergeError::Limit(_) => store::Error::Busy,
                _ => store::Error::Invalid(e.to_string()),
            })
        })
    }
    fn put_block(&self, bytes: Vec<u8>) -> FutureResult<'_, ContentBlock> {
        Box::pin(async move {
            self.stage_block(bytes).map_err(|e| match e {
                MergeError::Store(e) => e,
                MergeError::Limit(_) => store::Error::Busy,
                _ => store::Error::Invalid(e.to_string()),
            })
        })
    }
}
