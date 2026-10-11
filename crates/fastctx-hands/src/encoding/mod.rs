//! Strict streaming codecs shared by file readers and writers.
//!
//! The file module owns whole-file evidence and per-segment selection. The
//! lower-level detect helpers validate a codec or a supplied prefix only;
//! successful prefix decoding makes no claim about unseen bytes.

mod encode;
mod evidence;
pub(crate) mod file;
pub(crate) mod iso2022;
pub(crate) mod line;
mod mapping;
mod statistics;
mod stream;
pub use stream::TextDecoder;

/// Whether an encoding was chosen by byte statistics alone, the one basis a
/// reader may need to doubt; a BOM, an explicit choice or a strict pattern
/// leaves nothing to say.
pub(crate) fn guessed(basis: &str) -> bool {
    basis == statistics::BASIS
}

use encoding_rs::{BIG5, EUC_KR, Encoding, GBK, SHIFT_JIS, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Codec {
    Rs(&'static Encoding),
    Utf32Le,
    Utf32Be,
}

/// A codec plus the exact BOM length to skip once, at the beginning of the input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextEncoding {
    codec: Codec,
    bom_len: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum EncodingError {
    #[error("Unknown encoding label: {0}")]
    UnknownLabel(String),
    #[error("Invalid {encoding} at byte offset {offset}; decode stopped without replacing bytes")]
    Malformed { encoding: String, offset: usize },
    #[error("Encoding is uncertain at byte offset {offset}; candidates: {}. Use fastctx view --encoding <label> -- <absolute-path>", candidates.join(", "))]
    Ambiguous { candidates: Vec<String>, offset: usize },
    #[error(
        "No supported text encoding explains the bytes at offset {offset}. Inspect with xxd, or use fastctx view --encoding <label> -- <absolute-path>"
    )]
    Undecodable { offset: usize },
    #[error("Text contains {character:?}, which cannot be represented exactly in {encoding}")]
    Unrepresentable { encoding: String, character: char },
    #[error("{0} cannot reproduce the original bytes exactly; convert the file explicitly before editing")]
    NotRoundtrippable(String),
    #[error("A prefix shorter than four bytes cannot settle the BOM; supply more bytes or mark end of input")]
    IncompletePrefix,
}

impl TextEncoding {
    pub fn utf8() -> Self {
        Self { codec: Codec::Rs(UTF_8), bom_len: 0 }
    }

    pub fn label(self) -> &'static str {
        match self.codec {
            Codec::Rs(enc) => enc.name(),
            Codec::Utf32Le => "UTF-32LE",
            Codec::Utf32Be => "UTF-32BE",
        }
    }

    pub fn bom_len(self) -> usize {
        self.bom_len
    }

    /// Whether ASCII text reads the same in this encoding as in every other
    /// such encoding, so that a line of it says nothing about which one a
    /// file uses.
    pub fn ascii_compatible(self) -> bool {
        match self.codec {
            Codec::Rs(enc) => enc.is_ascii_compatible(),
            Codec::Utf32Le | Codec::Utf32Be => false,
        }
    }

    /// Decodes the complete supplied bytes, including validation of the final character.
    pub fn decode(self, bytes: &[u8]) -> Result<String, EncodingError> {
        TextDecoder::new(self).feed(bytes, true)
    }

    /// Encodes text without a BOM, rejecting substitutions and non-roundtripping mappings.
    pub fn encode(self, text: &str) -> Result<Vec<u8>, EncodingError> {
        let bytes = match self.codec {
            Codec::Utf32Le => text.chars().flat_map(|c| (c as u32).to_le_bytes()).collect(),
            Codec::Utf32Be => text.chars().flat_map(|c| (c as u32).to_be_bytes()).collect(),
            Codec::Rs(enc) if enc == UTF_8 => text.as_bytes().to_vec(),
            Codec::Rs(enc) if enc == UTF_16LE => text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            Codec::Rs(enc) if enc == UTF_16BE => text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
            Codec::Rs(enc) if enc == encoding_rs::ISO_2022_JP => iso2022::encode(text)?,
            Codec::Rs(enc) => encode::legacy(enc, text)?,
        };
        let decoded = Self { bom_len: 0, ..self }.decode(&bytes)?;
        if decoded != text {
            let character = text
                .chars()
                .zip(decoded.chars())
                .find(|(a, b)| a != b)
                .map(|(a, _)| a)
                .or_else(|| text.chars().next())
                .unwrap_or('\0');
            return Err(EncodingError::Unrepresentable { encoding: self.label().into(), character });
        }
        Ok(bytes)
    }

    /// A writer must prove this before re-encoding unchanged text, since some legacy
    /// encodings have multiple byte spellings of the same Unicode character.
    pub fn decode_for_write(self, bytes: &[u8]) -> Result<String, EncodingError> {
        let text = self.decode(bytes)?;
        if self.encode(&text)? != bytes[self.bom_len..] {
            return Err(EncodingError::NotRoundtrippable(self.label().into()));
        }
        Ok(text)
    }
}

pub fn canonical_label(label: &str) -> Result<&'static str, EncodingError> {
    Ok(explicit(label, &[])?.label())
}

/// Detects and validates an entire byte slice. No replacement decoding is used.
pub fn detect(bytes: &[u8], label: Option<&str>) -> Result<TextEncoding, EncodingError> {
    detect_prefix(bytes, true, label)
}

/// Selects an interpretation for a streaming reader. Pass at least four initial
/// bytes unless this is the whole input. Continue validation with `TextDecoder`;
/// success here makes no claim about bytes outside this prefix.
pub fn detect_prefix(bytes: &[u8], end: bool, label: Option<&str>) -> Result<TextEncoding, EncodingError> {
    if bytes.len() < 4 && !end {
        return Err(EncodingError::IncompletePrefix);
    }
    if let Some(label) = label {
        let enc = explicit(label, bytes)?;
        TextDecoder::new(enc).feed(bytes, end)?;
        return Ok(enc);
    }
    if let Some(enc) = bom(bytes) {
        TextDecoder::new(enc).feed(bytes, end)?;
        return Ok(enc);
    }
    let utf8 = TextEncoding::utf8();
    let invalid_offset = match TextDecoder::new(utf8).feed(bytes, end) {
        Ok(_) => {
            // ISO-2022-JP consists of ASCII bytes too; its escape introducer is
            // evidence of a second interpretation, not permission to pick one.
            if let Some(offset) = bytes.windows(2).position(|w| w == b"\x1b$" || w == b"\x1b(") {
                return Err(EncodingError::Ambiguous {
                    candidates: vec!["UTF-8".into(), "ISO-2022-JP".into()],
                    offset,
                });
            }
            return Ok(utf8);
        }
        Err(EncodingError::Malformed { offset, .. }) => offset,
        Err(error) => return Err(error),
    };
    let mut candidates = Vec::new();
    for codec in [GBK, SHIFT_JIS, BIG5, EUC_KR, WINDOWS_1252] {
        let enc = TextEncoding { codec: Codec::Rs(codec), bom_len: 0 };
        if let Ok(text) = TextDecoder::new(enc).feed(bytes, end)
            && text.chars().all(legacy_text_character)
        {
            candidates.push(enc);
        }
    }
    match candidates.as_slice() {
        [] => Err(EncodingError::Undecodable { offset: invalid_offset }),
        _ => Err(EncodingError::Ambiguous {
            candidates: candidates.iter().map(|enc| enc.label().into()).collect(),
            offset: invalid_offset,
        }),
    }
}

fn legacy_text_character(c: char) -> bool {
    (!c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        && !(0xfdd0..=0xfdef).contains(&(c as u32))
        && (c as u32 & 0xffff) < 0xfffe
}

pub(crate) fn explicit(label: &str, bytes: &[u8]) -> Result<TextEncoding, EncodingError> {
    let label = label.trim();
    let codec = if label.eq_ignore_ascii_case("utf-32le") {
        Codec::Utf32Le
    } else if label.eq_ignore_ascii_case("utf-32be") {
        Codec::Utf32Be
    } else {
        let encoding = Encoding::for_label_no_replacement(label.as_bytes())
            .ok_or_else(|| EncodingError::UnknownLabel(label.into()))?;
        Codec::Rs(if encoding == GBK { encoding_rs::GB18030 } else { encoding })
    };
    // UTF-16LE followed by NUL shares the UTF-32LE BOM prefix. An explicit
    // codec must check its own signature before automatic BOM precedence.
    let bom_len = BOMS
        .iter()
        .find(|(prefix, candidate)| *candidate == codec && bytes.starts_with(prefix))
        .map_or(0, |(prefix, _)| prefix.len());
    Ok(TextEncoding { codec, bom_len })
}

const BOMS: &[(&[u8], Codec)] = &[
    (b"\x00\x00\xfe\xff", Codec::Utf32Be),
    (b"\xff\xfe\x00\x00", Codec::Utf32Le),
    (b"\xef\xbb\xbf", Codec::Rs(UTF_8)),
    (b"\xff\xfe", Codec::Rs(UTF_16LE)),
    (b"\xfe\xff", Codec::Rs(UTF_16BE)),
];

fn bom(bytes: &[u8]) -> Option<TextEncoding> {
    for &(prefix, codec) in BOMS {
        if bytes.starts_with(prefix) {
            return Some(TextEncoding { codec, bom_len: prefix.len() });
        }
    }
    None
}
