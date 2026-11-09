//! JSONL framing and the explicit stable-EOF boundary. A following file never
//! publishes its unterminated tail, even when its current JSON is complete.
use super::{
    body::{BodyKey, BodySink},
    document::{Completion, Decoder, PartialDocument},
    lexical::WINDOW_BYTES,
};
use fastctx_types::{
    file_identity::{FileChangeStamp, FileIdentity},
    sync::cursor::ByteCount,
};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SealReason {
    SessionEnded,
    HostCompleted,
    Replaced,
    Archived,
}
/// Both samples are from the same opened handle. The caller establishes the
/// reason from host/world evidence, never from a quiet interval alone.
#[derive(Clone, Debug)]
pub struct StableSeal {
    pub reason: SealReason,
    pub identity: FileIdentity,
    pub before: FileChangeStamp,
    pub after: FileChangeStamp,
}
impl StableSeal {
    pub fn validate(&self, identity: &FileIdentity, end: u64) -> io::Result<()> {
        if &self.identity != identity || self.before != self.after || self.after.size.0 != end {
            return Err(io::Error::other("source seal did not pass its stability check"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialLine {
    pub line: ByteCount,
    pub read_offset: ByteCount,
    pub document: PartialDocument,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub line: ByteCount,
    pub start: ByteCount,
    pub end: ByteCount,
    pub sealed: bool,
    pub content: Completion,
}
pub struct Step {
    pub used: usize,
    pub row: Option<Row>,
}
pub struct Jsonl {
    key: BodyKey,
    line: ByteCount,
    read_offset: ByteCount,
    decoder: Option<Decoder>,
}
impl Jsonl {
    pub fn new(key: BodyKey, line: ByteCount) -> Self {
        Self { read_offset: key.line_start, line, key: key.clone(), decoder: Some(Decoder::new(key)) }
    }
    pub fn resume(partial: PartialLine) -> io::Result<Self> {
        let key = partial.document.key.clone();
        if key.line_start.0.checked_add(partial.document.bodies.raw.bytes.0) != Some(partial.read_offset.0) {
            return Err(io::Error::other("partial JSONL offsets differ from its preserved bytes"));
        }
        Ok(Self {
            key,
            line: partial.line,
            read_offset: partial.read_offset,
            decoder: Some(Decoder::resume(partial.document)?),
        })
    }
    pub fn read_offset(&self) -> ByteCount {
        self.read_offset
    }
    /// Candidate complete-row boundary, persisted only with the row's effects.
    pub fn committed_offset(&self) -> ByteCount {
        self.key.line_start
    }
    /// Processes at most one window and returns after the first completed row.
    /// The caller commits its bounded effects before requesting more rows.
    pub async fn push(&mut self, input: &[u8], sink: &mut dyn BodySink) -> io::Result<Step> {
        let input = &input[..input.len().min(WINDOW_BYTES)];
        let length = input.iter().position(|&b| b == b'\n').unwrap_or(input.len());
        let decoder =
            self.decoder.as_mut().ok_or_else(|| io::Error::other("JSONL decoder needs committed-state recovery"))?;
        decoder.push(&input[..length], sink).await?;
        self.read_offset.0 =
            self.read_offset.0.checked_add(length as u64).ok_or_else(|| io::Error::other("source offset overflow"))?;
        if length == input.len() {
            return Ok(Step { used: length, row: None });
        }
        self.read_offset.0 =
            self.read_offset.0.checked_add(1).ok_or_else(|| io::Error::other("source offset overflow"))?;
        let empty = decoder.is_empty();
        let decoder = self.decoder.take().ok_or_else(|| io::Error::other("missing JSONL decoder"))?;
        let content = if empty { None } else { Some(decoder.finish(sink).await?) };
        let row = content.map(|content| Row {
            line: self.line,
            start: self.key.line_start,
            end: self.read_offset,
            sealed: false,
            content,
        });
        self.advance()?;
        Ok(Step { used: length + 1, row })
    }
    fn advance(&mut self) -> io::Result<()> {
        self.line.0 = self.line.0.checked_add(1).ok_or_else(|| io::Error::other("source line overflow"))?;
        self.key.line_start = self.read_offset;
        self.decoder = Some(Decoder::new(self.key.clone()));
        Ok(())
    }
    pub async fn checkpoint(&mut self, sink: &mut dyn BodySink) -> io::Result<PartialLine> {
        let document =
            self.decoder.as_mut().ok_or_else(|| io::Error::other("missing JSONL decoder"))?.checkpoint(sink).await?;
        Ok(PartialLine { line: self.line, read_offset: self.read_offset, document })
    }
    pub async fn seal(
        &mut self,
        identity: &FileIdentity,
        seal: &StableSeal,
        sink: &mut dyn BodySink,
    ) -> io::Result<Option<Row>> {
        seal.validate(identity, self.read_offset.0)?;
        let decoder = self.decoder.take().ok_or_else(|| io::Error::other("missing JSONL decoder"))?;
        if decoder.is_empty() {
            if decoder.bytes()? > 0 {
                self.advance()?;
            } else {
                self.decoder = Some(decoder);
            }
            return Ok(None);
        }
        let content = decoder.finish(sink).await?;
        let row = Row { line: self.line, start: self.key.line_start, end: self.read_offset, sealed: true, content };
        self.advance()?;
        Ok(Some(row))
    }
}
