//! Delivery-time decoding for raw shell output.

use crate::encoding::decode_text;
use crate::shell::normalize::StreamEncoding;
use encoding_rs::{DecoderResult, Encoding, UTF_8, UTF_16BE, UTF_16LE};

const MAX_PRESENTED_LINE_CHARS: usize = 2_000;

/// One stored line borrowed from the foreground ring or a direct/legacy job record.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EncodedLine<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) total_bytes: u64,
    pub(crate) stream_encoding: Option<StreamEncoding>,
    pub(crate) legacy_text: Option<&'a str>,
    pub(crate) known_truncated: bool,
}

/// A validated ASCII-compatible WHATWG label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OutputEncoding {
    encoding: &'static Encoding,
}

impl OutputEncoding {
    pub(crate) fn label(self) -> &'static str {
        self.encoding.name()
    }
}

/// UTF-8 presentation plus notes and loss facts derived independently from the raw bytes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DecodedLines {
    pub(crate) lines: Vec<String>,
    pub(crate) invalid_sequences: u64,
    pub(crate) invalid_sequences_per_line: Vec<u64>,
    pub(crate) truncated_per_line: Vec<bool>,
    pub(crate) transcoding_note: Option<String>,
    pub(crate) had_truncation: bool,
}

#[derive(Clone, Copy, Debug)]
enum DecodeKind {
    Whatwg(&'static Encoding),
    Wide(StreamEncoding),
}

#[derive(Clone, Copy, Debug)]
struct DecodePlan {
    kind: DecodeKind,
    requested: bool,
}

impl DecodePlan {
    fn label(self) -> &'static str {
        match self.kind {
            DecodeKind::Whatwg(encoding) => encoding.name(),
            DecodeKind::Wide(encoding) => encoding.label(),
        }
    }

    fn is_utf8(self) -> bool {
        matches!(self.kind, DecodeKind::Whatwg(encoding) if encoding == UTF_8)
    }
}

#[derive(Debug)]
struct DecodedBytes {
    text: String,
    invalid_offsets: Vec<usize>,
}

/// Validates an output encoding before a command can cause side effects.
pub(crate) fn validate_output_encoding(value: &str) -> Result<OutputEncoding, String> {
    let label = value.trim_matches(|character: char| character.is_ascii_whitespace());
    if label.eq_ignore_ascii_case("utf-32")
        || label.eq_ignore_ascii_case("utf-32le")
        || label.eq_ignore_ascii_case("utf-32be")
    {
        return Err(wide_label_error(value));
    }
    let Some(encoding) = Encoding::for_label_no_replacement(label.as_bytes()) else {
        return Err(format!(
            // The examples must stay inside what this function accepts: UTF-16/UTF-32 labels are
            // rejected below, so offering them here would send the caller straight back into a
            // second rejection (2026-07-24).
            "Invalid encoding value \"{value}\". Use a WHATWG encoding label such as \"gbk\", \"shift_jis\", \"big5\", \"euc-kr\", or \"windows-1252\"."
        ));
    };
    if encoding == UTF_16LE || encoding == UTF_16BE {
        return Err(wide_label_error(value));
    }
    Ok(OutputEncoding { encoding })
}

fn wide_label_error(value: &str) -> String {
    format!(
        "Encoding \"{value}\" is not supported for command output. UTF-16/UTF-32 output is decoded automatically when the stream starts with a BOM; otherwise redirect the command to a file (command > file 2>&1) and read it with the read tool."
    )
}

/// Decodes a complete foreground capture, using trusted automatic detection when no label is given.
pub(crate) fn decode_run(
    lines: &[EncodedLine<'_>],
    explicit: Option<OutputEncoding>,
) -> DecodedLines {
    let plan = explicit.map_or_else(
        || automatic_run_plan(lines),
        |encoding| DecodePlan {
            kind: DecodeKind::Whatwg(encoding.encoding),
            requested: true,
        },
    );
    decode_lines(lines, plan)
}

/// Decodes a job page without automatic detection.
pub(crate) fn decode_job(
    lines: &[EncodedLine<'_>],
    call_encoding: Option<OutputEncoding>,
    job_encoding: Option<OutputEncoding>,
) -> DecodedLines {
    let plan = call_encoding
        .or(job_encoding)
        .map(|encoding| DecodePlan {
            kind: DecodeKind::Whatwg(encoding.encoding),
            requested: true,
        })
        .or_else(|| bom_plan(lines))
        .unwrap_or(DecodePlan {
            kind: DecodeKind::Whatwg(UTF_8),
            requested: false,
        });
    decode_lines(lines, plan)
}

fn automatic_run_plan(lines: &[EncodedLine<'_>]) -> DecodePlan {
    if let Some(plan) = bom_plan(lines) {
        return plan;
    }
    let mut evidence = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.legacy_text.is_some() {
            continue;
        }
        if index > 0 {
            evidence.push(b'\n');
        }
        evidence.extend_from_slice(line.bytes);
    }
    match decode_text(&evidence).and_then(|decoded| decoded.source_encoding) {
        Some(label) => {
            let encoding = Encoding::for_label_no_replacement(label.as_bytes())
                .expect("trusted detector returned a WHATWG encoding");
            DecodePlan {
                kind: DecodeKind::Whatwg(encoding),
                requested: false,
            }
        }
        None => DecodePlan {
            kind: DecodeKind::Whatwg(UTF_8),
            requested: false,
        },
    }
}

fn bom_plan(lines: &[EncodedLine<'_>]) -> Option<DecodePlan> {
    lines
        .iter()
        .find_map(|line| line.stream_encoding)
        .map(|encoding| DecodePlan {
            kind: DecodeKind::Wide(encoding),
            requested: false,
        })
}

fn decode_lines(lines: &[EncodedLine<'_>], plan: DecodePlan) -> DecodedLines {
    let mut decoded = DecodedLines {
        transcoding_note: (!plan.is_utf8()).then(|| {
            if plan.requested {
                format!(
                    "(Note: decoded from {} as requested; output is UTF-8.)",
                    plan.label()
                )
            } else {
                format!("(Note: decoded from {}; output is UTF-8.)", plan.label())
            }
        }),
        ..DecodedLines::default()
    };

    for line in lines {
        if let Some(text) = line.legacy_text {
            decoded.lines.push(text.to_string());
            decoded.invalid_sequences_per_line.push(0);
            decoded.truncated_per_line.push(line.known_truncated);
            decoded.had_truncation |= line.known_truncated;
            continue;
        }
        let source = decode_bytes(line.bytes, plan.kind);
        let (text, shown_end, truncated) =
            present_line(&source.text, line.total_bytes, line.known_truncated);
        let invalid_sequences = source
            .invalid_offsets
            .iter()
            .filter(|offset| **offset < shown_end)
            .count() as u64;
        decoded.invalid_sequences = decoded.invalid_sequences.saturating_add(invalid_sequences);
        decoded.invalid_sequences_per_line.push(invalid_sequences);
        decoded.truncated_per_line.push(truncated);
        decoded.had_truncation |= truncated;
        decoded.lines.push(text);
    }
    decoded
}

fn present_line(text: &str, total_bytes: u64, known_truncated: bool) -> (String, usize, bool) {
    let mut characters = text.char_indices();
    let mut shown_end = text.len();
    for _ in 0..MAX_PRESENTED_LINE_CHARS {
        if characters.next().is_none() {
            break;
        }
    }
    let has_more_characters = if let Some((offset, _)) = characters.next() {
        shown_end = offset;
        true
    } else {
        false
    };
    let truncated = known_truncated || has_more_characters;
    let mut shown = text[..shown_end].to_string();
    if truncated {
        shown.push_str(&format!("... [line truncated: {total_bytes} bytes total]"));
    }
    (shown, shown_end, truncated)
}

fn decode_bytes(bytes: &[u8], kind: DecodeKind) -> DecodedBytes {
    match kind {
        DecodeKind::Whatwg(encoding) => decode_whatwg(bytes, encoding),
        DecodeKind::Wide(StreamEncoding::Utf16Le) => decode_whatwg(bytes, UTF_16LE),
        DecodeKind::Wide(StreamEncoding::Utf16Be) => decode_whatwg(bytes, UTF_16BE),
        DecodeKind::Wide(StreamEncoding::Utf32Le) => decode_utf32(bytes, true),
        DecodeKind::Wide(StreamEncoding::Utf32Be) => decode_utf32(bytes, false),
    }
}

fn decode_whatwg(bytes: &[u8], encoding: &'static Encoding) -> DecodedBytes {
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let capacity = bytes.len().saturating_mul(4).saturating_add(16);
    let mut text = String::with_capacity(capacity);
    let mut invalid_offsets = Vec::new();
    let mut offset = 0;
    loop {
        if text.capacity().saturating_sub(text.len()) < 16 {
            text.reserve(bytes.len().saturating_sub(offset).saturating_mul(4).max(64));
        }
        let (result, read) =
            decoder.decode_to_string_without_replacement(&bytes[offset..], &mut text, true);
        offset = offset.saturating_add(read);
        match result {
            DecoderResult::InputEmpty => break,
            DecoderResult::OutputFull => {
                text.reserve(bytes.len().saturating_sub(offset).saturating_mul(4).max(64));
            }
            DecoderResult::Malformed(_, _) => {
                invalid_offsets.push(text.len());
                text.push(char::REPLACEMENT_CHARACTER);
                if read == 0 && offset < bytes.len() {
                    offset += 1;
                    decoder = encoding.new_decoder_without_bom_handling();
                }
            }
        }
    }
    DecodedBytes {
        text,
        invalid_offsets,
    }
}

fn decode_utf32(bytes: &[u8], little_endian: bool) -> DecodedBytes {
    let mut text = String::with_capacity(bytes.len());
    let mut invalid_offsets = Vec::new();
    let mut chunks = bytes.chunks_exact(4);
    for chunk in &mut chunks {
        let value = if little_endian {
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        } else {
            u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        };
        if let Some(character) = char::from_u32(value) {
            text.push(character);
        } else {
            invalid_offsets.push(text.len());
            text.push(char::REPLACEMENT_CHARACTER);
        }
    }
    if !chunks.remainder().is_empty() {
        invalid_offsets.push(text.len());
        text.push(char::REPLACEMENT_CHARACTER);
    }
    DecodedBytes {
        text,
        invalid_offsets,
    }
}

pub(crate) fn run_garble_note(invalid_sequences: u64) -> Option<String> {
    if invalid_sequences == 0 {
        return None;
    }
    let noun = if invalid_sequences == 1 {
        "sequence"
    } else {
        "sequences"
    };
    Some(match legacy_code_page_label() {
        Some(label) => format!(
            "(Note: {invalid_sequences} invalid byte {noun} shown as U+FFFD — the command likely wrote {label}, this system's legacy code page. Re-run with encoding=\"{label}\", or redirect to a file and use the read tool.)"
        ),
        None => format!(
            "(Note: {invalid_sequences} invalid byte {noun} shown as U+FFFD. If the text looks garbled, pass the source encoding via the encoding parameter.)"
        ),
    })
}

pub(crate) fn job_garble_note(invalid_sequences: u64, anchor: u64) -> Option<String> {
    if invalid_sequences == 0 {
        return None;
    }
    let noun = if invalid_sequences == 1 {
        "sequence"
    } else {
        "sequences"
    };
    Some(match legacy_code_page_label() {
        Some(label) => format!(
            "(Note: {invalid_sequences} invalid byte {noun} shown as U+FFFD — the job likely wrote {label}, this system's legacy code page. Call job_output again with after_seq={anchor} and encoding=\"{label}\" to re-read this page.)"
        ),
        None => format!(
            "(Note: {invalid_sequences} invalid byte {noun} shown as U+FFFD. If the text looks garbled, call job_output again with after_seq={anchor} and the source encoding via encoding.)"
        ),
    })
}

#[cfg(windows)]
fn legacy_code_page_label() -> Option<&'static str> {
    use windows_sys::Win32::Globalization::GetACP;

    // SAFETY: GetACP has no preconditions and returns process-global system state.
    match unsafe { GetACP() } {
        874 => Some("windows-874"),
        932 => Some("shift_jis"),
        936 => Some("gbk"),
        949 => Some("euc-kr"),
        950 => Some("big5"),
        1_250 => Some("windows-1250"),
        1_251 => Some("windows-1251"),
        1_252 => Some("windows-1252"),
        1_253 => Some("windows-1253"),
        1_254 => Some("windows-1254"),
        1_255 => Some("windows-1255"),
        1_256 => Some("windows-1256"),
        1_257 => Some("windows-1257"),
        1_258 => Some("windows-1258"),
        54_936 => Some("gb18030"),
        _ => None,
    }
}

#[cfg(not(windows))]
fn legacy_code_page_label() -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use super::{EncodedLine, OutputEncoding, decode_job, decode_run, validate_output_encoding};
    use crate::shell::normalize::StreamEncoding;

    fn raw(bytes: &[u8]) -> EncodedLine<'_> {
        EncodedLine {
            bytes,
            total_bytes: bytes.len() as u64,
            stream_encoding: None,
            legacy_text: None,
            known_truncated: false,
        }
    }

    #[test]
    fn explicit_gbk_decoding_and_byte_counted_truncation_are_exact() {
        let encoding = validate_output_encoding("gbk").unwrap();
        let decoded = decode_job(&[raw(&[0xd6, 0xd0, 0xce, 0xc4])], Some(encoding), None);
        assert_eq!(decoded.lines, ["中文"]);
        assert_eq!(
            decoded.transcoding_note.as_deref(),
            Some("(Note: decoded from GBK as requested; output is UTF-8.)")
        );

        let input = vec![b'x'; 2_001];
        let decoded = decode_run(
            &[raw(&input)],
            Some(validate_output_encoding("utf-8").unwrap()),
        );
        assert_eq!(
            decoded.lines[0],
            format!(
                "{}... [line truncated: 2001 bytes total]",
                "x".repeat(2_000)
            )
        );
        assert!(decoded.had_truncation);
    }

    #[test]
    fn utf16_bom_locked_lines_decode_without_accepting_wide_parameters() {
        let wide = EncodedLine {
            bytes: &[b'a', 0, b'b', 0],
            total_bytes: 4,
            stream_encoding: Some(StreamEncoding::Utf16Le),
            legacy_text: None,
            known_truncated: false,
        };
        let decoded = decode_run(&[wide], None);
        assert_eq!(decoded.lines, ["ab"]);
        assert_eq!(
            decoded.transcoding_note.as_deref(),
            Some("(Note: decoded from UTF-16LE; output is UTF-8.)")
        );
        assert_eq!(
            validate_output_encoding("utf-16le").unwrap_err(),
            "Encoding \"utf-16le\" is not supported for command output. UTF-16/UTF-32 output is decoded automatically when the stream starts with a BOM; otherwise redirect the command to a file (command > file 2>&1) and read it with the read tool."
        );
    }

    #[test]
    fn invalid_sequences_are_counted_independently_from_literal_replacement_characters() {
        let decoded = decode_job(
            &[raw(&[0xef, 0xbf, 0xbd, 0xff, 0xfe])],
            Some(OutputEncoding {
                encoding: encoding_rs::UTF_8,
            }),
            None,
        );
        assert_eq!(decoded.lines, ["���"]);
        assert_eq!(decoded.invalid_sequences, 2);
    }
}
