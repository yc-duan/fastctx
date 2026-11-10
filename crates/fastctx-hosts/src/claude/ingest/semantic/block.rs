use super::fields;
use crate::claude::ingest::document::{Address, Span, Value, read::Reader};
use fastctx_types::sync::cursor::Count;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub address: Address,
    pub ordinal: Count,
    pub raw: Span,
    pub kind: Kind,
    pub pairing: Option<Pairing>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    Text { body: Span },
    Thinking { body: Span },
    RedactedThinking,
    ToolUse { name: String, input: Span, fields: Address },
    ToolResult { content: Option<Address>, failed: bool },
    Image { block: Address, encoded: Span, media_type: String },
    Document { source: Option<Address> },
    Other,
}
/// Invalid candidates still participate in ambiguity detection. Two occurrences
/// of one native call ID must not silently become a valid call/result pair.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pairing {
    pub id: String,
    pub call: bool,
    pub valid: bool,
}

pub async fn decode(reader: &Reader<'_>, address: Address) -> io::Result<Block> {
    let node = reader.node(address).await?;
    let mut pairing = None;
    let kind = match fields::summary(reader, address, "type", 256).await?.as_deref() {
        Some("text") => match reader.field(address, "text").await? {
            Some((_, n)) => match n.value {
                Value::String { text, .. } => Kind::Text { body: text },
                _ => Kind::Other,
            },
            None => Kind::Other,
        },
        Some("thinking") => match reader.field(address, "thinking").await? {
            Some((_, n)) => match n.value {
                Value::String { text, .. } => Kind::Thinking { body: text },
                _ => Kind::Other,
            },
            None => Kind::Other,
        },
        Some("redacted_thinking") => {
            if reader.field(address, "data").await?.is_some_and(|(_, n)| matches!(n.value, Value::String { .. })) {
                Kind::RedactedThinking
            } else {
                Kind::Other
            }
        }
        Some("tool_use") => {
            let id = fields::text(reader, address, "id").await?.filter(|id| !id.is_empty());
            let name = fields::summary(reader, address, "name", 256).await?.filter(|name| !name.is_empty());
            let input = reader.field(address, "input").await?.filter(|(_, n)| matches!(n.value, Value::Object { .. }));
            let valid = name.is_some() && input.is_some();
            pairing = id.map(|id| Pairing { id, call: true, valid });
            match (name, input) {
                (Some(name), Some((fields, input))) if pairing.is_some() => {
                    Kind::ToolUse { name, input: input.raw, fields }
                }
                _ => Kind::Other,
            }
        }
        Some("tool_result") => {
            let id = fields::text(reader, address, "tool_use_id").await?.filter(|id| !id.is_empty());
            let failed = match fields::boolean(reader, address, "is_error").await {
                Err(error) if !fields::is_field_error(&error) => return Err(error),
                result => result,
            };
            let content = reader.field(address, "content").await?;
            let valid = failed.is_ok()
                && content
                    .as_ref()
                    .is_none_or(|(_, n)| matches!(n.value, Value::String { .. } | Value::Array { .. } | Value::Null));
            pairing = id.map(|id| Pairing { id, call: false, valid });
            if valid && pairing.is_some() {
                Kind::ToolResult { content: content.map(|(a, _)| a), failed: failed.ok().flatten().unwrap_or(false) }
            } else {
                Kind::Other
            }
        }
        Some("image") => image(reader, address).await?.unwrap_or(Kind::Other),
        Some("document") => Kind::Document { source: fields::object(reader, address, "source").await? },
        _ => Kind::Other,
    };
    Ok(Block { address, ordinal: node.ordinal, raw: node.raw, kind, pairing })
}
async fn image(reader: &Reader<'_>, address: Address) -> io::Result<Option<Kind>> {
    if !only_fields(reader, address, &["type", "source"]).await? {
        return Ok(None);
    }
    let Some(source) = fields::object(reader, address, "source").await? else {
        return Ok(None);
    };
    if !only_fields(reader, source, &["type", "media_type", "data"]).await?
        || fields::text(reader, source, "type").await?.as_deref() != Some("base64")
    {
        return Ok(None);
    }
    Ok(crate::claude::ingest::image::ImageSource::from_block(reader, address).await?.map(|image| Kind::Image {
        block: address,
        encoded: image.encoded_span(),
        media_type: image.media_type().into(),
    }))
}
/// Exact known shapes ignore duplicate keys but reject extra names, matching
/// serde's last-key-wins map without collecting an unbounded key set.
pub async fn only_fields(reader: &Reader<'_>, address: Address, names: &[&str]) -> io::Result<bool> {
    let Value::Object { last, members } = reader.node(address).await?.value else {
        return Ok(false);
    };
    let mut next = last;
    let mut remaining = members.0;
    let mut found = vec![false; names.len()];
    while let Some(address) = next {
        let node = reader.node(address).await?;
        if remaining == 0 || node.ordinal.0 != remaining - 1 {
            return Err(io::Error::other("object member list is inconsistent"));
        }
        let Some(span) = node.name else {
            return Err(io::Error::other("object member has no name"));
        };
        let Some(key) = reader.text(span, 64).await? else {
            return Ok(false);
        };
        let Some(index) = names.iter().position(|&name| name == key) else {
            return Ok(false);
        };
        found[index] = true;
        remaining -= 1;
        next = node.previous;
    }
    if remaining != 0 {
        return Err(io::Error::other("object member list ended early"));
    }
    Ok(found.into_iter().all(|present| present))
}
