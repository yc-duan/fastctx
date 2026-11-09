//! Bounded navigation over a validated document. The injected range reader may
//! seek a manifest tree; it must not flatten a body to satisfy a small read.
use super::{Address, Bodies, NODE_BYTES, Node, Span, ValidatedDocument, Value};
use crate::claude::ingest::{
    body::{BodyCursor, BodyFuture},
    lexical::WINDOW_BYTES,
};
use std::io;

pub trait BodyRead: Send + Sync {
    fn read<'a>(&'a self, body: &'a BodyCursor, offset: u64, bytes: usize) -> BodyFuture<'a, Vec<u8>>;
}
pub struct Reader<'a> {
    pub document: &'a ValidatedDocument,
    pub input: &'a dyn BodyRead,
}
impl Reader<'_> {
    pub async fn node(&self, address: Address) -> io::Result<Node> {
        let body = &self.document.bodies.nodes;
        let prefix = exact(self.input, body, address.0.0, 4).await?;
        let length =
            u32::from_be_bytes(prefix.try_into().map_err(|_| io::Error::other("invalid descriptor length"))?) as usize;
        if length > NODE_BYTES {
            return Err(io::Error::other("JSON descriptor exceeds its bound"));
        }
        let offset = address.0.0.checked_add(4).ok_or_else(|| io::Error::other("descriptor offset overflow"))?;
        let bytes = exact(self.input, body, offset, length).await?;
        let node: Node = super::codec::decode(&bytes)?;
        validate(&node, address, &self.document.bodies)?;
        Ok(node)
    }
    /// The last matching key wins. Large names are compared by length first, so
    /// unknown, arbitrarily large host fields do not become allocations.
    pub async fn field(&self, object: Address, name: &str) -> io::Result<Option<(Address, Node)>> {
        let node = self.node(object).await?;
        if matches!(node.value, Value::Opaque { shape: super::Shape::Object, .. }) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "JSON subtree is not indexed; read its preserved raw span",
            ));
        }
        let Value::Object { last, members } = node.value else {
            return Ok(None);
        };
        let mut cursor = last;
        let mut remaining = members.0;
        while let Some(address) = cursor {
            if remaining == 0 {
                return Err(io::Error::other("JSON object list exceeds its declared count"));
            }
            let child = self.node(address).await?;
            if child.ordinal.0 != remaining - 1 {
                return Err(io::Error::other("JSON object ordinal is inconsistent"));
            }
            let key = child.name.ok_or_else(|| io::Error::other("JSON object member has no name"))?;
            if key.bytes.0 == name.len() as u64
                && name.len() <= WINDOW_BYTES
                && exact(self.input, &self.document.bodies.strings, key.offset.0, name.len()).await? == name.as_bytes()
            {
                return Ok(Some((address, child)));
            }
            remaining -= 1;
            cursor = child.previous;
        }
        if remaining != 0 {
            return Err(io::Error::other("JSON object list ended early"));
        }
        Ok(None)
    }
    pub async fn text(&self, span: Span, limit: usize) -> io::Result<Option<String>> {
        if limit > WINDOW_BYTES {
            return Err(io::Error::other("scalar read exceeds one window"));
        }
        if span.bytes.0 > limit as u64 {
            return Ok(None);
        }
        let bytes = exact(self.input, &self.document.bodies.strings, span.offset.0, span.bytes.0 as usize).await?;
        Ok(Some(String::from_utf8(bytes).map_err(io::Error::other)?))
    }
}
pub async fn exact(input: &dyn BodyRead, body: &BodyCursor, offset: u64, bytes: usize) -> io::Result<Vec<u8>> {
    if bytes > WINDOW_BYTES || offset.checked_add(bytes as u64).is_none_or(|end| end > body.bytes.0) {
        return Err(io::Error::other("body read exceeds its bounded range"));
    }
    let value = input.read(body, offset, bytes).await?;
    if value.len() != bytes {
        return Err(io::Error::other("short body read"));
    }
    Ok(value)
}
fn span(value: Span, body: &BodyCursor) -> io::Result<()> {
    if value.end()? > body.bytes.0 {
        return Err(io::Error::other("JSON span exceeds its body"));
    }
    Ok(())
}
fn validate(node: &Node, address: Address, bodies: &Bodies) -> io::Result<()> {
    span(node.raw, &bodies.raw)?;
    if node.previous.is_some_and(|p| p.0.0 >= address.0.0) {
        return Err(io::Error::other("JSON sibling link is not backward"));
    }
    if let Some(name) = node.name {
        span(name, &bodies.strings)?;
    }
    match node.value {
        Value::Object { last, members } | Value::Array { last, members } => {
            if last.is_some_and(|p| p.0.0 >= address.0.0) || last.is_none() != (members.0 == 0) {
                return Err(io::Error::other("JSON child list is inconsistent"));
            }
        }
        Value::String { text } | Value::Number { text } => span(text, &bodies.strings)?,
        Value::Bool { .. } | Value::Null | Value::Opaque { .. } => {}
    }
    Ok(())
}
