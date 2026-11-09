//! A selective, disk-backed JSON index. Three append-only bodies preserve raw
//! bytes, selected decoded scalars and compact node descriptors. Unknown subtrees
//! remain opaque raw spans. Index bytes never exceed twice the row bytes plus
//! 4 KiB; exhausted rows retain their evidence with an explicit unindexed root.
//! Images are decoded by semantics only after the complete row establishes type,
//! independent of JSON member order. Descriptors never hide content roots.
//! Duplicate object keys retain their order; a reader selects the last matching
//! member, just as the legacy JSON decoder does.

mod build;
mod codec;
pub mod read;
mod select;
use super::{
    body::{BodyCursor, BodyKey, BodyPart, BodySink, BufferedBody},
    lexical::{Lexer, WINDOW_BYTES},
};
use fastctx_types::sync::cursor::{ByteCount, Count};
use serde::{Deserialize, Serialize};
use std::io;

pub const NODE_BYTES: usize = 256;
/// Hard bound, including unreachable descriptors from a row that lost its index.
pub fn index_limit(raw_bytes: u64) -> u64 {
    raw_bytes.saturating_mul(2).saturating_add(4096)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unindexed {
    NotSelected,
    RowBudget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub offset: ByteCount,
    pub bytes: ByteCount,
}
impl Span {
    pub fn end(self) -> io::Result<u64> {
        self.offset.0.checked_add(self.bytes.0).ok_or_else(|| io::Error::other("JSON span overflow"))
    }
}
/// Byte address of a length-prefixed descriptor in the node body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Address(pub ByteCount);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Value {
    Object { last: Option<Address>, members: Count },
    Array { last: Option<Address>, members: Count },
    String { text: Span },
    Number { text: Span },
    Bool { value: bool },
    Null,
    Opaque { shape: Shape, reason: Unindexed },
}
impl Value {
    pub fn shape(&self) -> Shape {
        match self {
            Self::Object { .. } => Shape::Object,
            Self::Array { .. } => Shape::Array,
            Self::String { .. } => Shape::String,
            Self::Number { .. } => Shape::Number,
            Self::Bool { .. } => Shape::Bool,
            Self::Null => Shape::Null,
            Self::Opaque { shape, .. } => *shape,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Exact JSON value bytes inside the preserved raw row, excluding whitespace.
    pub raw: Span,
    pub name: Option<Span>,
    pub ordinal: Count,
    pub previous: Option<Address>,
    pub value: Value,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bodies {
    pub raw: BodyCursor,
    pub strings: BodyCursor,
    pub nodes: BodyCursor,
}
/// These roots must be explicit edges of the checkpoint/observation manifest.
/// Serializing the struct as an opaque block alone does not retain its bodies.
impl Bodies {
    pub fn roots(&self) -> [&fastctx_types::sync::manifest::ContentRef; 3] {
        [&self.raw.content, &self.strings.content, &self.nodes.content]
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatedDocument {
    pub bodies: Bodies,
    pub root: Address,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialDocument {
    pub key: BodyKey,
    lexer: Lexer,
    build: build::State,
    pub bodies: Bodies,
    failure: Option<String>,
}

pub struct Decoder {
    key: BodyKey,
    lexer: Lexer,
    build: build::State,
    raw: BufferedBody,
    strings: BufferedBody,
    nodes: BufferedBody,
    failure: Option<String>,
}
impl Decoder {
    pub fn new(key: BodyKey) -> Self {
        let body = |part| BufferedBody::new(BodyKey { part, ..key.clone() }, None);
        Self {
            raw: body(BodyPart::Raw),
            strings: body(BodyPart::Strings),
            nodes: body(BodyPart::Index),
            key,
            lexer: Lexer::new(128),
            build: build::State::default(),
            failure: None,
        }
    }
    pub fn resume(state: PartialDocument) -> io::Result<Self> {
        state.lexer.validate_checkpoint().map_err(io::Error::other)?;
        state.build.validate()?;
        if state.bodies.nodes.bytes.0 > index_limit(state.bodies.raw.bytes.0)
            || state.bodies.strings.bytes > state.bodies.raw.bytes
        {
            return Err(io::Error::other("saved JSON bodies exceed their row budget"));
        }
        let body = |part, stored| BufferedBody::new(BodyKey { part, ..state.key.clone() }, Some(stored));
        Ok(Self {
            raw: body(BodyPart::Raw, state.bodies.raw),
            strings: body(BodyPart::Strings, state.bodies.strings),
            nodes: body(BodyPart::Index, state.bodies.nodes),
            key: state.key,
            lexer: state.lexer,
            build: state.build,
            failure: state.failure,
        })
    }
    pub fn bytes(&self) -> io::Result<u64> {
        self.raw.bytes()
    }
    pub fn is_empty(&self) -> bool {
        self.lexer.is_empty() && self.failure.is_none()
    }
    /// Input is one framing window without LF. After an IO error discard this
    /// decoder and resume the last committed checkpoint; no partial IO is a fact.
    pub async fn push(&mut self, input: &[u8], sink: &mut dyn BodySink) -> io::Result<()> {
        if input.len() > WINDOW_BYTES {
            return Err(io::Error::other("JSON input exceeds one window"));
        }
        let start = self.raw.bytes()?;
        self.raw.push(input, sink).await?;
        if self.failure.is_some() {
            return Ok(());
        }
        let mut used = 0;
        while used < input.len() {
            let step = match self.lexer.step(&input[used..]) {
                Ok(step) => step,
                Err(error) => {
                    self.failure = Some(error.to_string());
                    break;
                }
            };
            used += step.used;
            if let Some(event) = step.event {
                self.build
                    .event(event, ByteCount(start + used as u64), &mut self.strings, &mut self.nodes, sink)
                    .await?;
            } else if step.used == 0 {
                return Err(io::Error::other("JSON lexer made no progress"));
            }
        }
        Ok(())
    }
    async fn bodies(&mut self, sink: &mut dyn BodySink) -> io::Result<Bodies> {
        Ok(Bodies {
            raw: self.raw.flush(sink).await?,
            strings: self.strings.flush(sink).await?,
            nodes: self.nodes.flush(sink).await?,
        })
    }
    pub async fn checkpoint(&mut self, sink: &mut dyn BodySink) -> io::Result<PartialDocument> {
        let bodies = self.bodies(sink).await?;
        Ok(PartialDocument {
            key: self.key.clone(),
            lexer: self.lexer.clone(),
            build: self.build.clone(),
            bodies,
            failure: self.failure.clone(),
        })
    }
    /// The framing layer alone may call this, after LF or an explicitly sealed,
    /// stable EOF. Syntax failure returns preserved bytes without an observation.
    pub async fn finish(mut self, sink: &mut dyn BodySink) -> io::Result<Completion> {
        if self.failure.is_none() {
            match self.lexer.finish() {
                Ok(Some(event)) => {
                    self.build
                        .event(event, ByteCount(self.raw.bytes()?), &mut self.strings, &mut self.nodes, sink)
                        .await?
                }
                Ok(None) => {}
                Err(error) => self.failure = Some(error.to_string()),
            }
        }
        let bodies = self.bodies(sink).await?;
        if let Some(reason) = self.failure {
            return Ok(Completion::Invalid { bodies, reason });
        }
        let root = self.build.root.ok_or_else(|| io::Error::other("complete JSON has no root descriptor"))?;
        Ok(Completion::Valid(ValidatedDocument { bodies, root }))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Completion {
    Valid(ValidatedDocument),
    Invalid { bodies: Bodies, reason: String },
}
