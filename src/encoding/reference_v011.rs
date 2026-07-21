//! Test-only copy of the v0.1.1 encoding trust classifier.
//!
//! This module is intentionally independent from the production pipeline. Its
//! byte-oriented decision tree was extracted from commit
//! `64a6a45f88e65a2c0305e36673fa5e3f99d95384` and remains the differential oracle.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{
    BIG5, EUC_KR, EncoderResult, Encoding, GBK, SHIFT_JIS, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252,
};

const BINARY_PROBE_BYTES: usize = 8 * 1024;
const LEGACY_EVIDENCE_BYTES: usize = 32;
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

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ReferenceDecision {
    Text {
        kind: String,
        bom_len: usize,
        source_encoding: Option<String>,
        origin: ReferenceOrigin,
        total_lines: usize,
        has_trailing_newline: bool,
        explicit_utf8_warning: bool,
        note: Option<String>,
    },
    Binary,
    Rejected(ReferenceRejection),
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ReferenceOrigin {
    Explicit(String),
    Bom(String),
    Automatic,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ReferenceRejection {
    Ambiguous(Vec<String>),
    MixedOrInconsistent(Option<usize>),
    Iso2022JpSignature,
    Undecodable,
    BomMismatch(String),
    ExplicitMalformed(String),
    InvalidLabel(String),
}

pub(super) fn classify_v011(bytes: &[u8], explicit_encoding: Option<&str>) -> ReferenceDecision {
    let prefix = &bytes[..bytes.len().min(BINARY_PROBE_BYTES)];
    if let Some(value) = explicit_encoding {
        let detected = match explicit_detected_encoding(value, prefix) {
            Ok(detected) => detected,
            Err(rejection) => return ReferenceDecision::Rejected(rejection),
        };
        return match validate_selected(bytes, &detected, false) {
            Some(stats) => {
                let explicit_utf8_warning = is_legacy_encoding(detected.kind)
                    && std::str::from_utf8(bytes).is_ok_and(|text| !text.is_ascii());
                text_decision(detected, stats, explicit_utf8_warning)
            }
            None => ReferenceDecision::Rejected(ReferenceRejection::ExplicitMalformed(
                value.to_string(),
            )),
        };
    }

    if let Some(detected) = bom_detected_encoding(prefix) {
        if detected.kind == ReferenceKind::EncodingRs(UTF_8) && has_binary_nul(prefix) {
            return ReferenceDecision::Binary;
        }
        return match validate_selected(bytes, &detected, false) {
            Some(stats) => text_decision(detected, stats, false),
            None => ReferenceDecision::Rejected(ReferenceRejection::BomMismatch(
                detected
                    .source_encoding
                    .map(str::to_string)
                    .or_else(|| match &detected.origin {
                        ReferenceOrigin::Bom(encoding) => Some(encoding.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "UTF-8".to_string()),
            )),
        };
    }

    if has_binary_nul(prefix) {
        return ReferenceDecision::Binary;
    }
    let utf8 = ReferenceDetected {
        kind: ReferenceKind::EncodingRs(UTF_8),
        bom_len: 0,
        source_encoding: None,
        origin: ReferenceOrigin::Automatic,
    };
    if let Some(stats) = validate_selected(bytes, &utf8, false) {
        if stats.has_iso_2022_escape {
            return ReferenceDecision::Rejected(ReferenceRejection::Iso2022JpSignature);
        }
        return text_decision(utf8, stats, false);
    }
    if has_reference_binary_magic(prefix) {
        return ReferenceDecision::Binary;
    }

    let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let candidate = detector.guess(None, Utf8Detection::Deny);
    let non_ascii_bytes = bytes.iter().filter(|byte| !byte.is_ascii()).count();
    let candidate_detected = automatic_detected(candidate);
    if let Some(conflict_hex_offset) = first_conflicting_utf8_hex_offset(bytes) {
        return ReferenceDecision::Rejected(ReferenceRejection::MixedOrInconsistent(Some(
            conflict_hex_offset,
        )));
    }
    if non_ascii_bytes >= LEGACY_EVIDENCE_BYTES
        && let Some(stats) = validate_selected(bytes, &candidate_detected, true)
    {
        if legacy_segments_are_inconsistent(bytes, candidate) {
            return ReferenceDecision::Rejected(ReferenceRejection::MixedOrInconsistent(None));
        }
        if !candidate.is_single_byte() {
            return text_decision(candidate_detected, stats, false);
        }
    }

    let mut candidates = Vec::new();
    for (label, encoding) in FIXED_LEGACY_ENCODINGS {
        if validate_selected(bytes, &automatic_detected(encoding), true).is_some() {
            candidates.push(label.to_string());
        }
    }
    if candidates.is_empty() {
        ReferenceDecision::Rejected(ReferenceRejection::Undecodable)
    } else {
        ReferenceDecision::Rejected(ReferenceRejection::Ambiguous(candidates))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ReferenceKind {
    EncodingRs(&'static Encoding),
    Utf32Le,
    Utf32Be,
}

struct ReferenceDetected {
    kind: ReferenceKind,
    bom_len: usize,
    source_encoding: Option<&'static str>,
    origin: ReferenceOrigin,
}

fn automatic_detected(encoding: &'static Encoding) -> ReferenceDetected {
    ReferenceDetected {
        kind: ReferenceKind::EncodingRs(encoding),
        bom_len: 0,
        source_encoding: Some(encoding.name()),
        origin: ReferenceOrigin::Automatic,
    }
}

fn text_decision(
    detected: ReferenceDetected,
    stats: ReferenceStats,
    explicit_utf8_warning: bool,
) -> ReferenceDecision {
    let kind = match detected.kind {
        ReferenceKind::EncodingRs(encoding) => encoding.name().to_string(),
        ReferenceKind::Utf32Le => "UTF-32LE".to_string(),
        ReferenceKind::Utf32Be => "UTF-32BE".to_string(),
    };
    let note = transcoding_note(
        detected.source_encoding,
        &detected.origin,
        explicit_utf8_warning,
    );
    ReferenceDecision::Text {
        kind,
        bom_len: detected.bom_len,
        source_encoding: detected.source_encoding.map(str::to_string),
        origin: detected.origin,
        total_lines: stats.total_lines,
        has_trailing_newline: stats.has_trailing_newline,
        explicit_utf8_warning,
        note,
    }
}

fn transcoding_note(
    source_encoding: Option<&'static str>,
    origin: &ReferenceOrigin,
    explicit_utf8_warning: bool,
) -> Option<String> {
    let encoding = source_encoding?;
    match origin {
        ReferenceOrigin::Explicit(_) if explicit_utf8_warning => Some(format!(
            "(Note: decoded from {encoding} as requested; output is UTF-8. Warning: the raw bytes are also valid UTF-8 — if this looks garbled, retry with encoding=\"utf-8\" or omit encoding.)"
        )),
        ReferenceOrigin::Explicit(_) => Some(format!(
            "(Note: decoded from {encoding} as requested; output is UTF-8.)"
        )),
        ReferenceOrigin::Bom(_) | ReferenceOrigin::Automatic => {
            Some(format!("(Note: decoded from {encoding}; output is UTF-8.)"))
        }
    }
}

fn has_binary_nul(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_PROBE_BYTES).any(|byte| *byte == 0)
}

fn has_reference_binary_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"\x1F\x8B")
        || bytes.starts_with(b"\x37\x7A\xBC\xAF\x27\x1C")
        || bytes.starts_with(b"\x28\xB5\x2F\xFD")
        || bytes.starts_with(b"\x7FELF")
        || bytes.starts_with(b"MZ")
        || [
            b"\xFE\xED\xFA\xCE".as_slice(),
            b"\xFE\xED\xFA\xCF".as_slice(),
            b"\xCE\xFA\xED\xFE".as_slice(),
            b"\xCF\xFA\xED\xFE".as_slice(),
        ]
        .iter()
        .any(|magic| bytes.starts_with(magic))
        || bytes.starts_with(b"SQLite format 3\0")
        || bytes.starts_with(b"\0asm")
        || bytes.get(257..262) == Some(b"ustar")
}

fn bom_detected_encoding(bytes: &[u8]) -> Option<ReferenceDetected> {
    if bytes.starts_with(b"\x00\x00\xFE\xFF") {
        return Some(ReferenceDetected {
            kind: ReferenceKind::Utf32Be,
            bom_len: 4,
            source_encoding: Some("UTF-32BE"),
            origin: ReferenceOrigin::Bom("UTF-32BE".to_string()),
        });
    }
    if bytes.starts_with(b"\xFF\xFE\x00\x00") {
        return Some(ReferenceDetected {
            kind: ReferenceKind::Utf32Le,
            bom_len: 4,
            source_encoding: Some("UTF-32LE"),
            origin: ReferenceOrigin::Bom("UTF-32LE".to_string()),
        });
    }
    if bytes.starts_with(b"\xEF\xBB\xBF") {
        return Some(ReferenceDetected {
            kind: ReferenceKind::EncodingRs(UTF_8),
            bom_len: 3,
            source_encoding: None,
            origin: ReferenceOrigin::Bom("UTF-8".to_string()),
        });
    }
    if bytes.starts_with(b"\xFF\xFE") {
        return Some(ReferenceDetected {
            kind: ReferenceKind::EncodingRs(UTF_16LE),
            bom_len: 2,
            source_encoding: Some("UTF-16LE"),
            origin: ReferenceOrigin::Bom("UTF-16LE".to_string()),
        });
    }
    if bytes.starts_with(b"\xFE\xFF") {
        return Some(ReferenceDetected {
            kind: ReferenceKind::EncodingRs(UTF_16BE),
            bom_len: 2,
            source_encoding: Some("UTF-16BE"),
            origin: ReferenceOrigin::Bom("UTF-16BE".to_string()),
        });
    }
    None
}

fn explicit_detected_encoding(
    value: &str,
    prefix: &[u8],
) -> Result<ReferenceDetected, ReferenceRejection> {
    let label = value.trim_matches(|character: char| character.is_ascii_whitespace());
    let (kind, source_encoding) = if label.eq_ignore_ascii_case("utf-32le") {
        (ReferenceKind::Utf32Le, Some("UTF-32LE"))
    } else if label.eq_ignore_ascii_case("utf-32be") {
        (ReferenceKind::Utf32Be, Some("UTF-32BE"))
    } else {
        let Some(encoding) = Encoding::for_label_no_replacement(label.as_bytes()) else {
            return Err(ReferenceRejection::InvalidLabel(value.to_string()));
        };
        (
            ReferenceKind::EncodingRs(encoding),
            (encoding != UTF_8).then_some(encoding.name()),
        )
    };
    Ok(ReferenceDetected {
        kind,
        bom_len: matching_bom_len(kind, prefix),
        source_encoding,
        origin: ReferenceOrigin::Explicit(value.to_string()),
    })
}

fn matching_bom_len(kind: ReferenceKind, bytes: &[u8]) -> usize {
    match kind {
        ReferenceKind::Utf32Le if bytes.starts_with(b"\xFF\xFE\x00\x00") => 4,
        ReferenceKind::Utf32Be if bytes.starts_with(b"\x00\x00\xFE\xFF") => 4,
        ReferenceKind::EncodingRs(encoding)
            if encoding == UTF_8 && bytes.starts_with(b"\xEF\xBB\xBF") =>
        {
            3
        }
        ReferenceKind::EncodingRs(encoding)
            if encoding == UTF_16LE && bytes.starts_with(b"\xFF\xFE") =>
        {
            2
        }
        ReferenceKind::EncodingRs(encoding)
            if encoding == UTF_16BE && bytes.starts_with(b"\xFE\xFF") =>
        {
            2
        }
        _ => 0,
    }
}

fn is_legacy_encoding(kind: ReferenceKind) -> bool {
    matches!(
        kind,
        ReferenceKind::EncodingRs(encoding)
            if encoding != UTF_8 && encoding != UTF_16LE && encoding != UTF_16BE
    )
}

struct ReferenceStats {
    total_lines: usize,
    has_trailing_newline: bool,
    has_iso_2022_escape: bool,
}

fn validate_selected(
    bytes: &[u8],
    detected: &ReferenceDetected,
    enforce_legacy_hard_checks: bool,
) -> Option<ReferenceStats> {
    let content = bytes.get(detected.bom_len..)?;
    let decoded = decode_bytes(content, detected.kind)?;
    if enforce_legacy_hard_checks {
        let ReferenceKind::EncodingRs(encoding) = detected.kind else {
            return None;
        };
        if !hard_validate_bytes(encoding, content) {
            return None;
        }
    }
    let decoded_any = !decoded.is_empty();
    let newline_count = decoded.bytes().filter(|byte| *byte == b'\n').count();
    Some(ReferenceStats {
        total_lines: if decoded_any { newline_count + 1 } else { 0 },
        has_trailing_newline: decoded.ends_with('\n'),
        has_iso_2022_escape: contains_iso_2022_escape(decoded.as_bytes()),
    })
}

fn decode_bytes(bytes: &[u8], kind: ReferenceKind) -> Option<String> {
    match kind {
        ReferenceKind::EncodingRs(encoding) => encoding
            .decode_without_bom_handling_and_without_replacement(bytes)
            .map(|text| text.into_owned()),
        ReferenceKind::Utf32Le => decode_utf32(bytes, true),
        ReferenceKind::Utf32Be => decode_utf32(bytes, false),
    }
}

fn decode_utf32(bytes: &[u8], little_endian: bool) -> Option<String> {
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

fn hard_validate_bytes(encoding: &'static Encoding, bytes: &[u8]) -> bool {
    let Some(decoded) = encoding.decode_without_bom_handling_and_without_replacement(bytes) else {
        return false;
    };
    if decoded.chars().any(is_disallowed_legacy_character) {
        return false;
    }
    let mut encoder = encoding.new_encoder();
    let capacity = encoder
        .max_buffer_length_from_utf8_without_replacement(decoded.len())
        .unwrap_or(decoded.len().saturating_mul(4).saturating_add(16))
        .saturating_add(16)
        .max(16);
    let mut encoded = Vec::with_capacity(capacity);
    let (result, read) =
        encoder.encode_from_utf8_to_vec_without_replacement(&decoded, &mut encoded, true);
    result == EncoderResult::InputEmpty && read == decoded.len() && encoded == bytes
}

fn is_disallowed_legacy_character(character: char) -> bool {
    let value = character as u32;
    (value <= 0x1F && !matches!(character, '\t' | '\n' | '\r'))
        || (0x80..=0x9F).contains(&value)
        || (0xFDD0..=0xFDEF).contains(&value)
        || value & 0xFFFF == 0xFFFE
        || value & 0xFFFF == 0xFFFF
}

fn first_conflicting_utf8_hex_offset(bytes: &[u8]) -> Option<usize> {
    let mut scanner = ReferenceUtf8PrefixScanner::default();
    scanner.push(bytes).or_else(|| scanner.finish())
}

#[derive(Default)]
struct ReferenceUtf8PrefixScanner {
    absolute_base: usize,
    valid_bytes: usize,
    non_ascii_bytes: usize,
    carry: Vec<u8>,
    abandoned: bool,
}

impl ReferenceUtf8PrefixScanner {
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

fn legacy_segments_are_inconsistent(bytes: &[u8], whole_encoding: &'static Encoding) -> bool {
    let mut scanner = ReferenceLegacySegmentScanner::new(whole_encoding);
    scanner.push(bytes) || scanner.finish()
}

struct ReferenceLegacySegmentScanner {
    whole_encoding: &'static Encoding,
    pending: Vec<u8>,
}

impl ReferenceLegacySegmentScanner {
    fn new(whole_encoding: &'static Encoding) -> Self {
        Self {
            whole_encoding,
            pending: Vec::with_capacity(LEGACY_SEGMENT_MAX_BYTES),
        }
    }

    fn push(&mut self, input: &[u8]) -> bool {
        for byte in input {
            self.pending.push(*byte);
            if *byte == b'\n' && has_segment_evidence(&self.pending) {
                if self.inspect_pending() {
                    return true;
                }
                self.pending.clear();
            } else if self.pending.len() >= LEGACY_SEGMENT_MAX_BYTES {
                let end = aligned_prefix_len(self.whole_encoding, &self.pending);
                if end == 0 {
                    continue;
                }
                if segment_disagrees(self.whole_encoding, &self.pending[..end]) {
                    return true;
                }
                self.pending.drain(..end);
            }
        }
        false
    }

    fn finish(&mut self) -> bool {
        self.inspect_pending()
    }

    fn inspect_pending(&self) -> bool {
        has_segment_evidence(&self.pending)
            && hard_validate_bytes(self.whole_encoding, &self.pending)
            && segment_disagrees(self.whole_encoding, &self.pending)
    }
}

fn aligned_prefix_len(encoding: &'static Encoding, bytes: &[u8]) -> usize {
    (0..=4)
        .filter_map(|trim| bytes.len().checked_sub(trim))
        .find(|end| hard_validate_bytes(encoding, &bytes[..*end]))
        .unwrap_or(0)
}

fn has_segment_evidence(bytes: &[u8]) -> bool {
    bytes.iter().filter(|byte| !byte.is_ascii()).count() >= LEGACY_EVIDENCE_BYTES
        || is_strong_utf8_segment(bytes)
}

fn segment_disagrees(whole_encoding: &'static Encoding, bytes: &[u8]) -> bool {
    if is_strong_utf8_segment(bytes) {
        return true;
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let candidate = detector.guess(None, Utf8Detection::Deny);
    candidate != whole_encoding
        && !candidate.is_single_byte()
        && hard_validate_bytes(candidate, bytes)
}

fn is_strong_utf8_segment(bytes: &[u8]) -> bool {
    bytes.len() >= UTF8_SEGMENT_MIN_BYTES
        && bytes.iter().filter(|byte| !byte.is_ascii()).count() >= UTF8_SEGMENT_MIN_NON_ASCII_BYTES
        && std::str::from_utf8(bytes).is_ok()
}

fn contains_iso_2022_escape(bytes: &[u8]) -> bool {
    bytes
        .windows(2)
        .any(|pair| pair[0] == 0x1B && matches!(pair[1], b'(' | b'$'))
}
