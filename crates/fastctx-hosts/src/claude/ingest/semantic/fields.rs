use super::super::document::{
    Address, Node, Span, Value,
    read::{Reader, exact},
};
use std::io;

pub async fn text(reader: &Reader<'_>, object: Address, name: &str) -> io::Result<Option<String>> {
    match reader.field(object, name).await? {
        Some((_, Node { value: Value::String { text, .. }, .. })) => reader
            .text(text, 64 * 1024)
            .await?
            .map(Some)
            .ok_or_else(|| invalid(name, "metadata field exceeds one window")),
        _ => Ok(None),
    }
}
pub async fn boolean(reader: &Reader<'_>, object: Address, name: &str) -> io::Result<Option<bool>> {
    match reader.field(object, name).await? {
        Some((_, Node { value: Value::Bool { value }, .. })) => Ok(Some(value)),
        None => Ok(None),
        _ => Err(invalid(name, "expected a boolean")),
    }
}
pub async fn present(reader: &Reader<'_>, object: Address, name: &str) -> io::Result<bool> {
    Ok(reader.field(object, name).await?.is_some_and(|(_, n)| !matches!(n.value, Value::Null)))
}
pub async fn object(reader: &Reader<'_>, parent: Address, name: &str) -> io::Result<Option<Address>> {
    Ok(reader
        .field(parent, name)
        .await?
        .and_then(|(address, node)| matches!(node.value, Value::Object { .. }).then_some(address)))
}
/// Reads a bounded UTF-8 prefix without requiring its final input chunk to end
/// at a character boundary. The document lexer already validated the full text.
pub async fn prefix(reader: &Reader<'_>, span: Span, limit: usize) -> io::Result<String> {
    if limit > 64 * 1024 {
        return Err(io::Error::other("text preview exceeds one window"));
    }
    let bytes =
        exact(reader.input, &reader.document.bodies.strings, span.offset.0, span.bytes.0.min(limit as u64) as usize)
            .await?;
    let end = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(error) => return Err(io::Error::other(error)),
    };
    Ok(std::str::from_utf8(&bytes[..end]).map_err(io::Error::other)?.to_owned())
}
/// Finds the first non-whitespace prefix without materializing long padding.
pub async fn leading(reader: &Reader<'_>, span: Span, limit: usize) -> io::Result<String> {
    let mut at = 0;
    while at < span.bytes.0 {
        let remaining = Span {
            offset: fastctx_types::sync::cursor::ByteCount(span.offset.0 + at),
            bytes: fastctx_types::sync::cursor::ByteCount(span.bytes.0 - at),
        };
        let text = prefix(reader, remaining, limit.clamp(4096, 64 * 1024)).await?;
        if text.is_empty() {
            return Err(io::Error::other("UTF-8 prefix made no progress"));
        }
        let trimmed = text.trim_start();
        if !trimmed.is_empty() {
            let skipped = text.len() - trimmed.len();
            let rest = Span {
                offset: fastctx_types::sync::cursor::ByteCount(remaining.offset.0 + skipped as u64),
                bytes: fastctx_types::sync::cursor::ByteCount(remaining.bytes.0 - skipped as u64),
            };
            return prefix(reader, rest, limit).await;
        }
        at += text.len() as u64;
    }
    Ok(String::new())
}

/// A malformed host field is distinguishable from failed or corrupt state IO.
#[derive(Debug)]
pub struct FieldError {
    field: String,
    problem: &'static str,
}
impl std::fmt::Display for FieldError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}: {}", self.field, self.problem)
    }
}
impl std::error::Error for FieldError {}
fn invalid(field: &str, problem: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, FieldError { field: field.into(), problem })
}
pub fn is_field_error(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|error| error.is::<FieldError>())
}
/// A small display summary may be clipped; native identifiers never use this API.
pub async fn summary(reader: &Reader<'_>, object: Address, name: &str, limit: usize) -> io::Result<Option<String>> {
    match reader.field(object, name).await? {
        Some((_, Node { value: Value::String { text }, .. })) => prefix(reader, text, limit).await.map(Some),
        _ => Ok(None),
    }
}
