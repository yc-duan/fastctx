//! Host-format IO over the shared persistent byte tree. Parser state is private
//! even after commit, so its reads always carry the captured checkpoint revision.
use super::step_content::StepContent;
use fastctx_hosts::claude::ingest::{
    body::{BodyCursor, BodyFuture, BodyKey, BodySink},
    document::read::BodyRead,
};
use fastctx_sync::{
    conversation::body_tree::{self, BodyExtent, BodyTreeError, BodyTreeIo},
    cursor::ByteCount,
    manifest::{ContentBlock, ManifestReply, ManifestRoot},
    store::{Error, FutureResult},
};
use std::io;

pub struct Bodies<'a, 'source> {
    pub content: &'a StepContent<'source>,
}
impl BodyTreeIo for Bodies<'_, '_> {
    fn read_manifest(&self, root: ManifestRoot) -> FutureResult<'_, ManifestReply> {
        Box::pin(self.content.state_manifest(root))
    }
    fn read_block(&self, block: ContentBlock) -> FutureResult<'_, Vec<u8>> {
        Box::pin(self.content.state_block(block))
    }
    fn put_manifest(&self, page: ManifestReply) -> FutureResult<'_, ()> {
        Box::pin(self.content.put_manifest(page))
    }
    fn put_block(&self, bytes: Vec<u8>) -> FutureResult<'_, ContentBlock> {
        Box::pin(self.content.put_block(bytes))
    }
}
impl BodySink for Bodies<'_, '_> {
    fn append<'a>(
        &'a mut self,
        _key: &'a BodyKey,
        before: Option<&'a BodyCursor>,
        bytes: Vec<u8>,
    ) -> BodyFuture<'a, BodyCursor> {
        Box::pin(async move {
            if bytes.is_empty()
                && let Some(before) = before
            {
                return Ok(before.clone());
            }
            let block = self.content.put_block(bytes).await.map_err(store_error)?;
            let before = before.map(extent);
            let next = body_tree::append_block(self, before.as_ref(), block).await.map_err(tree_error)?;
            Ok(BodyCursor { content: next.content, bytes: next.bytes })
        })
    }
}
impl BodyRead for Bodies<'_, '_> {
    fn read<'a>(&'a self, body: &'a BodyCursor, offset: u64, bytes: usize) -> BodyFuture<'a, Vec<u8>> {
        Box::pin(async move {
            body_tree::read_range(self, &extent(body), ByteCount(offset), bytes, 64 * 1024).await.map_err(tree_error)
        })
    }
}
pub fn extent(body: &BodyCursor) -> BodyExtent {
    BodyExtent { content: body.content.clone(), bytes: body.bytes }
}
pub fn tree_error(error: BodyTreeError) -> io::Error {
    match error {
        BodyTreeError::Store(error) => store_error(error),
        other => io::Error::new(io::ErrorKind::InvalidData, other),
    }
}
pub fn store_error(error: Error) -> io::Error {
    if let Error::LocalConflict { current } = error {
        return super::state_read::Superseded { current: (current.0 != 0).then_some(current) }.into_io();
    }
    let kind = match error {
        Error::Busy => io::ErrorKind::WouldBlock,
        Error::Denied(_) | Error::Revoked => io::ErrorKind::PermissionDenied,
        Error::Corrupt(_) => io::ErrorKind::InvalidData,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}
