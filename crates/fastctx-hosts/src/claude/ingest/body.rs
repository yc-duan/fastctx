//! The format decoder publishes bounded bytes through an injected body writer.
//! Only the node adapter knows storage, custody and pins. A saved body cursor
//! names a durable prefix; implementations append without flattening that prefix.

use fastctx_types::sync::{
    cursor::ByteCount,
    identity::{HostGeneration, SourceId},
    manifest::ContentRef,
};
use serde::{Deserialize, Serialize};
use std::{future::Future, io, pin::Pin};

pub const BLOCK_BYTES: usize = 1024 * 1024;
pub type BodyFuture<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyPart {
    Raw,
    Strings,
    Images,
    Index,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyKey {
    pub source: SourceId,
    pub generation: HostGeneration,
    pub line_start: ByteCount,
    pub part: BodyPart,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyCursor {
    pub content: ContentRef,
    pub bytes: ByteCount,
}

/// `append` retains its result before returning and never consumes ownership of
/// `before`: the preceding durable checkpoint may still need that prefix.
/// Repeating the same key/prefix/bytes must be safe. Each input is at most 1 MiB.
/// The node binds every publication in a step to the same ingestion operation;
/// its writer adopts the closure and retires upload pins atomically. This trait
/// publishes bytes only and does not implement or impersonate SourceIngest.
pub trait BodySink: Send {
    fn append<'a>(
        &'a mut self,
        key: &'a BodyKey,
        before: Option<&'a BodyCursor>,
        bytes: Vec<u8>,
    ) -> BodyFuture<'a, BodyCursor>;
}

/// One active output buffer, never an entire row. Checkpointing flushes its tail;
/// resumption uses only the saved cursor, not bytes reread from the host file.
pub struct BufferedBody {
    pub key: BodyKey,
    pub stored: Option<BodyCursor>,
    pending: Vec<u8>,
}
impl BufferedBody {
    pub fn new(key: BodyKey, stored: Option<BodyCursor>) -> Self {
        Self { key, stored, pending: Vec::new() }
    }
    pub fn bytes(&self) -> io::Result<u64> {
        self.stored
            .as_ref()
            .map_or(0, |body| body.bytes.0)
            .checked_add(self.pending.len() as u64)
            .ok_or_else(|| io::Error::other("body length overflow"))
    }
    pub async fn push(&mut self, mut bytes: &[u8], sink: &mut dyn BodySink) -> io::Result<()> {
        while !bytes.is_empty() {
            let take = bytes.len().min(BLOCK_BYTES - self.pending.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.pending.len() == BLOCK_BYTES {
                self.flush(sink).await?;
            }
        }
        Ok(())
    }
    pub async fn flush(&mut self, sink: &mut dyn BodySink) -> io::Result<BodyCursor> {
        if self.pending.is_empty()
            && let Some(stored) = &self.stored
        {
            return Ok(stored.clone());
        }
        let expected = self.bytes()?;
        let stored = sink.append(&self.key, self.stored.as_ref(), self.pending.clone()).await?;
        stored.content.validate().map_err(io::Error::other)?;
        if stored.bytes.0 != expected {
            return Err(io::Error::other("body sink returned a different prefix length"));
        }
        self.pending.clear();
        self.stored = Some(stored.clone());
        Ok(stored)
    }
}
