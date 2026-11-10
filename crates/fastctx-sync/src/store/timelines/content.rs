//! Item membership is proved at the held snapshot before object custody reads.
mod io;
mod ownership;
mod parts;
use super::{SqliteTimelines, state};
use crate::{
    conversation::body_tree::{self, BodyExtent, BodyTreeError},
    cursor::ByteCount,
    manifest::{ContentBlock, ContentRef},
    store::{Error, access::AccessContext, sql},
    timeline::{ItemRead, TimelineContentRead, TimelineContentResult, TimelineReader},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fastctx_types::sync::{PAGE_BYTES, transport_codec::bounded_json};
use io::Io;

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
        ownership::validate_path(&request.fence.conversation.scope, &request.content, &request.path)?;
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
        let io = Io::new(&self.blocks, custody);
        let result = async {
            let owned = ownership::contains(&io, &item, &request.content, &request.path);
            io.finish_ownership();
            if !owned? {
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
