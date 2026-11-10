//! Item membership is proved at the held snapshot before object custody reads.
mod ownership;
mod parts;
use super::{SqliteTimelines, state};
use crate::{
    blocks::SqliteBlocks,
    conversation::body_tree::{self, BodyExtent, BodyTreeError, BodyTreeIo},
    cursor::ByteCount,
    manifest::{ContentBlock, ContentRef, ManifestPage, ManifestReply, ManifestRoot},
    store::{
        Error, FutureResult,
        access::{AccessContext, Custody},
        sql,
    },
    timeline::{ItemRead, TimelineContentRead, TimelineContentResult, TimelineReader},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fastctx_types::sync::{PAGE_BYTES, transport_codec::bounded_json};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Io<'a> {
    blocks: &'a SqliteBlocks,
    custody: Custody,
    reads: AtomicUsize,
    permits: Mutex<Vec<crate::admission::BytePermit>>,
}
impl Io<'_> {
    fn charge(&self) -> Result<(), Error> {
        if self.reads.fetch_add(1, Ordering::Relaxed) >= 1024 {
            return Err(Error::Busy);
        }
        Ok(())
    }
    fn block(&self, block: &ContentBlock) -> Result<Vec<u8>, Error> {
        self.charge()?;
        let (bytes, permit) = self.blocks.content_block(&self.custody, block)?;
        self.permits.lock().expect("never poisoned").push(permit);
        Ok(bytes)
    }
    fn page(&self, root: &ManifestRoot) -> Result<ManifestPage, Error> {
        self.charge()?;
        let reply = self.blocks.content_manifest(&self.custody, root)?;
        crate::blocks::manifest::validate(&self.custody.object.scope, reply.clone())?;
        serde_json::from_str(&reply.page_json).map_err(|e| Error::Corrupt(e.to_string()))
    }
    fn metadata(&self, root: &ManifestRoot, page: &ManifestPage) -> Result<Option<Vec<u8>>, Error> {
        let ManifestPage::Metadata { codec, payload, references } = page else {
            return Ok(None);
        };
        let bytes = self.block(payload)?;
        crate::blocks::metadata::validate(&root.0.domain, *codec, payload, &bytes, references)?;
        Ok(Some(bytes))
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
impl SqliteTimelines {
    pub(super) async fn read_content(
        &self,
        context: AccessContext,
        request: TimelineContentRead,
    ) -> Result<TimelineContentResult, Error> {
        self.authorize(&context, &request.fence.conversation)?;
        if !self.held(&request.fence).await? {
            return Ok(TimelineContentResult::ReanchorNeeded);
        }
        request.content.validate().map_err(Error::Invalid)?;
        if request.content.id().domain != request.fence.conversation.scope {
            return Err(Error::Denied("the content belongs to another scope".into()));
        }
        let item = match self.item(context.clone(), request.fence.clone(), request.item.clone()).await? {
            ItemRead::Item { item: Some(item) } => item,
            ItemRead::Item { item: None } => return Err(Error::Denied("the item is absent from this snapshot".into())),
            ItemRead::Pending { progress } => return Ok(TimelineContentResult::Pending { progress }),
            ItemRead::ReanchorNeeded => return Ok(TimelineContentResult::ReanchorNeeded),
        };
        let conversation = request.fence.conversation.clone();
        let holder = self.blocks.holder().clone();
        let mut custody = self
            .blocks
            .database()
            .read(move |db| state::custody(db, &holder, &conversation))
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
        custody.context = context.clone();
        let io = Io { blocks: &self.blocks, custody, reads: AtomicUsize::new(0), permits: Mutex::new(Vec::new()) };
        let result = async {
            if !ownership::contains(&io, &item, &request.content)? {
                return Err(Error::Denied("the content is not owned by this item at the fence".into()));
            }
            logical(&io, &request).await
        }
        .await;
        self.authorize(&context, &request.fence.conversation)?;
        if !self.held(&request.fence).await? {
            return Ok(TimelineContentResult::ReanchorNeeded);
        }
        match result {
            Err(Error::Unavailable(_)) => Ok(TimelineContentResult::Pending { progress: Some(request.fence.through) }),
            result => result,
        }
    }
}
async fn logical(io: &Io<'_>, request: &TimelineContentRead) -> Result<TimelineContentResult, Error> {
    let _output = io.blocks.buffer_budget().try_acquire(PAGE_BYTES as usize * 2)?;
    let (payload, extent, origin) = match &request.content {
        ContentRef::Single { block, decoded_bytes } => (
            Some(io.block(&ContentBlock { id: block.clone(), decoded_bytes: *decoded_bytes })?),
            u64::from(*decoded_bytes),
            ByteCount(0),
        ),
        ContentRef::Manifest { root } => {
            let page = io.page(root)?;
            if let Some(bytes) = io.metadata(root, &page)? {
                let length = bytes.len() as u64;
                (Some(bytes), length, ByteCount(0))
            } else if let Some(bytes) = parts::logical(&page)? {
                let length = bytes.len() as u64;
                (Some(bytes), length, ByteCount(0))
            } else {
                let (origin, bytes) = body_tree::page_extent(&page).map_err(tree_error)?;
                (None, bytes.0, origin)
            }
        }
    };
    let offset = request.range.map_or(ByteCount(0), |range| range.offset);
    let wanted = request.range.map_or(PAGE_BYTES, |range| range.bytes);
    if wanted == 0 || wanted > PAGE_BYTES || offset.0 > extent {
        return Err(Error::Invalid("content ranges need 1..PAGE_BYTES bytes and an in-bounds offset".into()));
    }
    let empty = TimelineContentResult::Content {
        base64: String::new(),
        offset,
        total: ByteCount(extent),
        next: Some(ByteCount(u64::MAX)),
    };
    let overhead = bounded_json(&empty, PAGE_BYTES as usize).map_err(|_| Error::Busy)?.len();
    let raw_bound = (PAGE_BYTES as usize - overhead) / 4 * 3;
    let length = (extent - offset.0).min(u64::from(wanted)).min(raw_bound as u64) as usize;
    let bytes = match payload {
        Some(bytes) => bytes[offset.0 as usize..offset.0 as usize + length].to_vec(),
        None => body_tree::read_range_at(
            io,
            &BodyExtent { content: request.content.clone(), bytes: ByteCount(extent) },
            origin,
            offset,
            length,
            raw_bound,
        )
        .await
        .map_err(tree_error)?,
    };
    let end = offset.0 + bytes.len() as u64;
    Ok(TimelineContentResult::Content {
        base64: STANDARD.encode(bytes),
        offset,
        total: ByteCount(extent),
        next: (end < extent).then_some(ByteCount(end)),
    })
}
fn tree_error(error: BodyTreeError) -> Error {
    match error {
        BodyTreeError::Store(error) => error,
        BodyTreeError::Budget { .. } => Error::Busy,
        BodyTreeError::InvalidInput(reason) => Error::Invalid(reason),
        other => Error::Corrupt(other.to_string()),
    }
}
