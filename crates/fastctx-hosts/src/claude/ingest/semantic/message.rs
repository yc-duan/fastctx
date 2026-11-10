//! Message-level observations and provider counters, without materializing a row.
use super::{fields, row::Header};
use crate::claude::ingest::document::{Address, Span, Value, read::Reader};
use fastctx_types::sync::conversation_timeline::MessageTokensV1;
use std::io;

#[derive(Clone, Debug, Default)]
pub struct Message {
    pub model: Option<Span>,
    pub usage: Option<Span>,
    pub tokens: Option<MessageTokensV1>,
    pub api_error: bool,
    pub stop_reason: Option<Span>,
    pub thinking_duration_ms: Option<u64>,
}
pub async fn decode(reader: &Reader<'_>, header: &Header) -> io::Result<Message> {
    let Some(message) = header.message else { return Ok(Message::default()) };
    let usage = reader.field(message, "usage").await?;
    let mut tokens = None;
    if let Some((address, node)) = &usage
        && matches!(node.value, Value::Object { .. })
    {
        let details = fields::object(reader, *address, "output_tokens_details").await?;
        tokens = Some(MessageTokensV1 {
            input: unsigned(reader, *address, "input_tokens").await?,
            cache_read: unsigned(reader, *address, "cache_read_input_tokens").await?,
            cache_creation: unsigned(reader, *address, "cache_creation_input_tokens").await?,
            output: unsigned(reader, *address, "output_tokens").await?,
            thinking: match details {
                Some(details) => unsigned(reader, details, "thinking_tokens").await?,
                None => None,
            },
        });
    }
    Ok(Message {
        model: string(reader, message, "model").await?,
        usage: usage.filter(|(_, node)| !matches!(node.value, Value::Null)).map(|(_, node)| node.raw),
        tokens,
        api_error: reader
            .field(reader.document.root, "isApiErrorMessage")
            .await?
            .is_some_and(|(_, node)| matches!(node.value, Value::Bool { value: true })),
        stop_reason: string(reader, message, "stop_reason").await?,
        thinking_duration_ms: unsigned(reader, reader.document.root, "thinkingDurationMs").await?,
    })
}
pub async fn string(reader: &Reader<'_>, object: Address, name: &str) -> io::Result<Option<Span>> {
    Ok(reader.field(object, name).await?.and_then(|(_, node)| match node.value {
        Value::String { text } => Some(text),
        _ => None,
    }))
}
pub async fn unsigned(reader: &Reader<'_>, object: Address, name: &str) -> io::Result<Option<u64>> {
    let Some((_, node)) = reader.field(object, name).await? else { return Ok(None) };
    let Value::Number { text } = node.value else { return Ok(None) };
    Ok(reader.text(text, 32).await?.and_then(|text| text.parse().ok()))
}
