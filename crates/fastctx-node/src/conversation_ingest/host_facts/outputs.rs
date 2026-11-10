//! Complete MCP content, including extension fields, plus typed World headers.
use super::content;
use crate::conversation_ingest::step_content::StepContent;
use fastctx_hosts::claude::ingest::{
    document::{
        Address, Span,
        read::{Reader, exact},
    },
    semantic::{block::only_fields, fields, message::string},
};
use fastctx_sync::store::Error;
use fastctx_types::{
    MachineName,
    sync::{conversation_timeline::*, cursor::ByteCount, manifest::ContentRef},
};

pub struct Output {
    pub parts: Option<ContentRef>,
    pub machine: Option<MachineName>,
    pub colleague: Option<ContentRef>,
}
pub async fn header(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    parts: Option<ContentRef>,
    head: Option<Span>,
) -> Result<Output, Error> {
    let mut result = Output { parts, machine: None, colleague: None };
    if let Some(head) = head {
        let prefix = fields::prefix(reader, head, 32).await.map_err(super::error)?;
        if prefix.starts_with("=== colleague ") {
            let after = after(head, "=== colleague ".len() as u64);
            let end = find(reader, after, b" ").await?.unwrap_or(after.bytes.0);
            result.colleague = Some(content::text(io, reader, Some(Span { bytes: ByteCount(end), ..after })).await?);
        }
        let rest = prefix.starts_with("=== ").then(|| after(head, 4));
        if let Some(rest) = rest
            && let Some(on) = find(reader, rest, b" on ").await?
        {
            let name = after(rest, on + 4);
            if let Some(end) = find(reader, name, b" ").await? {
                let name = Span { bytes: ByteCount(end), ..name };
                result.machine = reader.text(name, 256).await.map_err(super::error)?.and_then(|name| name.parse().ok());
            }
        }
    }
    Ok(result)
}
pub(super) async fn block(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    address: Address,
) -> Result<(ToolOutputV1, Option<Span>), Error> {
    let kind = match string(reader, address, "type").await.map_err(super::error)? {
        Some(span) => reader.text(span, 64).await.map_err(super::error)?,
        None => None,
    };
    let text = if kind.as_deref() == Some("text") {
        string(reader, address, "text").await.map_err(super::error)?
    } else {
        None
    };
    if let Some(text) = text
        && only_fields(reader, address, &["type", "text"]).await.map_err(super::error)?
    {
        return Ok((ToolOutputV1::Text { text: content::text(io, reader, Some(text)).await? }, Some(text)));
    }
    if kind.as_deref() == Some("image")
        && only_fields(reader, address, &["type", "source"]).await.map_err(super::error)?
        && let Some(source) = fields::object(reader, address, "source").await.map_err(super::error)?
        && only_fields(reader, source, &["type", "media_type", "data"]).await.map_err(super::error)?
        && fields::summary(reader, source, "type", 32).await.map_err(super::error)?.as_deref() == Some("base64")
        && let (Some(mime), Some(data)) = (
            string(reader, source, "media_type").await.map_err(super::error)?,
            string(reader, source, "data").await.map_err(super::error)?,
        )
    {
        return Ok((
            ToolOutputV1::Image {
                mime: content::text(io, reader, Some(mime)).await?,
                base64: content::text(io, reader, Some(data)).await?,
            },
            None,
        ));
    }
    Ok((
        ToolOutputV1::Other {
            block: content::raw(io, reader, reader.node(address).await.map_err(super::error)?.raw).await?,
        },
        text,
    ))
}
fn after(span: Span, bytes: u64) -> Span {
    Span { offset: ByteCount(span.offset.0 + bytes), bytes: ByteCount(span.bytes.0 - bytes) }
}
async fn find(reader: &Reader<'_>, span: Span, needle: &[u8]) -> Result<Option<u64>, Error> {
    let mut at = 0;
    let mut matched = 0;
    while at < span.bytes.0 {
        let bytes = exact(
            reader.input,
            &reader.document.bodies.strings,
            span.offset.0 + at,
            (span.bytes.0 - at).min(64 * 1024) as usize,
        )
        .await
        .map_err(super::error)?;
        for (index, &byte) in bytes.iter().enumerate() {
            if byte == needle[matched] {
                matched += 1;
            } else {
                matched = usize::from(byte == needle[0]);
            }
            if matched == needle.len() {
                return Ok(Some(at + index as u64 + 1 - needle.len() as u64));
            }
        }
        at += bytes.len() as u64;
    }
    Ok(None)
}
