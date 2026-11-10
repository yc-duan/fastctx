//! Line coordinates over decoded UTF-8, including lines larger than one window.
use super::fields;
use crate::claude::ingest::document::{
    Span,
    read::{Reader, exact},
};
use fastctx_types::sync::cursor::ByteCount;
use std::io;

pub struct Lines {
    span: Span,
    at: u64,
    window_at: u64,
    window: Vec<u8>,
}
impl Lines {
    pub fn new(span: Span) -> Self {
        Self { span, at: 0, window_at: 0, window: vec![] }
    }
    pub async fn next(&mut self, reader: &Reader<'_>) -> io::Result<Option<Span>> {
        self.span.end()?;
        if self.at >= self.span.bytes.0 {
            return Ok(None);
        }
        let start = self.at;
        let mut end;
        loop {
            if self.at < self.window_at || self.at >= self.window_at + self.window.len() as u64 {
                self.window_at = self.at;
                self.window = exact(
                    reader.input,
                    &reader.document.bodies.strings,
                    self.span.offset.0 + self.at,
                    (self.span.bytes.0 - self.at).min(64 * 1024) as usize,
                )
                .await?;
            }
            let local = (self.at - self.window_at) as usize;
            if let Some(lf) = self.window[local..].iter().position(|&byte| byte == b'\n') {
                end = self.at + lf as u64;
                self.at = end + 1;
                if end > start
                    && exact(reader.input, &reader.document.bodies.strings, self.span.offset.0 + end - 1, 1).await?
                        == b"\r"
                {
                    end -= 1;
                }
                break;
            }
            self.at = self.window_at + self.window.len() as u64;
            if self.at == self.span.bytes.0 {
                end = self.at;
                break;
            }
        }
        Ok(Some(Span { offset: ByteCount(self.span.offset.0 + start), bytes: ByteCount(end - start) }))
    }
}
pub async fn trim(reader: &Reader<'_>, span: Span) -> io::Result<Span> {
    let mut start = span.offset.0;
    let mut end = span.end()?;
    while start < end {
        let text =
            fields::prefix(reader, Span { offset: ByteCount(start), bytes: ByteCount(end - start) }, 64 * 1024).await?;
        let removed = text.len() - text.trim_start().len();
        start += removed as u64;
        if removed < text.len() {
            break;
        }
    }
    while start < end {
        let take = (end - start).min(64 * 1024) as usize;
        let bytes = exact(reader.input, &reader.document.bodies.strings, end - take as u64, take).await?;
        let skip = bytes.iter().take_while(|byte| (**byte & 0xc0) == 0x80).count();
        let text = std::str::from_utf8(&bytes[skip..]).map_err(io::Error::other)?;
        let removed = text.len() - text.trim_end().len();
        end -= removed as u64;
        if removed < text.len() {
            break;
        }
    }
    Ok(Span { offset: ByteCount(start), bytes: ByteCount(end - start) })
}
