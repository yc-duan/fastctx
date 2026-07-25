//! Streaming terminal-output normalization with deterministic, bounded raw-byte storage.

use serde::{Deserialize, Serialize};

/// Raw bytes retained per normalized line. This covers 2,000 characters even
/// for the widest WHATWG encodings while keeping a single-line stream bounded.
const MAX_LINE_PREFIX_BYTES: usize = 64 * 1024;

/// A BOM-locked stream encoding whose code-unit width affects line splitting.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum StreamEncoding {
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
}

impl StreamEncoding {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Utf16Le => "UTF-16LE",
            Self::Utf16Be => "UTF-16BE",
            Self::Utf32Le => "UTF-32LE",
            Self::Utf32Be => "UTF-32BE",
        }
    }

    const fn unit_bytes(self) -> usize {
        match self {
            Self::Utf16Le | Self::Utf16Be => 2,
            Self::Utf32Le | Self::Utf32Be => 4,
        }
    }

    fn ascii_unit(self, bytes: &[u8]) -> Option<u8> {
        let value = match self {
            Self::Utf16Le => u32::from(u16::from_le_bytes([bytes[0], bytes[1]])),
            Self::Utf16Be => u32::from(u16::from_be_bytes([bytes[0], bytes[1]])),
            Self::Utf32Le => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            Self::Utf32Be => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        };
        u8::try_from(value).ok().filter(u8::is_ascii)
    }
}

/// One normalized output line whose source bytes remain available for delivery-time decoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NormalizedLine {
    /// Prefix after ANSI stripping and line-ending normalization.
    pub(crate) bytes: Vec<u8>,
    /// Full normalized byte count even when only a bounded prefix is retained.
    pub(crate) total_bytes: u64,
    /// True when CR, LF, or CRLF terminated this line.
    pub(crate) terminated: bool,
    /// BOM-locked encoding for wide streams; absent for ASCII-compatible streams.
    pub(crate) stream_encoding: Option<StreamEncoding>,
    /// True when bytes beyond the bounded raw prefix were discarded.
    pub(crate) raw_truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EscapeState {
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamMode {
    Bytes,
    Wide(StreamEncoding),
}

/// Streaming events used by the background-log writer. Byte events are not
/// line-bounded, so an arbitrarily long line never has to live in memory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NormalizedEvent {
    /// Announces the stream encoding after the BOM probe is settled.
    Start(Option<StreamEncoding>),
    /// Normalized content bytes with ANSI sequences and line endings removed.
    Bytes(Vec<u8>),
    /// Commits one logical line; an unterminated line is emitted only at stream end.
    LineEnd { terminated: bool },
}

#[derive(Debug, Default)]
struct LineAccumulator {
    prefix: Vec<u8>,
    total_bytes: u64,
}

impl LineAccumulator {
    fn push(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        let remaining = MAX_LINE_PREFIX_BYTES.saturating_sub(self.prefix.len());
        self.prefix
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
    }

    fn finish(self, terminated: bool, stream_encoding: Option<StreamEncoding>) -> NormalizedLine {
        NormalizedLine {
            raw_truncated: self.total_bytes > self.prefix.len() as u64,
            bytes: self.prefix,
            total_bytes: self.total_bytes,
            terminated,
            stream_encoding,
        }
    }
}

/// Stateful byte processor shared by bounded foreground capture and the complete
/// background log. It never owns a whole logical line.
#[derive(Debug)]
struct NormalizerCore {
    mode: Option<StreamMode>,
    bom_probe: Vec<u8>,
    wide_pending: Vec<u8>,
    escape: EscapeState,
    pending_cr: bool,
    emitted: Vec<u8>,
    line_has_content: bool,
}

impl NormalizerCore {
    fn new() -> Self {
        Self {
            mode: None,
            bom_probe: Vec::with_capacity(4),
            wide_pending: Vec::with_capacity(4),
            escape: EscapeState::Text,
            pending_cr: false,
            emitted: Vec::with_capacity(16 * 1024),
            line_has_content: false,
        }
    }

    fn push(&mut self, bytes: &[u8], output: &mut Vec<NormalizedEvent>) {
        if self.mode.is_none() {
            self.bom_probe.extend_from_slice(bytes);
            self.select_mode(false, output);
            return;
        }
        self.process(bytes, output);
    }

    fn finish(&mut self, output: &mut Vec<NormalizedEvent>) {
        if self.mode.is_none() {
            self.select_mode(true, output);
        }
        if !self.wide_pending.is_empty() {
            let pending = std::mem::take(&mut self.wide_pending);
            self.push_text_unit(&pending, None, output);
        }
        if self.pending_cr {
            self.emit_line(true, output);
            self.pending_cr = false;
        } else if self.line_has_content {
            self.emit_line(false, output);
        }
        self.flush_bytes(output);
    }

    fn select_mode(&mut self, final_chunk: bool, output: &mut Vec<NormalizedEvent>) {
        let Some((mode, bom_len)) = detect_stream_mode(&self.bom_probe, final_chunk) else {
            return;
        };
        self.mode = Some(mode);
        output.push(NormalizedEvent::Start(match mode {
            StreamMode::Bytes => None,
            StreamMode::Wide(encoding) => Some(encoding),
        }));
        let buffered = std::mem::take(&mut self.bom_probe);
        self.process(&buffered[bom_len.min(buffered.len())..], output);
    }

    fn process(&mut self, bytes: &[u8], output: &mut Vec<NormalizedEvent>) {
        match self
            .mode
            .expect("stream mode is selected before processing")
        {
            StreamMode::Bytes => {
                for &byte in bytes {
                    self.process_unit(&[byte], Some(byte), output);
                }
            }
            StreamMode::Wide(encoding) => {
                for &byte in bytes {
                    self.wide_pending.push(byte);
                    if self.wide_pending.len() == encoding.unit_bytes() {
                        let unit = std::mem::take(&mut self.wide_pending);
                        let ascii = encoding.ascii_unit(&unit);
                        self.process_unit(&unit, ascii, output);
                    }
                }
            }
        }
        self.flush_bytes(output);
    }

    fn process_unit(&mut self, raw: &[u8], ascii: Option<u8>, output: &mut Vec<NormalizedEvent>) {
        match self.escape {
            EscapeState::Text => {
                if ascii == Some(0x1b) {
                    self.escape = EscapeState::Escape;
                } else {
                    self.push_text_unit(raw, ascii, output);
                }
            }
            EscapeState::Escape => {
                self.escape = match ascii {
                    Some(b'[') => EscapeState::Csi,
                    Some(b']') => EscapeState::Osc,
                    _ => EscapeState::Text,
                };
            }
            EscapeState::Csi => {
                if ascii.is_some_and(|byte| (0x40..=0x7e).contains(&byte)) {
                    self.escape = EscapeState::Text;
                }
            }
            EscapeState::Osc => match ascii {
                Some(0x07) => self.escape = EscapeState::Text,
                Some(0x1b) => self.escape = EscapeState::OscEscape,
                _ => {}
            },
            EscapeState::OscEscape => {
                self.escape = match ascii {
                    Some(b'\\') => EscapeState::Text,
                    Some(0x1b) => EscapeState::OscEscape,
                    _ => EscapeState::Osc,
                };
            }
        }
    }

    fn push_text_unit(&mut self, raw: &[u8], ascii: Option<u8>, output: &mut Vec<NormalizedEvent>) {
        if self.pending_cr {
            self.emit_line(true, output);
            self.pending_cr = false;
            if ascii == Some(b'\n') {
                return;
            }
        }
        match ascii {
            Some(b'\r') => self.pending_cr = true,
            Some(b'\n') => self.emit_line(true, output),
            _ => {
                self.emitted.extend_from_slice(raw);
                self.line_has_content = true;
            }
        }
    }

    fn emit_line(&mut self, terminated: bool, output: &mut Vec<NormalizedEvent>) {
        self.flush_bytes(output);
        output.push(NormalizedEvent::LineEnd { terminated });
        self.line_has_content = false;
    }

    fn flush_bytes(&mut self, output: &mut Vec<NormalizedEvent>) {
        if !self.emitted.is_empty() {
            output.push(NormalizedEvent::Bytes(std::mem::take(&mut self.emitted)));
        }
    }
}

/// Incrementally removes ANSI CSI/OSC sequences and returns bounded lines for
/// foreground presentation.
#[derive(Debug)]
pub(crate) struct StreamNormalizer {
    core: NormalizerCore,
    line: LineAccumulator,
    stream_encoding: Option<StreamEncoding>,
}

impl StreamNormalizer {
    pub(crate) fn new() -> Self {
        Self {
            core: NormalizerCore::new(),
            line: LineAccumulator::default(),
            stream_encoding: None,
        }
    }

    /// Consumes an arbitrary byte chunk without decoding or corrupting split code units.
    pub(crate) fn push(&mut self, bytes: &[u8], output: &mut Vec<NormalizedLine>) {
        let mut events = Vec::new();
        self.core.push(bytes, &mut events);
        self.consume(events, output);
    }

    /// Flushes a final unterminated line and any pending isolated carriage return.
    pub(crate) fn finish(mut self, output: &mut Vec<NormalizedLine>) {
        let mut events = Vec::new();
        self.core.finish(&mut events);
        self.consume(events, output);
    }

    fn consume(&mut self, events: Vec<NormalizedEvent>, output: &mut Vec<NormalizedLine>) {
        for event in events {
            match event {
                NormalizedEvent::Start(encoding) => self.stream_encoding = encoding,
                NormalizedEvent::Bytes(bytes) => self.line.push(&bytes),
                NormalizedEvent::LineEnd { terminated } => {
                    output.push(
                        std::mem::take(&mut self.line).finish(terminated, self.stream_encoding),
                    );
                }
            }
        }
    }
}

/// Streaming normalizer for the append-only background log. Callers receive
/// bounded chunks and explicit line boundaries, never a whole-line allocation.
#[derive(Debug)]
pub(crate) struct LogNormalizer {
    core: NormalizerCore,
}

impl LogNormalizer {
    pub(crate) fn new() -> Self {
        Self {
            core: NormalizerCore::new(),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8], output: &mut Vec<NormalizedEvent>) {
        self.core.push(bytes, output);
    }

    pub(crate) fn finish(mut self, output: &mut Vec<NormalizedEvent>) {
        self.core.finish(output);
    }
}

impl Default for StreamNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

fn detect_stream_mode(bytes: &[u8], final_chunk: bool) -> Option<(StreamMode, usize)> {
    const UTF32_BE_BOM: &[u8] = &[0x00, 0x00, 0xfe, 0xff];
    const UTF32_LE_BOM: &[u8] = &[0xff, 0xfe, 0x00, 0x00];
    const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];
    const UTF16_LE_BOM: &[u8] = &[0xff, 0xfe];
    const UTF16_BE_BOM: &[u8] = &[0xfe, 0xff];

    if bytes.starts_with(UTF32_BE_BOM) {
        return Some((StreamMode::Wide(StreamEncoding::Utf32Be), 4));
    }
    if bytes.starts_with(UTF32_LE_BOM) {
        return Some((StreamMode::Wide(StreamEncoding::Utf32Le), 4));
    }
    if bytes.starts_with(UTF8_BOM) {
        return Some((StreamMode::Bytes, 3));
    }
    if bytes.starts_with(UTF16_BE_BOM) {
        return Some((StreamMode::Wide(StreamEncoding::Utf16Be), 2));
    }
    if bytes.starts_with(UTF16_LE_BOM) {
        if bytes.len() >= 3 && bytes[2] != 0 {
            return Some((StreamMode::Wide(StreamEncoding::Utf16Le), 2));
        }
        if bytes.len() >= 4 || final_chunk {
            return Some((StreamMode::Wide(StreamEncoding::Utf16Le), 2));
        }
        return None;
    }

    let possible_bom = [
        UTF32_BE_BOM,
        UTF32_LE_BOM,
        UTF8_BOM,
        UTF16_LE_BOM,
        UTF16_BE_BOM,
    ]
    .iter()
    .any(|bom| bom.starts_with(bytes));
    if possible_bom && !final_chunk {
        None
    } else {
        Some((StreamMode::Bytes, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::{NormalizedLine, StreamEncoding, StreamNormalizer};

    fn normalize(chunks: &[&[u8]]) -> Vec<NormalizedLine> {
        let mut normalizer = StreamNormalizer::new();
        let mut lines = Vec::new();
        for chunk in chunks {
            normalizer.push(chunk, &mut lines);
        }
        normalizer.finish(&mut lines);
        lines
    }

    fn line(bytes: &[u8], terminated: bool) -> NormalizedLine {
        NormalizedLine {
            bytes: bytes.to_vec(),
            total_bytes: bytes.len() as u64,
            terminated,
            stream_encoding: None,
            raw_truncated: false,
        }
    }

    #[test]
    fn strips_split_ansi_and_normalizes_crlf_and_isolated_cr() {
        assert_eq!(
            normalize(&[
                b"one\x1b[3",
                b"1m red\x1b[0m\r",
                b"\ntwo\rthree\x1b]0;title\x1b\\"
            ]),
            vec![
                line(b"one red", true),
                line(b"two", true),
                line(b"three", false),
            ]
        );
    }

    #[test]
    fn preserves_split_multibyte_bytes_without_decoding_them() {
        assert_eq!(
            normalize(&[&[0xe7, 0x95], &[0x8c, 0xff]]),
            vec![line(&[0xe7, 0x95, 0x8c, 0xff], false)]
        );
    }

    #[test]
    fn ascii_line_boundaries_do_not_split_gbk_or_shift_jis_characters() {
        assert_eq!(
            normalize(&[&[0xd6], &[0xd0, 0xce, 0xc4, b'\n', 0x93, 0xfa, 0x96, 0x7b]]),
            vec![
                line(&[0xd6, 0xd0, 0xce, 0xc4], true),
                line(&[0x93, 0xfa, 0x96, 0x7b], false),
            ]
        );
    }

    #[test]
    fn split_utf16le_bom_locks_wide_line_boundaries_and_strips_wide_ansi() {
        let raw = [
            0xff, 0xfe, b'a', 0, 0x1b, 0, b'[', 0, b'3', 0, b'1', 0, b'm', 0, b'b', 0, b'\r', 0,
            b'\n', 0, b'c', 0,
        ];
        let lines = normalize(&[&raw[..1], &raw[1..7], &raw[7..]]);
        assert_eq!(
            lines,
            vec![
                NormalizedLine {
                    bytes: vec![b'a', 0, b'b', 0],
                    total_bytes: 4,
                    terminated: true,
                    stream_encoding: Some(StreamEncoding::Utf16Le),
                    raw_truncated: false,
                },
                NormalizedLine {
                    bytes: vec![b'c', 0],
                    total_bytes: 2,
                    terminated: false,
                    stream_encoding: Some(StreamEncoding::Utf16Le),
                    raw_truncated: false,
                },
            ]
        );
    }

    #[test]
    fn overlong_lines_keep_a_bounded_prefix_and_exact_byte_count() {
        let input = vec![b'x'; 400_000];
        let lines = normalize(&[&input]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].bytes.len(), 64 * 1024);
        assert_eq!(lines[0].total_bytes, 400_000);
        assert!(lines[0].raw_truncated);
    }

    #[test]
    fn incomplete_wide_code_unit_at_eof_is_preserved_for_lossy_delivery() {
        assert_eq!(
            normalize(&[&[0xff, 0xfe, b'a']]),
            vec![NormalizedLine {
                bytes: vec![b'a'],
                total_bytes: 1,
                terminated: false,
                stream_encoding: Some(StreamEncoding::Utf16Le),
                raw_truncated: false,
            }]
        );
    }
}
