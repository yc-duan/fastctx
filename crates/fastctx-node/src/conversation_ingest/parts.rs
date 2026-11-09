//! Realize bounded public parts from validated private spans. Reusing a block's
//! hash is not promotion: every selected byte is supplied through the new step.
use super::{
    bodies::{Bodies, extent, store_error, tree_error},
    step_content::StepContent,
};
use fastctx_hosts::claude::ingest::{
    body::{BLOCK_BYTES, BodyCursor},
    document::{Span, read::Reader},
    image::ImageSource,
};
use fastctx_sync::{
    conversation::body_tree::{self, BodyExtent},
    cursor::{ByteCount, Count},
    manifest::ContentRef,
};
use fastctx_types::sync::{
    conversation_identity::ImageSelector,
    conversation_timeline::{DisplayChannel, DisplayPart, DisplayPicture, PictureTarget},
};
use std::io;

pub async fn slice(content: &StepContent<'_>, body: &BodyCursor, span: Span) -> io::Result<BodyExtent> {
    let end = span.end()?;
    if end > body.bytes.0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "part span exceeds its private body"));
    }
    let io = Bodies { content };
    let before = extent(body);
    let mut at = span.offset;
    let mut output = None;
    while at.0 < end {
        let (start, block) = body_tree::locate(&io, &before, at).await.map_err(tree_error)?;
        let bytes = content.state_block(block).await.map_err(store_error)?;
        let local = usize::try_from(at.0 - start.0).map_err(io::Error::other)?;
        let available = bytes.len().checked_sub(local).ok_or_else(|| io::Error::other("part offset outside block"))?;
        let take = (end - at.0).min(available as u64) as usize;
        if take == 0 {
            return Err(io::Error::other("part slice made no progress"));
        }
        let bytes = if local == 0 && take == bytes.len() { bytes } else { bytes[local..local + take].to_vec() };
        let block = content.put_part(bytes).await.map_err(store_error)?;
        output = Some(body_tree::append_block(&io, output.as_ref(), block).await.map_err(tree_error)?);
        at.0 += take as u64;
    }
    match output {
        Some(output) => Ok(output),
        None => Ok(BodyExtent {
            content: content.put_part(Vec::new()).await.map_err(store_error)?.into(),
            bytes: ByteCount(0),
        }),
    }
}

pub async fn text(
    content: &StepContent<'_>,
    body: &BodyCursor,
    span: Span,
    index: Count,
    channel: DisplayChannel,
    preview_bytes: usize,
) -> io::Result<DisplayPart> {
    let preview_bytes = preview_bytes.min(4096).min(span.bytes.0.min(usize::MAX as u64) as usize);
    let bytes = body_tree::read_range(&Bodies { content }, &extent(body), span.offset, preview_bytes, 4096)
        .await
        .map_err(tree_error)?;
    let preview = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_owned(),
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(io::Error::other)?.to_owned()
        }
        Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
    };
    let output = slice(content, body, span).await?;
    Ok(DisplayPart {
        index,
        channel,
        content: output.content,
        bytes: output.bytes,
        preview: Some(preview),
        media_type: None,
        picture: None,
    })
}

pub async fn image(
    content: &StepContent<'_>,
    reader: &Reader<'_>,
    source: &ImageSource,
    index: Count,
    selector: ImageSelector,
) -> io::Result<DisplayPart> {
    let mut cursor = source.start();
    let mut output = None;
    let mut buffer = Vec::with_capacity(BLOCK_BYTES);
    while !source.complete(&cursor) {
        let bytes = source.read_window(reader, &mut cursor, 48 * 1024).await?;
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            let take = remaining.len().min(BLOCK_BYTES - buffer.len());
            buffer.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            if buffer.len() == BLOCK_BYTES {
                let block = content
                    .put_part(std::mem::replace(&mut buffer, Vec::with_capacity(BLOCK_BYTES)))
                    .await
                    .map_err(store_error)?;
                output = Some(
                    body_tree::append_block(&Bodies { content }, output.as_ref(), block).await.map_err(tree_error)?,
                );
            }
        }
    }
    if !buffer.is_empty() || output.is_none() {
        let block = content.put_part(buffer).await.map_err(store_error)?;
        output = Some(body_tree::append_block(&Bodies { content }, output.as_ref(), block).await.map_err(tree_error)?);
    }
    let output = output.ok_or_else(|| io::Error::other("image decoder produced no extent"))?;
    let reference: ContentRef = output.content;
    Ok(DisplayPart {
        index,
        channel: DisplayChannel::Image,
        content: reference.clone(),
        bytes: output.bytes,
        preview: None,
        media_type: Some(source.media_type().to_owned()),
        picture: Some(DisplayPicture { selector, target: PictureTarget::Owned { content: reference } }),
    })
}
