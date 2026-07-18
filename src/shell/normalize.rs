//! Streaming terminal-output normalization with deterministic, bounded line storage.

const MAX_LINE_CHARS: usize = 2_000;
const UTF8_FLUSH_BYTES: usize = 8 * 1024;

/// One normalized output line and whether the source ended it with a line break.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NormalizedLine {
    /// Display text after ANSI stripping, lossy UTF-8 decoding, and line truncation.
    pub(crate) text: String,
    /// True when CR, LF, or CRLF terminated this line.
    pub(crate) terminated: bool,
    /// True when characters beyond the 2,000-character presentation limit were discarded.
    pub(crate) truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EscapeState {
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

#[derive(Debug, Default)]
struct LineAccumulator {
    pending_utf8: Vec<u8>,
    prefix: String,
    total_chars: usize,
}

impl LineAccumulator {
    fn push_byte(&mut self, byte: u8) {
        self.pending_utf8.push(byte);
        if self.pending_utf8.len() >= UTF8_FLUSH_BYTES {
            self.decode_pending(false);
        }
    }

    fn has_content(&self) -> bool {
        self.total_chars > 0 || !self.pending_utf8.is_empty()
    }

    fn finish(mut self, terminated: bool) -> NormalizedLine {
        self.decode_pending(true);
        let truncated = self.total_chars > MAX_LINE_CHARS;
        if truncated {
            self.prefix.push_str(&format!(
                "... [line truncated: {} chars total]",
                self.total_chars
            ));
        }
        NormalizedLine {
            text: self.prefix,
            terminated,
            truncated,
        }
    }

    fn decode_pending(&mut self, final_chunk: bool) {
        let mut consumed = 0;
        while consumed < self.pending_utf8.len() {
            match std::str::from_utf8(&self.pending_utf8[consumed..]) {
                Ok(valid) => {
                    let valid = valid.to_owned();
                    self.push_valid(&valid);
                    consumed = self.pending_utf8.len();
                }
                Err(error) => {
                    let valid_end = consumed + error.valid_up_to();
                    if valid_end > consumed {
                        let valid = unsafe {
                            // from_utf8 reported this exact prefix as valid.
                            std::str::from_utf8_unchecked(&self.pending_utf8[consumed..valid_end])
                        }
                        .to_owned();
                        self.push_valid(&valid);
                    }
                    consumed = valid_end;
                    match error.error_len() {
                        Some(length) => {
                            self.push_char(char::REPLACEMENT_CHARACTER);
                            consumed = consumed.saturating_add(length);
                        }
                        None if final_chunk => {
                            self.push_char(char::REPLACEMENT_CHARACTER);
                            consumed = self.pending_utf8.len();
                        }
                        None => break,
                    }
                }
            }
        }
        if consumed > 0 {
            self.pending_utf8.drain(..consumed);
        }
    }

    fn push_valid(&mut self, valid: &str) {
        for character in valid.chars() {
            self.push_char(character);
        }
    }

    fn push_char(&mut self, character: char) {
        self.total_chars = self.total_chars.saturating_add(1);
        if self.total_chars <= MAX_LINE_CHARS {
            self.prefix.push(character);
        }
    }
}

/// Incrementally removes ANSI CSI/OSC sequences and normalizes all CR forms to LF lines.
#[derive(Debug)]
pub(crate) struct StreamNormalizer {
    escape: EscapeState,
    pending_cr: bool,
    line: LineAccumulator,
}

impl StreamNormalizer {
    pub(crate) fn new() -> Self {
        Self {
            escape: EscapeState::Text,
            pending_cr: false,
            line: LineAccumulator::default(),
        }
    }

    /// Consumes an arbitrary byte chunk without corrupting split UTF-8 or escape sequences.
    pub(crate) fn push(&mut self, bytes: &[u8], output: &mut Vec<NormalizedLine>) {
        for &byte in bytes {
            match self.escape {
                EscapeState::Text => {
                    if byte == 0x1b {
                        self.escape = EscapeState::Escape;
                    } else {
                        self.push_text_byte(byte, output);
                    }
                }
                EscapeState::Escape => {
                    self.escape = match byte {
                        b'[' => EscapeState::Csi,
                        b']' => EscapeState::Osc,
                        _ => EscapeState::Text,
                    };
                }
                EscapeState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        self.escape = EscapeState::Text;
                    }
                }
                EscapeState::Osc => match byte {
                    0x07 => self.escape = EscapeState::Text,
                    0x1b => self.escape = EscapeState::OscEscape,
                    _ => {}
                },
                EscapeState::OscEscape => {
                    self.escape = if byte == b'\\' {
                        EscapeState::Text
                    } else if byte == 0x1b {
                        EscapeState::OscEscape
                    } else {
                        EscapeState::Osc
                    };
                }
            }
        }
    }

    /// Flushes a final unterminated line and any pending isolated carriage return.
    pub(crate) fn finish(mut self, output: &mut Vec<NormalizedLine>) {
        if self.pending_cr {
            self.emit_line(true, output);
            self.pending_cr = false;
        } else if self.line.has_content() {
            self.emit_line(false, output);
        }
    }

    fn push_text_byte(&mut self, byte: u8, output: &mut Vec<NormalizedLine>) {
        if self.pending_cr {
            self.emit_line(true, output);
            self.pending_cr = false;
            if byte == b'\n' {
                return;
            }
        }
        match byte {
            b'\r' => self.pending_cr = true,
            b'\n' => self.emit_line(true, output),
            _ => self.line.push_byte(byte),
        }
    }

    fn emit_line(&mut self, terminated: bool, output: &mut Vec<NormalizedLine>) {
        let line = std::mem::take(&mut self.line).finish(terminated);
        output.push(line);
    }
}

impl Default for StreamNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{NormalizedLine, StreamNormalizer};

    fn normalize(chunks: &[&[u8]]) -> Vec<NormalizedLine> {
        let mut normalizer = StreamNormalizer::new();
        let mut lines = Vec::new();
        for chunk in chunks {
            normalizer.push(chunk, &mut lines);
        }
        normalizer.finish(&mut lines);
        lines
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
                NormalizedLine {
                    text: "one red".to_string(),
                    terminated: true,
                    truncated: false,
                },
                NormalizedLine {
                    text: "two".to_string(),
                    terminated: true,
                    truncated: false,
                },
                NormalizedLine {
                    text: "three".to_string(),
                    terminated: false,
                    truncated: false,
                },
            ]
        );
    }

    #[test]
    fn keeps_split_multibyte_utf8_and_replaces_only_invalid_bytes() {
        assert_eq!(
            normalize(&[&[0xe7, 0x95], &[0x8c, 0xff]]),
            vec![NormalizedLine {
                text: "界�".to_string(),
                terminated: false,
                truncated: false,
            }]
        );
    }

    #[test]
    fn truncates_overlong_lines_without_retaining_the_full_raw_line() {
        let input = "界".repeat(400_000);
        let lines = normalize(&[input.as_bytes()]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].text.starts_with(&"界".repeat(2_000)));
        assert!(
            lines[0]
                .text
                .ends_with("... [line truncated: 400000 chars total]")
        );
        assert!(lines[0].truncated);
        assert!(lines[0].text.len() < 10_000);
    }

    #[test]
    fn incomplete_utf8_at_eof_matches_lossy_decoding() {
        assert_eq!(
            normalize(&[&[0xf0, 0x9f]]),
            vec![NormalizedLine {
                text: "�".to_string(),
                terminated: false,
                truncated: false,
            }]
        );
    }
}
