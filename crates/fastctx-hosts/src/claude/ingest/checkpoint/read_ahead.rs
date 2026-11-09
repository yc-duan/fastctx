use super::*;
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadAhead {
    pub start: ByteCount,
    pub bytes: u32,
    pub consumed: u32,
    pub content: ContentRef,
}
impl ReadAhead {
    pub fn end(&self) -> io::Result<ByteCount> {
        self.start
            .0
            .checked_add(u64::from(self.bytes))
            .map(ByteCount)
            .ok_or_else(|| io::Error::other("read-ahead extent overflow"))
    }
    pub(super) fn validate(&self, parsed: ByteCount, observed: ByteCount) -> io::Result<()> {
        if self.bytes == 0
            || self.bytes as usize > crate::claude::ingest::lexical::WINDOW_BYTES
            || self.consumed >= self.bytes
            || self.start.0.checked_add(u64::from(self.consumed)) != Some(parsed.0)
            || self.end()? > observed
            || !matches!(self.content, ContentRef::Single { decoded_bytes, .. } if decoded_bytes == self.bytes)
        {
            return Err(io::Error::other("read-ahead bytes differ from their bounded parsed position"));
        }
        Ok(())
    }
}
impl ClaudeCheckpointV1 {
    /// The source boundary actually held by this checkpoint, including unparsed
    /// read-ahead. Report it only after the writer acknowledges this checkpoint.
    pub fn staged_offset(&self) -> io::Result<ByteCount> {
        self.read_ahead.as_ref().map_or(Ok(self.read_offset), ReadAhead::end)
    }
}
