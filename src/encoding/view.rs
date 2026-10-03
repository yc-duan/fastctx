//! Bounded encoding evidence and strict incremental decoding for the view command.

use super::{
    ByteSource, DetectedEncoding, EncodingDecision, EncodingKind, EncodingOrigin,
    EncodingRejection, bom_detected_encoding, decode_bytes, explicit_detected_encoding,
    validate_source_encoding,
};
use encoding_rs::{Decoder, DecoderResult, UTF_8};
use std::fs::File;
use std::io::{BufReader, Cursor, Read};

/// A file prefix classified without validating the unread remainder of the file.
pub(crate) enum OpenedText {
    Text(Box<TextReader>),
    Binary,
}

enum StrictDecoder {
    Encoding(Decoder),
    Utf32(bool),
}

/// Strict decoding keeps bounded input/output buffers and reports zero-based source byte offsets.
pub(crate) struct TextReader {
    input: BufReader<std::io::Chain<Cursor<Vec<u8>>, File>>,
    decoder: StrictDecoder,
    label: &'static str,
    offset: u64,
    pending: String,
    position: usize,
    finished: bool,
    pub(crate) fact: Option<String>,
    pub(crate) total_lines: Option<usize>,
}

impl TextReader {
    /// The encoding the text is decoded with.
    pub(crate) fn label(&self) -> &'static str {
        self.label
    }

    pub(crate) fn open(
        mut file: File,
        explicit: Option<&str>,
        character_budget: usize,
    ) -> Result<OpenedText, String> {
        // Four bytes can encode one Unicode scalar. This evidence buffer is bounded by
        // the response budget, not by the file size; four more bytes settle its boundary.
        let probe_bytes = character_budget * 4 + 4;
        let mut prefix = Vec::new();
        file.by_ref()
            .take(probe_bytes as u64)
            .read_to_end(&mut prefix)
            .map_err(|error| error.to_string())?;
        let at_end = prefix.len() < probe_bytes
            || file.metadata().map_err(|error| error.to_string())?.len() == prefix.len() as u64;
        let detected = if let Some(label) = explicit {
            explicit_detected_encoding(label, &prefix)
                .map_err(|_| format!("Invalid encoding value \"{label}\". Use a WHATWG encoding label or utf-32le/utf-32be."))?
        } else if let Some(detected) = bom_detected_encoding(&prefix) {
            if detected.kind == EncodingKind::EncodingRs(UTF_8) && super::has_binary_nul(&prefix) {
                return Ok(OpenedText::Binary);
            }
            detected
        } else {
            if prefix
                .windows(2)
                .any(|bytes| matches!(bytes, [0x1b, b'$' | b'(']))
            {
                return Err("Cannot determine the text encoding with confidence: iso-2022-jp, utf-8; ISO-2022 escape sequence found. Use --encoding to specify the source encoding.".to_string());
            }
            match std::str::from_utf8(&prefix) {
                Ok(_) if !super::has_binary_nul(&prefix) => utf8(),
                Err(error)
                    if error.error_len().is_none()
                        && !at_end
                        && !super::has_binary_nul(&prefix) =>
                {
                    utf8()
                }
                _ => {
                    let mut decision = validate_source_encoding(ByteSource::Bytes(&prefix), None)
                        .map_err(|error| error.to_string())?;
                    // A sampling boundary can split a legacy multibyte character. Try only
                    // the possible trailing bytes; the stream still validates every byte.
                    if !at_end && matches!(decision, EncodingDecision::Rejected(_)) {
                        for tail in 1..=3.min(prefix.len()) {
                            let candidate = validate_source_encoding(
                                ByteSource::Bytes(&prefix[..prefix.len() - tail]),
                                None,
                            )
                            .map_err(|error| error.to_string())?;
                            if matches!(candidate, EncodingDecision::Text(_)) {
                                decision = candidate;
                                break;
                            }
                        }
                    }
                    match decision {
                        EncodingDecision::Text(validated) => validated.detected,
                        EncodingDecision::Binary => return Ok(OpenedText::Binary),
                        EncodingDecision::Rejected(reason) => {
                            let offset = std::str::from_utf8(&prefix)
                                .err()
                                .map(|error| error.valid_up_to())
                                .unwrap_or(0);
                            return Err(rejection(reason, offset));
                        }
                    }
                }
            }
        };
        let label = detected.source_encoding.unwrap_or("UTF-8");
        let fact = detected.source_encoding.map(|encoding| {
            if explicit.is_some() {
                format!("decoded from {encoding} as requested")
            } else {
                format!("decoded from {encoding}")
            }
        });
        let total_lines = if at_end {
            decode_bytes(&prefix, &detected).map(|text| line_count(&text))
        } else {
            None
        };
        let decoder = match detected.kind {
            EncodingKind::EncodingRs(encoding) => {
                StrictDecoder::Encoding(encoding.new_decoder_without_bom_handling())
            }
            EncodingKind::Utf32Le => StrictDecoder::Utf32(true),
            EncodingKind::Utf32Be => StrictDecoder::Utf32(false),
        };
        prefix.drain(..detected.bom_len as usize);
        Ok(OpenedText::Text(Box::new(Self {
            input: BufReader::new(Cursor::new(prefix).chain(file)),
            decoder,
            label,
            offset: detected.bom_len,
            pending: String::new(),
            position: 0,
            finished: false,
            fact,
            total_lines,
        })))
    }

    pub(crate) fn next_char(&mut self) -> Result<Option<char>, String> {
        loop {
            if self.position < self.pending.len() {
                let character = self.pending[self.position..].chars().next().unwrap();
                self.position += character.len_utf8();
                return Ok(Some(character));
            }
            if self.finished {
                return Ok(None);
            }
            self.pending.clear();
            self.position = 0;
            match &mut self.decoder {
                StrictDecoder::Encoding(decoder) => {
                    use std::io::BufRead;
                    let bytes = self.input.fill_buf().map_err(|error| error.to_string())?;
                    let last = bytes.is_empty();
                    let mut output = [0_u8; 8192];
                    let (result, read, written) =
                        decoder.decode_to_utf8_without_replacement(bytes, &mut output, last);
                    self.offset += read as u64;
                    self.input.consume(read);
                    if let DecoderResult::Malformed(length, after) = result {
                        let offset = self.offset - u64::from(length) - u64::from(after);
                        return Err(format!(
                            "Cannot decode as {} at byte {offset}. Use --encoding to specify the source encoding.",
                            self.label
                        ));
                    }
                    self.pending
                        .push_str(std::str::from_utf8(&output[..written]).unwrap());
                    self.finished = last && result == DecoderResult::InputEmpty;
                }
                StrictDecoder::Utf32(little_endian) => {
                    let mut raw = [0_u8; 4];
                    let mut read = 0;
                    while read < raw.len() {
                        let count = self
                            .input
                            .read(&mut raw[read..])
                            .map_err(|error| error.to_string())?;
                        if count == 0 {
                            break;
                        }
                        read += count;
                    }
                    if read == 0 {
                        self.finished = true;
                        continue;
                    }
                    let value = if *little_endian {
                        u32::from_le_bytes(raw)
                    } else {
                        u32::from_be_bytes(raw)
                    };
                    let character = char::from_u32(value).filter(|_| read == 4).ok_or_else(|| format!(
                        "Cannot decode as {} at byte {}. Use --encoding to specify the source encoding.", self.label, self.offset
                    ))?;
                    self.offset += 4;
                    self.pending.push(character);
                }
            }
        }
    }
}

fn utf8() -> DetectedEncoding {
    DetectedEncoding {
        kind: EncodingKind::EncodingRs(UTF_8),
        bom_len: 0,
        source_encoding: None,
        origin: EncodingOrigin::Automatic,
    }
}

pub(crate) fn line_count(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count()
        + usize::from(!text.is_empty() && !text.ends_with('\n'))
}

fn rejection(reason: EncodingRejection, offset: usize) -> String {
    let candidates = match reason {
        EncodingRejection::Ambiguous { candidates } => candidates.join(", "),
        EncodingRejection::Iso2022JpSignature => "iso-2022-jp, utf-8".to_string(),
        EncodingRejection::MixedOrInconsistent { .. } => {
            "mixed or inconsistent encodings".to_string()
        }
        _ => "no supported encoding decodes it cleanly".to_string(),
    };
    format!(
        "Cannot determine the text encoding with confidence: {candidates}; UTF-8 fails at byte {offset}. Use --encoding to specify the source encoding."
    )
}
