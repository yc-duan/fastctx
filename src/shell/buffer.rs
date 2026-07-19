//! Bounded line-oriented storage for foreground command output.

use crate::shell::normalize::NormalizedLine;
use crate::shell::normalize::StreamEncoding;
use std::collections::VecDeque;
use std::mem::size_of;

/// Maximum in-memory payload retained for one foreground command.
pub(crate) const OUTPUT_BUFFER_BYTES: usize = 8 * 1024 * 1024;

/// One normalized line stored with a stable, job-local sequence number.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BufferedLine {
    pub(crate) seq: u64,
    pub(crate) bytes: Vec<u8>,
    pub(crate) total_bytes: u64,
    pub(crate) stream_encoding: Option<StreamEncoding>,
    pub(crate) raw_truncated: bool,
}

impl BufferedLine {
    fn storage_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(self.bytes.len())
            .saturating_add(1)
    }
}

/// An eight-megabyte ring that evicts only complete lines and never controls process life.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LineRing {
    lines: VecDeque<BufferedLine>,
    storage_bytes: usize,
    limit_bytes: usize,
    next_seq: u64,
    had_truncation: bool,
    had_drop: bool,
}

impl LineRing {
    pub(crate) fn new() -> Self {
        Self::with_limit(OUTPUT_BUFFER_BYTES)
    }

    pub(crate) fn with_limit(limit_bytes: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            storage_bytes: 0,
            limit_bytes,
            next_seq: 1,
            had_truncation: false,
            had_drop: false,
        }
    }

    /// Adds a line and returns its monotonically increasing sequence number.
    pub(crate) fn push(&mut self, line: NormalizedLine) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.had_truncation |= line.raw_truncated;

        let buffered = BufferedLine {
            seq,
            bytes: line.bytes,
            total_bytes: line.total_bytes,
            stream_encoding: line.stream_encoding,
            raw_truncated: line.raw_truncated,
        };
        let required = buffered.storage_bytes();

        while self.storage_bytes.saturating_add(required) > self.limit_bytes {
            let Some(removed) = self.lines.pop_front() else {
                break;
            };
            self.storage_bytes = self.storage_bytes.saturating_sub(removed.storage_bytes());
            self.had_drop = true;
        }

        // A raw line prefix is bounded, but a deliberately tiny test ring can still
        // be smaller than one record. Treat it as lost rather than violating the limit.
        if required > self.limit_bytes {
            self.had_drop = true;
            return seq;
        }

        self.storage_bytes = self.storage_bytes.saturating_add(required);
        self.lines.push_back(buffered);
        seq
    }

    pub(crate) fn total_lines(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    pub(crate) fn dropped_lines(&self) -> u64 {
        self.total_lines().saturating_sub(self.lines.len() as u64)
    }

    #[cfg(test)]
    pub(crate) fn had_truncation(&self) -> bool {
        self.had_truncation
    }

    pub(crate) fn all(&self) -> Vec<BufferedLine> {
        self.lines.iter().cloned().collect()
    }
}

impl Default for LineRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::LineRing;
    use crate::shell::normalize::NormalizedLine;

    fn line(text: &str, truncated: bool) -> NormalizedLine {
        NormalizedLine {
            bytes: text.as_bytes().to_vec(),
            total_bytes: text.len() as u64,
            terminated: true,
            stream_encoding: None,
            raw_truncated: truncated,
        }
    }

    #[test]
    fn ring_evicts_only_whole_lines_and_preserves_sequence_numbers() {
        let single_line_bytes = std::mem::size_of::<super::BufferedLine>() + "one".len() + 1;
        let mut ring = LineRing::with_limit(single_line_bytes * 2);
        assert_eq!(ring.push(line("one", false)), 1);
        assert_eq!(ring.push(line("two", false)), 2);
        assert_eq!(ring.push(line("six", false)), 3);

        assert_eq!(
            ring.all()
                .into_iter()
                .map(|line| (line.seq, String::from_utf8(line.bytes).unwrap()))
                .collect::<Vec<_>>(),
            [(2, "two".to_string()), (3, "six".to_string())]
        );
        assert_eq!(ring.total_lines(), 3);
        assert_eq!(ring.dropped_lines(), 1);
        assert!(ring.had_drop);
    }

    #[test]
    fn truncation_is_a_lifetime_loss_even_without_eviction() {
        let mut ring = LineRing::new();
        ring.push(line("short", true));
        assert!(ring.had_truncation());
        assert!(!ring.had_drop);
    }
}
