//! Binary detection, trusted encoding classification, and UTF-8 transcoding.

use crate::file_snapshot::{SealedSnapshot, SnapshotReader};
use encoding_rs::{
    BIG5, DecoderResult, EUC_KR, EncoderResult, Encoding, GBK, SHIFT_JIS, UTF_8, UTF_16BE,
    UTF_16LE, WINDOWS_1252,
};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

#[cfg(test)]
mod reference_v011;
mod snapshot_pipeline;

pub(crate) use snapshot_pipeline::{EncodingPipelineFailure, validate_snapshot_encoding};

const BINARY_PROBE_BYTES: usize = 8 * 1024;
const DECODE_CHUNK_BYTES: usize = 64 * 1024;
const LEGACY_EVIDENCE_BYTES: usize = 32;
// Segments buffer at most 4 KiB; a valid UTF-8 run of 32 bytes with 8 non-ASCII bytes catches one-line mixtures while limiting accidental matches in pure GBK (2026-07-13).
const LEGACY_SEGMENT_MAX_BYTES: usize = 4 * 1024;
const UTF8_SEGMENT_MIN_BYTES: usize = 32;
const UTF8_SEGMENT_MIN_NON_ASCII_BYTES: usize = 8;

const FIXED_LEGACY_ENCODINGS: [(&str, &Encoding); 5] = [
    ("windows-1252", WINDOWS_1252),
    ("gbk", GBK),
    ("shift_jis", SHIFT_JIS),
    ("big5", BIG5),
    ("euc-kr", EUC_KR),
];

/// One validation input: an on-disk file or an immutable byte snapshot.
///
/// Both variants drive the exact same chunked validation machinery, so the
/// trust hierarchy has a single implementation. Grep uses `Snapshot` so every
/// pass and the regex search observe the same single-open capture.
#[derive(Clone, Copy)]
pub(crate) enum ByteSource<'a> {
    File(&'a Path),
    Bytes(&'a [u8]),
    Snapshot(&'a SealedSnapshot),
}

enum SourceReader<'a> {
    File(BufReader<File>),
    Bytes(io::Cursor<&'a [u8]>),
    Snapshot(SnapshotReader<'a>),
}

impl<'a> SourceReader<'a> {
    fn open(source: ByteSource<'a>, start: u64) -> io::Result<Self> {
        match source {
            ByteSource::File(path) => Ok(Self::open_file(path, start)?),
            ByteSource::Bytes(bytes) => {
                let start = usize::try_from(start)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                Ok(SourceReader::Bytes(io::Cursor::new(&bytes[start..])))
            }
            ByteSource::Snapshot(snapshot) => {
                snapshot.open_reader(start).map(SourceReader::Snapshot)
            }
        }
    }

    fn open_file(path: &Path, start: u64) -> io::Result<SourceReader<'static>> {
        let mut reader = BufReader::new(File::open(path)?);
        if start > 0 {
            reader.seek(SeekFrom::Start(start))?;
        }
        Ok(SourceReader::File(reader))
    }
}

impl Read for SourceReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        match self {
            SourceReader::File(reader) => reader.read(output),
            SourceReader::Bytes(cursor) => cursor.read(output),
            SourceReader::Snapshot(reader) => reader.read(output),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EncodingKind {
    EncodingRs(&'static Encoding),
    Utf32Le,
    Utf32Be,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum EncodingOrigin {
    Explicit(String),
    Bom(&'static str),
    Automatic,
}

/// Encoding, BOM length, and public source label required for incremental decoding.
#[derive(Clone, Debug)]
pub(crate) struct DetectedEncoding {
    kind: EncodingKind,
    bom_len: u64,
    /// Canonical encoding name for non-UTF-8 input; UTF-8 uses None.
    pub(crate) source_encoding: Option<&'static str>,
    origin: EncodingOrigin,
}

/// Strictly validated file encoding and line-ending metadata shared by read and grep.
#[derive(Clone, Debug)]
pub(crate) struct ValidatedFileEncoding {
    pub(crate) detected: DetectedEncoding,
    pub(crate) total_lines: usize,
    pub(crate) has_trailing_newline: bool,
    explicit_utf8_warning: bool,
}

/// Fully decoded bytes plus an exact decoded-character-boundary to raw-byte map.
#[derive(Clone, Debug)]
pub(crate) struct EditableDecodedText {
    /// Strictly decoded text, including original CRLF characters.
    pub(crate) text: String,
    /// For every UTF-8 byte boundary in `text`, the corresponding raw-file byte offset.
    pub(crate) raw_boundaries: Vec<Option<usize>>,
}

impl ValidatedFileEncoding {
    /// Returns the raw snapshot offset where decoded UTF-8 begins when no transcoding is needed.
    pub(crate) fn utf8_snapshot_start(&self) -> Option<u64> {
        (self.detected.kind == EncodingKind::EncodingRs(UTF_8)).then_some(self.detected.bom_len)
    }

    /// Opens a streaming UTF-8 reader over the exact source that was validated.
    pub(crate) fn open_source_reader<'a>(
        &self,
        source: ByteSource<'a>,
    ) -> io::Result<Utf8Reader<'a>> {
        Utf8Reader::open_source(source, self.detected.clone())
    }

    /// Decodes an already-validated snapshot for in-memory search.
    ///
    /// UTF-8 content is borrowed without copying; every other source encoding
    /// is decoded to owned UTF-8 bytes. Returns `None` only if the bytes no
    /// longer decode under the validated encoding.
    pub(crate) fn decode_for_search<'a>(
        &self,
        raw: &'a [u8],
    ) -> Option<std::borrow::Cow<'a, [u8]>> {
        let content = raw.get(self.detected.bom_len as usize..)?;
        if self.detected.kind == EncodingKind::EncodingRs(UTF_8) {
            return Some(std::borrow::Cow::Borrowed(content));
        }
        decode_bytes(raw, &self.detected).map(|text| std::borrow::Cow::Owned(text.into_bytes()))
    }

    /// Streams UTF-8 text on character boundaries and stops immediately when the callback returns false.
    pub(crate) fn stream_text(
        &self,
        path: &Path,
        mut on_text: impl FnMut(&str) -> bool,
    ) -> Result<bool, StreamDecodeFailure> {
        let mut decoder = DecodedChunkReader::open(ByteSource::File(path), self.detected.clone())?;
        loop {
            match decoder.next_chunk()? {
                Some(chunk) if !on_text(&chunk) => return Ok(false),
                Some(_) => {}
                None => return Ok(true),
            }
        }
    }

    /// Preserves an accurate failure channel from the original trust source when the file changes after validation.
    pub(crate) fn malformed_rejection(&self) -> EncodingRejection {
        match &self.detected.origin {
            EncodingOrigin::Explicit(encoding) => EncodingRejection::ExplicitMalformed {
                encoding: encoding.clone(),
            },
            EncodingOrigin::Bom(encoding) => EncodingRejection::BomMismatch { encoding },
            EncodingOrigin::Automatic => EncodingRejection::Undecodable,
        }
    }

    /// Returns the transcoding audit note published after the body; original UTF-8 produces no note.
    pub(crate) fn transcoding_note(&self) -> Option<String> {
        let encoding = self.detected.source_encoding?;
        match &self.detected.origin {
            EncodingOrigin::Explicit(_) if self.explicit_utf8_warning => Some(format!(
                "(Note: decoded from {encoding} as requested; output is UTF-8. Warning: the raw bytes are also valid UTF-8 — if this looks garbled, retry with encoding=\"utf-8\" or omit encoding.)"
            )),
            EncodingOrigin::Explicit(_) => Some(format!(
                "(Note: decoded from {encoding} as requested; output is UTF-8.)"
            )),
            EncodingOrigin::Bom(_) | EncodingOrigin::Automatic => {
                Some(format!("(Note: decoded from {encoding}; output is UTF-8.)"))
            }
        }
    }

    /// Decodes an already-read snapshot and proves an exact raw boundary for every character.
    pub(crate) fn decode_editable_snapshot(
        &self,
        raw: &[u8],
    ) -> Result<EditableDecodedText, String> {
        let text = decode_bytes(raw, &self.detected)
            .ok_or_else(|| "the file changed after encoding validation".to_string())?;
        let bom_len = self.detected.bom_len as usize;
        let mut boundaries = vec![None; text.len().saturating_add(1)];
        boundaries[0] = Some(bom_len);
        let mut encoded = Vec::with_capacity(raw.len().saturating_sub(bom_len));
        for (start, character) in text.char_indices() {
            let end = start + character.len_utf8();
            let fragment = self.encode_fragment(&character.to_string()).ok_or_else(|| {
                format!(
                    "stateful source encoding {} cannot provide byte-preserving edit boundaries",
                    self.encoding_label()
                )
            })?;
            encoded.extend_from_slice(&fragment);
            boundaries[end] = Some(bom_len.saturating_add(encoded.len()));
        }
        if raw.get(bom_len..) != Some(encoded.as_slice()) {
            return Err(format!(
                "source encoding {} did not reproduce the original bytes exactly",
                self.encoding_label()
            ));
        }
        Ok(EditableDecodedText {
            text,
            raw_boundaries: boundaries,
        })
    }

    /// Encodes newly inserted text without adding a BOM; `None` means unmappable or stateful.
    pub(crate) fn encode_fragment(&self, text: &str) -> Option<Vec<u8>> {
        match self.detected.kind {
            EncodingKind::Utf32Le => Some(
                text.chars()
                    .flat_map(|character| (character as u32).to_le_bytes())
                    .collect(),
            ),
            EncodingKind::Utf32Be => Some(
                text.chars()
                    .flat_map(|character| (character as u32).to_be_bytes())
                    .collect(),
            ),
            EncodingKind::EncodingRs(encoding) if encoding == UTF_8 => {
                Some(text.as_bytes().to_vec())
            }
            EncodingKind::EncodingRs(encoding) if encoding == UTF_16LE => Some(
                text.encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            ),
            EncodingKind::EncodingRs(encoding) if encoding == UTF_16BE => Some(
                text.encode_utf16()
                    .flat_map(u16::to_be_bytes)
                    .collect::<Vec<_>>(),
            ),
            EncodingKind::EncodingRs(encoding) if is_editable_stateless_encoding(encoding) => {
                let mut encoder = encoding.new_encoder();
                let capacity = encoder
                    .max_buffer_length_from_utf8_without_replacement(text.len())
                    .unwrap_or(text.len().saturating_mul(4).saturating_add(16))
                    .saturating_add(16)
                    .max(16);
                let mut output = Vec::with_capacity(capacity);
                let (result, read) =
                    encoder.encode_from_utf8_to_vec_without_replacement(text, &mut output, true);
                (result == EncoderResult::InputEmpty && read == text.len()).then_some(output)
            }
            EncodingKind::EncodingRs(_) => None,
        }
    }

    /// Returns the canonical file encoding for diagnostics and write-back metadata.
    pub(crate) fn encoding_label(&self) -> &'static str {
        self.detected.source_encoding.unwrap_or("UTF-8")
    }
}

fn is_editable_stateless_encoding(encoding: &'static Encoding) -> bool {
    encoding == GBK
        || encoding == SHIFT_JIS
        || encoding == BIG5
        || encoding == EUC_KR
        || encoding == WINDOWS_1252
}

/// Rejection reason from automatic or explicit encoding selection, each mapping to a frozen error message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EncodingRejection {
    Ambiguous { candidates: Vec<&'static str> },
    MixedOrInconsistent { conflict_hex_offset: Option<usize> },
    Iso2022JpSignature,
    Undecodable,
    BomMismatch { encoding: &'static str },
    ExplicitMalformed { encoding: String },
    InvalidLabel { value: String },
}

impl EncodingRejection {
    /// Translates an internal rejection into the model-visible error shared by read and grep.
    pub(crate) fn message(&self, path_display: &str) -> String {
        match self {
            Self::Ambiguous { candidates } => format!(
                "Cannot determine the text encoding of {path_display} with confidence: the bytes decode cleanly as {}. Retry with encoding=\"...\" if the context tells you which one, or use view=\"hex\".",
                candidates.join(", ")
            ),
            Self::MixedOrInconsistent {
                conflict_hex_offset,
            } => {
                let conflict = conflict_hex_offset.map_or_else(String::new, |offset| {
                    format!(" The first conflicting bytes are at hex-view offset {offset}.")
                });
                format!(
                    "Cannot decode {path_display} as text: it appears to contain mixed or inconsistent encodings — no single encoding explains the whole file.{conflict} Use view=\"hex\" to inspect the raw bytes, or split/normalize the file to a single encoding externally."
                )
            }
            Self::Iso2022JpSignature => format!(
                "Cannot decode {path_display} as text with confidence: the bytes are valid UTF-8 but contain ISO-2022 escape sequences (a stateful encoding such as ISO-2022-JP). Retry with encoding=\"iso-2022-jp\", or encoding=\"utf-8\" to force the raw UTF-8 reading, or use view=\"hex\"."
            ),
            Self::Undecodable => format!(
                "Cannot decode {path_display} as text: no supported encoding decodes it cleanly. Use view=\"hex\" to inspect its raw bytes."
            ),
            Self::BomMismatch { encoding } => format!(
                "Cannot decode {path_display}: it has a {encoding} byte order mark but the content is not valid {encoding}. Use view=\"hex\" to inspect its raw bytes."
            ),
            Self::ExplicitMalformed { encoding } => format!(
                "Cannot decode {path_display} as {encoding}: the content is not valid {encoding}. Try another encoding or view=\"hex\"."
            ),
            Self::InvalidLabel { value } => format!(
                "Invalid encoding value \"{value}\". Use a WHATWG encoding label such as \"gbk\", \"shift_jis\", \"big5\", \"euc-kr\", \"windows-1252\", \"utf-16le\", or \"utf-32le\"."
            ),
        }
    }

    /// Stable short reason used by grep directory skip reports.
    pub(crate) fn skip_reason(&self) -> String {
        match self {
            Self::Ambiguous { candidates } => format!("ambiguous: {}", candidates.join(", ")),
            Self::Iso2022JpSignature => "ambiguous: iso-2022-jp".to_string(),
            Self::MixedOrInconsistent { .. } => "mixed or inconsistent encodings".to_string(),
            Self::Undecodable
            | Self::BomMismatch { .. }
            | Self::ExplicitMalformed { .. }
            | Self::InvalidLabel { .. } => "undecodable".to_string(),
        }
    }
}

/// Three-state encoding decision: trusted text, binary, or reasoned rejection.
pub(crate) enum EncodingDecision {
    Text(ValidatedFileEncoding),
    Binary,
    Rejected(EncodingRejection),
}

/// I/O or content-change failure while rereading a file after initial validation.
pub(crate) enum StreamDecodeFailure {
    Io(io::Error),
    Malformed,
}

impl From<io::Error> for StreamDecodeFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<DecodeFailure> for StreamDecodeFailure {
    fn from(error: DecodeFailure) -> Self {
        match error {
            DecodeFailure::Io(error) => Self::Io(error),
            DecodeFailure::Malformed => Self::Malformed,
        }
    }
}

/// Decoded UTF-8 text and source encoding that must be disclosed to the model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedText {
    /// Strictly decoded UTF-8 content.
    pub text: String,
    /// Canonical encoding name for non-UTF-8 input; UTF-8 uses None.
    pub source_encoding: Option<String>,
}

/// Contract NUL-based binary detection examines only the first 8 KiB.
pub fn has_binary_nul(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_PROBE_BYTES).any(|byte| *byte == 0)
}

/// Validates the full file in order: explicit intent, BOM, strict UTF-8, then trusted legacy encoding.
pub(crate) fn validate_file_encoding(
    path: &Path,
    explicit_encoding: Option<&str>,
) -> io::Result<EncodingDecision> {
    validate_source_encoding(ByteSource::File(path), explicit_encoding)
}

/// Runs the full trust hierarchy over a file or an in-memory snapshot.
pub(crate) fn validate_source_encoding(
    source: ByteSource<'_>,
    explicit_encoding: Option<&str>,
) -> io::Result<EncodingDecision> {
    match snapshot_pipeline::validate_source(source, explicit_encoding, None) {
        Ok(decision) => Ok(decision),
        Err(EncodingPipelineFailure::Io(error)) => Err(error),
        Err(EncodingPipelineFailure::Stopped(_)) => {
            unreachable!("shared validation without a work checkpoint cannot stop")
        }
    }
}

/// Applies the same trust classification to in-memory bytes, returning None for ambiguous, binary, or malformed input.
pub fn decode_text(bytes: &[u8]) -> Option<DecodedText> {
    let EncodingDecision::Text(validated) =
        validate_source_encoding(ByteSource::Bytes(bytes), None).ok()?
    else {
        return None;
    };
    decode_bytes(bytes, &validated.detected).map(|text| DecodedText {
        text,
        source_encoding: validated.detected.source_encoding.map(str::to_string),
    })
}

fn validated(
    detected: DetectedEncoding,
    stats: ValidationStats,
    explicit_utf8_warning: bool,
) -> ValidatedFileEncoding {
    ValidatedFileEncoding {
        detected,
        total_lines: stats.total_lines(),
        has_trailing_newline: stats.has_trailing_newline,
        explicit_utf8_warning,
    }
}

/// Validates a WHATWG label and returns the canonical encoding name exposed to the model.
pub(crate) fn canonical_encoding_label(value: &str) -> Result<&'static str, EncodingRejection> {
    let detected = explicit_detected_encoding(value, &[])?;
    Ok(detected.source_encoding.unwrap_or("UTF-8"))
}

fn bom_detected_encoding(bytes: &[u8]) -> Option<DetectedEncoding> {
    if bytes.starts_with(b"\x00\x00\xFE\xFF") {
        return Some(DetectedEncoding {
            kind: EncodingKind::Utf32Be,
            bom_len: 4,
            source_encoding: Some("UTF-32BE"),
            origin: EncodingOrigin::Bom("UTF-32BE"),
        });
    }
    if bytes.starts_with(b"\xFF\xFE\x00\x00") {
        return Some(DetectedEncoding {
            kind: EncodingKind::Utf32Le,
            bom_len: 4,
            source_encoding: Some("UTF-32LE"),
            origin: EncodingOrigin::Bom("UTF-32LE"),
        });
    }
    if bytes.starts_with(b"\xEF\xBB\xBF") {
        return Some(DetectedEncoding {
            kind: EncodingKind::EncodingRs(UTF_8),
            bom_len: 3,
            source_encoding: None,
            origin: EncodingOrigin::Bom("UTF-8"),
        });
    }
    if bytes.starts_with(b"\xFF\xFE") {
        return Some(DetectedEncoding {
            kind: EncodingKind::EncodingRs(UTF_16LE),
            bom_len: 2,
            source_encoding: Some("UTF-16LE"),
            origin: EncodingOrigin::Bom("UTF-16LE"),
        });
    }
    if bytes.starts_with(b"\xFE\xFF") {
        return Some(DetectedEncoding {
            kind: EncodingKind::EncodingRs(UTF_16BE),
            bom_len: 2,
            source_encoding: Some("UTF-16BE"),
            origin: EncodingOrigin::Bom("UTF-16BE"),
        });
    }
    None
}

fn explicit_detected_encoding(
    value: &str,
    prefix: &[u8],
) -> Result<DetectedEncoding, EncodingRejection> {
    let label = value.trim_matches(|character: char| character.is_ascii_whitespace());
    let (kind, source_encoding) = if label.eq_ignore_ascii_case("utf-32le") {
        (EncodingKind::Utf32Le, Some("UTF-32LE"))
    } else if label.eq_ignore_ascii_case("utf-32be") {
        (EncodingKind::Utf32Be, Some("UTF-32BE"))
    } else {
        let Some(encoding) = Encoding::for_label_no_replacement(label.as_bytes()) else {
            return Err(EncodingRejection::InvalidLabel {
                value: value.to_string(),
            });
        };
        (
            EncodingKind::EncodingRs(encoding),
            (encoding != UTF_8).then_some(encoding.name()),
        )
    };
    let bom_len = matching_bom_len(kind, prefix);
    Ok(DetectedEncoding {
        kind,
        bom_len,
        source_encoding,
        origin: EncodingOrigin::Explicit(value.to_string()),
    })
}

fn matching_bom_len(kind: EncodingKind, bytes: &[u8]) -> u64 {
    match kind {
        EncodingKind::Utf32Le if bytes.starts_with(b"\xFF\xFE\x00\x00") => 4,
        EncodingKind::Utf32Be if bytes.starts_with(b"\x00\x00\xFE\xFF") => 4,
        EncodingKind::EncodingRs(encoding)
            if encoding == UTF_8 && bytes.starts_with(b"\xEF\xBB\xBF") =>
        {
            3
        }
        EncodingKind::EncodingRs(encoding)
            if encoding == UTF_16LE && bytes.starts_with(b"\xFF\xFE") =>
        {
            2
        }
        EncodingKind::EncodingRs(encoding)
            if encoding == UTF_16BE && bytes.starts_with(b"\xFE\xFF") =>
        {
            2
        }
        _ => 0,
    }
}

fn is_legacy_encoding(kind: EncodingKind) -> bool {
    matches!(
        kind,
        EncodingKind::EncodingRs(encoding)
            if encoding != UTF_8 && encoding != UTF_16LE && encoding != UTF_16BE
    )
}

/// Incremental form of the v0.1.1 strong-UTF-8-prefix conflict detector.
pub(crate) struct Utf8ConflictProbe {
    scanner: Utf8PrefixScanner,
}

impl Utf8ConflictProbe {
    pub(crate) fn new() -> Self {
        Self {
            scanner: Utf8PrefixScanner::default(),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Option<usize> {
        self.scanner.push(bytes)
    }
}

#[derive(Default)]
struct Utf8PrefixScanner {
    absolute_base: usize,
    valid_bytes: usize,
    non_ascii_bytes: usize,
    carry: Vec<u8>,
    abandoned: bool,
}

impl Utf8PrefixScanner {
    fn push(&mut self, input: &[u8]) -> Option<usize> {
        if self.abandoned {
            return None;
        }
        let mut combined = std::mem::take(&mut self.carry);
        combined.extend_from_slice(input);
        match std::str::from_utf8(&combined) {
            Ok(_) => {
                self.observe_valid(&combined);
                self.absolute_base = self.absolute_base.saturating_add(combined.len());
                None
            }
            Err(error) => {
                let valid = &combined[..error.valid_up_to()];
                self.observe_valid(valid);
                if error.error_len().is_some() {
                    let conflict = self.absolute_base.saturating_add(error.valid_up_to());
                    if self.clear_prefix() {
                        return Some(conflict / 16 + 1);
                    }
                    self.abandoned = true;
                    return None;
                }
                self.absolute_base = self.absolute_base.saturating_add(error.valid_up_to());
                self.carry
                    .extend_from_slice(&combined[error.valid_up_to()..]);
                None
            }
        }
    }

    fn finish(mut self) -> Option<usize> {
        if self.abandoned || self.carry.is_empty() {
            return None;
        }
        let carry = std::mem::take(&mut self.carry);
        match std::str::from_utf8(&carry) {
            Ok(_) => None,
            Err(error) => {
                self.observe_valid(&carry[..error.valid_up_to()]);
                self.clear_prefix()
                    .then_some((self.absolute_base + error.valid_up_to()) / 16 + 1)
            }
        }
    }

    fn observe_valid(&mut self, bytes: &[u8]) {
        self.valid_bytes = self.valid_bytes.saturating_add(bytes.len());
        self.non_ascii_bytes = self
            .non_ascii_bytes
            .saturating_add(bytes.iter().filter(|byte| !byte.is_ascii()).count());
    }

    fn clear_prefix(&self) -> bool {
        self.valid_bytes >= UTF8_SEGMENT_MIN_BYTES
            && self.non_ascii_bytes >= UTF8_SEGMENT_MIN_NON_ASCII_BYTES
    }
}

fn is_disallowed_legacy_character(character: char) -> bool {
    let value = character as u32;
    (value <= 0x1F && !matches!(character, '\t' | '\n' | '\r'))
        || (0x80..=0x9F).contains(&value)
        || (0xFDD0..=0xFDEF).contains(&value)
        || value & 0xFFFF == 0xFFFE
        || value & 0xFFFF == 0xFFFF
}

#[derive(Clone, Default)]
struct ValidationStats {
    decoded_any: bool,
    newline_count: usize,
    has_trailing_newline: bool,
    has_non_ascii: bool,
    has_iso_2022_escape: bool,
    previous_was_escape: bool,
}

impl ValidationStats {
    fn observe(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.decoded_any = true;
        self.has_non_ascii |= !text.is_ascii();
        for byte in text.bytes() {
            if self.previous_was_escape && matches!(byte, b'(' | b'$') {
                self.has_iso_2022_escape = true;
            }
            self.previous_was_escape = byte == 0x1B;
        }
        self.newline_count = self
            .newline_count
            .saturating_add(text.bytes().filter(|byte| *byte == b'\n').count());
        self.has_trailing_newline = text.ends_with('\n');
    }

    fn total_lines(&self) -> usize {
        if self.decoded_any {
            self.newline_count.saturating_add(1)
        } else {
            0
        }
    }
}

enum DecodeFailure {
    Io(io::Error),
    Malformed,
}

/// A strict decoder used only to establish irreversible capture-time failures.
pub(crate) struct StrictStreamingValidator {
    decoder: ChunkDecoder,
    malformed: bool,
}

impl StrictStreamingValidator {
    fn new(detected: &DetectedEncoding) -> Self {
        Self {
            decoder: chunk_decoder_for(detected),
            malformed: false,
        }
    }

    /// Returns true once the already-observed prefix can never decode successfully.
    pub(crate) fn feed(&mut self, input: &[u8]) -> bool {
        if self.malformed {
            return true;
        }
        let result = match &mut self.decoder {
            ChunkDecoder::Utf8 { carry } => decode_utf8_chunk(carry, input, false),
            ChunkDecoder::EncodingRs(decoder) => decode_encoding_rs_chunk(decoder, input, false),
            ChunkDecoder::Utf32 {
                little_endian,
                carry,
            } => decode_utf32_chunk(carry, input, false, *little_endian),
        };
        self.malformed = result.is_err();
        self.malformed
    }
}

/// Capture-time decoder selected by an authoritative BOM.
pub(crate) struct BomStreamingValidator {
    pub(crate) validator: StrictStreamingValidator,
    pub(crate) bom_len: usize,
    pub(crate) encoding: &'static str,
    pub(crate) is_utf8: bool,
}

/// Builds the same strict decoder and matching-BOM offset as explicit validation.
pub(crate) fn explicit_streaming_validator(
    value: &str,
    prefix: &[u8],
) -> Result<(StrictStreamingValidator, usize), EncodingRejection> {
    let detected = explicit_detected_encoding(value, prefix)?;
    Ok((
        StrictStreamingValidator::new(&detected),
        detected.bom_len as usize,
    ))
}

/// Builds the same strict decoder selected by the normal BOM precedence.
pub(crate) fn bom_streaming_validator(prefix: &[u8]) -> Option<BomStreamingValidator> {
    let detected = bom_detected_encoding(prefix)?;
    let encoding = match detected.origin {
        EncodingOrigin::Bom(encoding) => encoding,
        _ => unreachable!("BOM detection always records a BOM origin"),
    };
    Some(BomStreamingValidator {
        validator: StrictStreamingValidator::new(&detected),
        bom_len: detected.bom_len as usize,
        encoding,
        is_utf8: detected.kind == EncodingKind::EncodingRs(UTF_8),
    })
}

impl From<io::Error> for DecodeFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

enum ChunkDecoder {
    Utf8 { carry: Vec<u8> },
    EncodingRs(encoding_rs::Decoder),
    Utf32 { little_endian: bool, carry: Vec<u8> },
}

struct DecodedChunkReader<'a> {
    source: SourceReader<'a>,
    decoder: ChunkDecoder,
    finished: bool,
}

impl<'a> DecodedChunkReader<'a> {
    fn open(source: ByteSource<'a>, detected: DetectedEncoding) -> io::Result<Self> {
        Ok(Self {
            source: SourceReader::open(source, detected.bom_len)?,
            decoder: chunk_decoder_for(&detected),
            finished: false,
        })
    }

    fn next_chunk(&mut self) -> Result<Option<String>, DecodeFailure> {
        if self.finished {
            return Ok(None);
        }
        loop {
            let mut input = [0_u8; DECODE_CHUNK_BYTES];
            let count = self.source.read(&mut input)?;
            let is_last = count == 0;
            let decoded = match &mut self.decoder {
                ChunkDecoder::Utf8 { carry } => decode_utf8_chunk(carry, &input[..count], is_last)?,
                ChunkDecoder::EncodingRs(decoder) => {
                    decode_encoding_rs_chunk(decoder, &input[..count], is_last)?
                }
                ChunkDecoder::Utf32 {
                    little_endian,
                    carry,
                } => decode_utf32_chunk(carry, &input[..count], is_last, *little_endian)?,
            };
            if is_last {
                self.finished = true;
            }
            if !decoded.is_empty() {
                return Ok(Some(decoded));
            }
            if self.finished {
                return Ok(None);
            }
        }
    }
}

fn chunk_decoder_for(detected: &DetectedEncoding) -> ChunkDecoder {
    match detected.kind {
        EncodingKind::EncodingRs(encoding) if encoding == UTF_8 => {
            ChunkDecoder::Utf8 { carry: Vec::new() }
        }
        EncodingKind::EncodingRs(encoding) => {
            ChunkDecoder::EncodingRs(encoding.new_decoder_without_bom_handling())
        }
        EncodingKind::Utf32Le => ChunkDecoder::Utf32 {
            little_endian: true,
            carry: Vec::new(),
        },
        EncodingKind::Utf32Be => ChunkDecoder::Utf32 {
            little_endian: false,
            carry: Vec::new(),
        },
    }
}

fn decode_utf8_chunk(
    carry: &mut Vec<u8>,
    input: &[u8],
    is_last: bool,
) -> Result<String, DecodeFailure> {
    let mut bytes = std::mem::take(carry);
    bytes.extend_from_slice(input);
    match std::str::from_utf8(&bytes) {
        Ok(text) => Ok(text.to_string()),
        Err(error) if error.error_len().is_none() && !is_last => {
            let valid_up_to = error.valid_up_to();
            let tail = bytes.split_off(valid_up_to);
            *carry = tail;
            std::str::from_utf8(&bytes)
                .map(str::to_string)
                .map_err(|_| DecodeFailure::Malformed)
        }
        Err(_) => Err(DecodeFailure::Malformed),
    }
}

fn decode_encoding_rs_chunk(
    decoder: &mut encoding_rs::Decoder,
    input: &[u8],
    is_last: bool,
) -> Result<String, DecodeFailure> {
    let mut consumed = 0_usize;
    let mut output = String::new();
    loop {
        let remaining = &input[consumed..];
        let capacity = decoder
            .max_utf8_buffer_length_without_replacement(remaining.len())
            .ok_or(DecodeFailure::Malformed)?
            .max(4);
        let mut decoded = String::with_capacity(capacity);
        let (result, read) =
            decoder.decode_to_string_without_replacement(remaining, &mut decoded, is_last);
        consumed = consumed.saturating_add(read);
        output.push_str(&decoded);
        match result {
            DecoderResult::InputEmpty => return Ok(output),
            DecoderResult::OutputFull => continue,
            DecoderResult::Malformed(_, _) => return Err(DecodeFailure::Malformed),
        }
    }
}

fn decode_utf32_chunk(
    carry: &mut Vec<u8>,
    input: &[u8],
    is_last: bool,
    little_endian: bool,
) -> Result<String, DecodeFailure> {
    let mut bytes = std::mem::take(carry);
    bytes.extend_from_slice(input);
    if is_last && !bytes.len().is_multiple_of(4) {
        return Err(DecodeFailure::Malformed);
    }
    let complete_len = bytes.len() / 4 * 4;
    if !is_last {
        *carry = bytes.split_off(complete_len);
    }
    let mut output = String::with_capacity(complete_len);
    for raw in bytes[..complete_len].chunks_exact(4) {
        let unit = if little_endian {
            u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])
        } else {
            u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])
        };
        let Some(character) = char::from_u32(unit) else {
            return Err(DecodeFailure::Malformed);
        };
        output.push(character);
    }
    Ok(output)
}

/// UTF-8 streaming reader for validated text that never emits replacement characters.
pub(crate) struct Utf8Reader<'a> {
    decoder: DecodedChunkReader<'a>,
    pending: Vec<u8>,
    pending_offset: usize,
}

impl<'a> Utf8Reader<'a> {
    fn open_source(source: ByteSource<'a>, detected: DetectedEncoding) -> io::Result<Self> {
        Ok(Self {
            decoder: DecodedChunkReader::open(source, detected)?,
            pending: Vec::new(),
            pending_offset: 0,
        })
    }
}

impl Read for Utf8Reader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            if self.pending_offset < self.pending.len() {
                let available = &self.pending[self.pending_offset..];
                let count = available.len().min(output.len());
                output[..count].copy_from_slice(&available[..count]);
                self.pending_offset += count;
                return Ok(count);
            }
            match self.decoder.next_chunk() {
                Ok(Some(chunk)) => {
                    self.pending = chunk.into_bytes();
                    self.pending_offset = 0;
                }
                Ok(None) => return Ok(0),
                Err(DecodeFailure::Io(error)) => return Err(error),
                Err(DecodeFailure::Malformed) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "file changed after encoding validation",
                    ));
                }
            }
        }
    }
}

fn decode_bytes(bytes: &[u8], detected: &DetectedEncoding) -> Option<String> {
    let content = bytes.get(detected.bom_len as usize..)?;
    match detected.kind {
        EncodingKind::EncodingRs(encoding) => encoding
            .decode_without_bom_handling_and_without_replacement(content)
            .map(|text| text.into_owned()),
        EncodingKind::Utf32Le => decode_utf32_bytes(content, true),
        EncodingKind::Utf32Be => decode_utf32_bytes(content, false),
    }
}

fn decode_utf32_bytes(bytes: &[u8], little_endian: bool) -> Option<String> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut output = String::with_capacity(bytes.len());
    for raw in bytes.chunks_exact(4) {
        let unit = if little_endian {
            u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])
        } else {
            u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])
        };
        output.push(char::from_u32(unit)?);
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::{
        DECODE_CHUNK_BYTES, EncodingDecision, decode_text, has_binary_nul,
        is_disallowed_legacy_character, validate_file_encoding,
    };
    use std::io::Read;

    #[test]
    fn nul_is_binary_but_unicode_boms_take_precedence() {
        assert!(decode_text(b"text\0binary").is_none());
        let decoded = decode_text(&[0xFF, 0xFE, b'A', 0, b'B', 0]).unwrap();
        assert_eq!(decoded.text, "AB");
        assert_eq!(decoded.source_encoding.as_deref(), Some("UTF-16LE"));

        let decoded = decode_text(&[0x00, 0x00, 0xFE, 0xFF, 0x00, 0x00, 0x00, b'A']).unwrap();
        assert_eq!(decoded.text, "A");
        assert_eq!(decoded.source_encoding.as_deref(), Some("UTF-32BE"));
    }

    #[test]
    fn low_evidence_legacy_bytes_are_never_returned_as_guessed_text() {
        assert!(decode_text(b"valid\xFFtail").is_none());
    }

    #[test]
    fn binary_probe_stops_after_exactly_eight_kibibytes() {
        let mut inside = vec![b'a'; 8 * 1024];
        inside[8 * 1024 - 1] = 0;
        assert!(has_binary_nul(&inside));

        let mut outside = vec![b'a'; 8 * 1024];
        outside.push(0);
        assert!(!has_binary_nul(&outside));
    }

    #[test]
    fn arbitrary_byte_fuzz_corpus_never_panics() {
        let mut state = 0xA5A5_5A5A_1234_5678_u64;
        for length in 0..=256 {
            let mut bytes = Vec::with_capacity(length);
            for _ in 0..length {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                bytes.push((state >> 32) as u8);
            }
            let _ = decode_text(&bytes);
        }
        for byte in 0_u8..=u8::MAX {
            let _ = decode_text(&[byte]);
            let _ = decode_text(&[0xA1, byte]);
            let _ = decode_text(&[byte, 0xFF, 0x00, 0x7F]);
        }
    }

    #[test]
    fn legacy_hard_filter_rejects_controls_and_unicode_noncharacters() {
        for character in [
            '\0',
            '\u{0001}',
            '\u{0080}',
            '\u{009F}',
            '\u{FDD0}',
            '\u{10FFFF}',
        ] {
            assert!(is_disallowed_legacy_character(character));
        }
        for character in ['\t', '\n', '\r', 'a', '界'] {
            assert!(!is_disallowed_legacy_character(character));
        }
    }

    #[test]
    fn strict_decoders_preserve_characters_split_at_internal_chunk_boundaries() {
        let temp = tempfile::tempdir().unwrap();

        let utf8_path = temp.path().join("utf8-boundary.txt");
        let utf8_text = format!("{}界", "a".repeat(DECODE_CHUNK_BYTES - 1));
        std::fs::write(&utf8_path, utf8_text.as_bytes()).unwrap();
        assert_decoded_file(&utf8_path, &utf8_text);

        let utf16_path = temp.path().join("utf16-boundary.txt");
        let utf16_text = format!("{}😀Z", "a".repeat(DECODE_CHUNK_BYTES / 2 - 1));
        let mut utf16_bytes = vec![0xFF, 0xFE];
        for unit in utf16_text.encode_utf16() {
            utf16_bytes.extend(unit.to_le_bytes());
        }
        std::fs::write(&utf16_path, utf16_bytes).unwrap();
        assert_decoded_file(&utf16_path, &utf16_text);

        let gbk_path = temp.path().join("gbk-boundary.txt");
        let mut gbk_bytes = vec![b'a'];
        for _ in 0..=DECODE_CHUNK_BYTES / 2 {
            gbk_bytes.extend([0xD6, 0xD0]);
        }
        std::fs::write(&gbk_path, gbk_bytes).unwrap();
        let gbk_text = format!("a{}", "中".repeat(DECODE_CHUNK_BYTES / 2 + 1));
        assert_decoded_file(&gbk_path, &gbk_text);
    }

    fn assert_decoded_file(path: &std::path::Path, expected: &str) {
        let EncodingDecision::Text(validated) = validate_file_encoding(path, None).unwrap() else {
            panic!("expected trusted text");
        };
        let mut reader = validated
            .open_source_reader(super::ByteSource::File(path))
            .unwrap();
        let mut actual = String::new();
        reader.read_to_string(&mut actual).unwrap();
        assert_eq!(actual, expected);
    }
}
