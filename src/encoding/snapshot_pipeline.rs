//! Decision-equivalent encoding validation over immutable snapshots.

use super::{
    BINARY_PROBE_BYTES, ByteSource, DECODE_CHUNK_BYTES, DetectedEncoding, EncodingDecision,
    EncodingKind, EncodingOrigin, EncodingRejection, FIXED_LEGACY_ENCODINGS, LEGACY_EVIDENCE_BYTES,
    LEGACY_SEGMENT_MAX_BYTES, SourceReader, UTF8_SEGMENT_MIN_BYTES,
    UTF8_SEGMENT_MIN_NON_ASCII_BYTES, Utf8PrefixScanner, ValidationStats, bom_detected_encoding,
    explicit_detected_encoding, has_binary_nul, is_disallowed_legacy_character, is_legacy_encoding,
    validated,
};
use crate::binary::detect_binary_type;
use crate::file_snapshot::SealedSnapshot;
#[cfg(test)]
use crate::operation::TestStage;
use crate::operation::{WorkCheckpoint, WorkStop};
use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{DecoderResult, EncoderResult, Encoding, UTF_8};
use std::collections::VecDeque;
use std::io::{self, Read};

const SEGMENT_CACHE_CAPACITY: usize = 256;
const SEGMENT_COMPACT_THRESHOLD: usize = 16 * 1024;

/// Failure channels that must remain distinct from an encoding rejection.
#[derive(Debug)]
pub(crate) enum EncodingPipelineFailure {
    Io(io::Error),
    Stopped(WorkStop),
}

impl From<io::Error> for EncodingPipelineFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Validates one immutable capture while honoring request cancellation and epoch retirement.
pub(crate) fn validate_snapshot_encoding(
    snapshot: &SealedSnapshot,
    explicit_encoding: Option<&str>,
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<EncodingDecision, EncodingPipelineFailure> {
    validate_source(ByteSource::Snapshot(snapshot), explicit_encoding, operation)
}

/// Runs the replacement pipeline for shared read/edit callers and test byte sources.
pub(super) fn validate_source(
    source: ByteSource<'_>,
    explicit_encoding: Option<&str>,
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<EncodingDecision, EncodingPipelineFailure> {
    checkpoint(operation)?;
    let result = match source {
        ByteSource::Bytes(bytes) => validate_with_prefix(
            source,
            &bytes[..bytes.len().min(BINARY_PROBE_BYTES)],
            explicit_encoding,
            operation,
        ),
        ByteSource::Snapshot(snapshot) => {
            if let Some(prefix) = snapshot.memory_prefix(BINARY_PROBE_BYTES) {
                validate_with_prefix(source, prefix, explicit_encoding, operation)
            } else {
                read_prefix(source, operation).and_then(|prefix| {
                    validate_with_prefix(source, &prefix, explicit_encoding, operation)
                })
            }
        }
        ByteSource::File(_) => read_prefix(source, operation)
            .and_then(|prefix| validate_with_prefix(source, &prefix, explicit_encoding, operation)),
    };
    prefer_stop(operation, result)
}

fn validate_with_prefix(
    source: ByteSource<'_>,
    prefix: &[u8],
    explicit_encoding: Option<&str>,
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<EncodingDecision, EncodingPipelineFailure> {
    let pipeline = EncodingPipeline {
        source,
        operation,
        chunk_bytes: DECODE_CHUNK_BYTES,
    };
    pipeline.validate(prefix, explicit_encoding)
}

fn read_prefix(
    source: ByteSource<'_>,
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<Vec<u8>, EncodingPipelineFailure> {
    let mut reader = SourceReader::open(source, 0)?;
    let mut prefix = Vec::with_capacity(BINARY_PROBE_BYTES);
    let mut buffer = [0_u8; BINARY_PROBE_BYTES];
    while prefix.len() < BINARY_PROBE_BYTES {
        encoding_chunk_checkpoint(operation)?;
        let count = reader.read(&mut buffer[..BINARY_PROBE_BYTES - prefix.len()])?;
        checkpoint(operation)?;
        if count == 0 {
            break;
        }
        prefix.extend_from_slice(&buffer[..count]);
    }
    Ok(prefix)
}

struct EncodingPipeline<'a> {
    source: ByteSource<'a>,
    operation: Option<&'a dyn WorkCheckpoint>,
    chunk_bytes: usize,
}

impl EncodingPipeline<'_> {
    fn validate(
        &self,
        prefix: &[u8],
        explicit_encoding: Option<&str>,
    ) -> Result<EncodingDecision, EncodingPipelineFailure> {
        checkpoint(self.operation)?;
        if let Some(value) = explicit_encoding {
            let detected = match explicit_detected_encoding(value, prefix) {
                Ok(detected) => detected,
                Err(rejection) => return Ok(EncodingDecision::Rejected(rejection)),
            };
            let options = PassOptions {
                enforce_legacy_hard_checks: false,
                scan_legacy_segments: None,
                probe_raw_utf8: is_legacy_encoding(detected.kind),
            };
            let result = self.validate_selected(&detected, options)?;
            return Ok(match result.stats {
                Some(stats) => {
                    EncodingDecision::Text(validated(detected, stats, result.raw_is_multibyte_utf8))
                }
                None => EncodingDecision::Rejected(EncodingRejection::ExplicitMalformed {
                    encoding: value.to_string(),
                }),
            });
        }

        if let Some(detected) = bom_detected_encoding(prefix) {
            if detected.kind == EncodingKind::EncodingRs(UTF_8) && has_binary_nul(prefix) {
                return Ok(EncodingDecision::Binary);
            }
            let result = self.validate_selected(&detected, PassOptions::plain())?;
            return Ok(match result.stats {
                Some(stats) => EncodingDecision::Text(validated(detected, stats, false)),
                None => EncodingDecision::Rejected(EncodingRejection::BomMismatch {
                    encoding: detected
                        .source_encoding
                        .or(match detected.origin {
                            EncodingOrigin::Bom(encoding) => Some(encoding),
                            _ => None,
                        })
                        .unwrap_or("UTF-8"),
                }),
            });
        }

        if has_binary_nul(prefix) {
            return Ok(EncodingDecision::Binary);
        }

        let utf8 = DetectedEncoding {
            kind: EncodingKind::EncodingRs(UTF_8),
            bom_len: 0,
            source_encoding: None,
            origin: EncodingOrigin::Automatic,
        };
        if let Some(stats) = self.validate_selected(&utf8, PassOptions::plain())?.stats {
            if stats.has_iso_2022_escape {
                return Ok(EncodingDecision::Rejected(
                    EncodingRejection::Iso2022JpSignature,
                ));
            }
            return Ok(EncodingDecision::Text(validated(utf8, stats, false)));
        }
        if detect_binary_type(prefix).is_some() {
            return Ok(EncodingDecision::Binary);
        }

        let nomination = self.scan_nomination()?;
        if let Some(conflict_hex_offset) = nomination.conflict_hex_offset {
            return Ok(EncodingDecision::Rejected(
                EncodingRejection::MixedOrInconsistent {
                    conflict_hex_offset: Some(conflict_hex_offset),
                },
            ));
        }

        let candidate = nomination.encoding;
        let candidate_detected = automatic_detected(candidate);
        let mut memo = ValidationMemo::default();
        if nomination.non_ascii_bytes >= LEGACY_EVIDENCE_BYTES {
            let result = self.validate_selected(
                &candidate_detected,
                PassOptions {
                    enforce_legacy_hard_checks: true,
                    scan_legacy_segments: Some(candidate),
                    probe_raw_utf8: false,
                },
            )?;
            memo.insert(candidate, result.stats.clone());
            if let Some(stats) = result.stats {
                if result.segments_inconsistent {
                    return Ok(EncodingDecision::Rejected(
                        EncodingRejection::MixedOrInconsistent {
                            conflict_hex_offset: None,
                        },
                    ));
                }
                if !candidate.is_single_byte() {
                    return Ok(EncodingDecision::Text(validated(
                        candidate_detected,
                        stats,
                        false,
                    )));
                }
            }
        }

        let mut candidates = Vec::new();
        for (label, encoding) in FIXED_LEGACY_ENCODINGS {
            let stats = match memo.get(encoding) {
                Some(cached) => {
                    candidate_checkpoint(self.operation)?;
                    cached
                }
                None => {
                    let detected = automatic_detected(encoding);
                    let result = self.validate_selected(
                        &detected,
                        PassOptions {
                            enforce_legacy_hard_checks: true,
                            scan_legacy_segments: None,
                            probe_raw_utf8: false,
                        },
                    )?;
                    memo.insert(encoding, result.stats.clone());
                    result.stats
                }
            };
            if stats.is_some() {
                candidates.push(label);
            }
        }
        Ok(if candidates.is_empty() {
            EncodingDecision::Rejected(EncodingRejection::Undecodable)
        } else {
            EncodingDecision::Rejected(EncodingRejection::Ambiguous { candidates })
        })
    }

    fn scan_nomination(&self) -> Result<Nomination, EncodingPipelineFailure> {
        let mut reader = SourceReader::open(self.source, 0)?;
        let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
        let mut conflict_scanner = Some(Utf8PrefixScanner::default());
        let mut conflict_hex_offset = None;
        let mut non_ascii_bytes = 0_usize;
        let mut input = [0_u8; DECODE_CHUNK_BYTES];
        loop {
            encoding_chunk_checkpoint(self.operation)?;
            let count = reader.read(&mut input[..self.chunk_bytes])?;
            checkpoint(self.operation)?;
            if count == 0 {
                detector.feed(&[], true);
                if conflict_hex_offset.is_none()
                    && let Some(scanner) = conflict_scanner.take()
                {
                    conflict_hex_offset = scanner.finish();
                }
                break;
            }
            let bytes = &input[..count];
            non_ascii_bytes = non_ascii_bytes
                .saturating_add(bytes.iter().filter(|byte| !byte.is_ascii()).count());
            detector.feed(bytes, false);
            if conflict_hex_offset.is_none()
                && let Some(scanner) = conflict_scanner.as_mut()
            {
                conflict_hex_offset = scanner.push(bytes);
            }
        }
        Ok(Nomination {
            encoding: detector.guess(None, Utf8Detection::Deny),
            non_ascii_bytes,
            conflict_hex_offset,
        })
    }

    fn validate_selected(
        &self,
        detected: &DetectedEncoding,
        options: PassOptions,
    ) -> Result<PassResult, EncodingPipelineFailure> {
        candidate_checkpoint(self.operation)?;
        if options.enforce_legacy_hard_checks
            && !matches!(detected.kind, EncodingKind::EncodingRs(_))
        {
            return Ok(PassResult::invalid());
        }

        let mut reader = SourceReader::open(self.source, 0)?;
        let mut decoder = StrictDecoderScratch::new(detected.kind);
        let mut raw_utf8 = options.probe_raw_utf8.then(RawUtf8Probe::new);
        let mut roundtrip = if options.enforce_legacy_hard_checks {
            let EncodingKind::EncodingRs(encoding) = detected.kind else {
                unreachable!("legacy hard checks reject non-encoding_rs kinds above");
            };
            Some(RoundTripVerifier::new(encoding))
        } else {
            None
        };
        let mut segments = options
            .scan_legacy_segments
            .map(LegacySegmentScannerV2::new);
        let mut segments_inconsistent = false;
        let mut stats = ValidationStats::default();
        let mut skip_bom = detected.bom_len as usize;
        let mut input = [0_u8; DECODE_CHUNK_BYTES];

        loop {
            encoding_chunk_checkpoint(self.operation)?;
            let count = reader.read(&mut input[..self.chunk_bytes])?;
            checkpoint(self.operation)?;
            let is_last = count == 0;
            let raw = &input[..count];
            if let Some(probe) = raw_utf8.as_mut() {
                probe.push(raw, is_last);
            }
            let skipped = skip_bom.min(raw.len());
            skip_bom -= skipped;
            let content = &raw[skipped..];

            if let Some(verifier) = roundtrip.as_mut() {
                verifier.observe_source(content);
            }
            let decoded = match decoder.push(content, is_last) {
                Ok(decoded) => decoded,
                Err(()) => return Ok(PassResult::invalid()),
            };
            if options.enforce_legacy_hard_checks
                && decoded.chars().any(is_disallowed_legacy_character)
            {
                return Ok(PassResult::invalid());
            }
            if let Some(verifier) = roundtrip.as_mut()
                && !verifier.push(decoded, false)
            {
                return Ok(PassResult::invalid());
            }
            stats.observe(decoded);

            if !segments_inconsistent && let Some(scanner) = segments.as_mut() {
                match scanner.push(content, self.operation)? {
                    SegmentScan::Continue => {}
                    SegmentScan::Inconsistent => segments_inconsistent = true,
                    SegmentScan::CandidateInvalid => return Ok(PassResult::invalid()),
                }
            }
            if is_last {
                break;
            }
        }

        if let Some(verifier) = roundtrip.as_mut()
            && !verifier.finish()
        {
            return Ok(PassResult::invalid());
        }
        if !segments_inconsistent && let Some(scanner) = segments.as_mut() {
            match scanner.finish(self.operation)? {
                SegmentScan::Continue => {}
                SegmentScan::Inconsistent => segments_inconsistent = true,
                SegmentScan::CandidateInvalid => return Ok(PassResult::invalid()),
            }
        }
        checkpoint(self.operation)?;
        Ok(PassResult {
            stats: Some(stats),
            raw_is_multibyte_utf8: raw_utf8.is_some_and(|probe| probe.is_multibyte_utf8()),
            segments_inconsistent,
            #[cfg(test)]
            roundtrip_allocations: roundtrip
                .as_ref()
                .map_or(0, RoundTripVerifier::allocation_count),
            #[cfg(test)]
            detector_constructions: segments
                .as_ref()
                .map_or(0, LegacySegmentScannerV2::detector_constructions),
        })
    }
}

fn automatic_detected(encoding: &'static Encoding) -> DetectedEncoding {
    DetectedEncoding {
        kind: EncodingKind::EncodingRs(encoding),
        bom_len: 0,
        source_encoding: Some(encoding.name()),
        origin: EncodingOrigin::Automatic,
    }
}

#[derive(Clone, Copy)]
struct PassOptions {
    enforce_legacy_hard_checks: bool,
    scan_legacy_segments: Option<&'static Encoding>,
    probe_raw_utf8: bool,
}

impl PassOptions {
    fn plain() -> Self {
        Self {
            enforce_legacy_hard_checks: false,
            scan_legacy_segments: None,
            probe_raw_utf8: false,
        }
    }
}

struct PassResult {
    stats: Option<ValidationStats>,
    raw_is_multibyte_utf8: bool,
    segments_inconsistent: bool,
    #[cfg(test)]
    roundtrip_allocations: usize,
    #[cfg(test)]
    detector_constructions: usize,
}

impl PassResult {
    fn invalid() -> Self {
        Self {
            stats: None,
            raw_is_multibyte_utf8: false,
            segments_inconsistent: false,
            #[cfg(test)]
            roundtrip_allocations: 0,
            #[cfg(test)]
            detector_constructions: 0,
        }
    }
}

struct Nomination {
    encoding: &'static Encoding,
    non_ascii_bytes: usize,
    conflict_hex_offset: Option<usize>,
}

#[derive(Default)]
struct ValidationMemo {
    entries: Vec<(&'static Encoding, Option<ValidationStats>)>,
}

impl ValidationMemo {
    fn get(&self, encoding: &'static Encoding) -> Option<Option<ValidationStats>> {
        self.entries
            .iter()
            .find(|(stored, _)| std::ptr::eq(*stored, encoding))
            .map(|(_, result)| result.clone())
    }

    fn insert(&mut self, encoding: &'static Encoding, result: Option<ValidationStats>) {
        if let Some((_, stored)) = self
            .entries
            .iter_mut()
            .find(|(stored, _)| std::ptr::eq(*stored, encoding))
        {
            *stored = result;
        } else {
            self.entries.push((encoding, result));
        }
    }
}

fn checkpoint(operation: Option<&dyn WorkCheckpoint>) -> Result<(), EncodingPipelineFailure> {
    match operation.map(WorkCheckpoint::check_work) {
        Some(Err(stop)) => Err(EncodingPipelineFailure::Stopped(stop)),
        Some(Ok(())) | None => Ok(()),
    }
}

fn prefer_stop<T>(
    operation: Option<&dyn WorkCheckpoint>,
    result: Result<T, EncodingPipelineFailure>,
) -> Result<T, EncodingPipelineFailure> {
    checkpoint(operation)?;
    result
}

fn encoding_chunk_checkpoint(
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<(), EncodingPipelineFailure> {
    checkpoint(operation)?;
    #[cfg(test)]
    if let Some(operation) = operation {
        operation.stage(TestStage::EncodingChunk);
    }
    checkpoint(operation)
}

fn candidate_checkpoint(
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<(), EncodingPipelineFailure> {
    checkpoint(operation)?;
    #[cfg(test)]
    if let Some(operation) = operation {
        operation.stage(TestStage::CandidateValidation);
    }
    checkpoint(operation)
}

fn segment_checkpoint(
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<(), EncodingPipelineFailure> {
    checkpoint(operation)?;
    #[cfg(test)]
    if let Some(operation) = operation {
        operation.stage(TestStage::LegacySegment);
    }
    checkpoint(operation)
}

struct RawUtf8Probe {
    decoder: StrictDecoderScratch,
    valid: bool,
    has_non_ascii: bool,
}

impl RawUtf8Probe {
    fn new() -> Self {
        Self {
            decoder: StrictDecoderScratch::new(EncodingKind::EncodingRs(UTF_8)),
            valid: true,
            has_non_ascii: false,
        }
    }

    fn push(&mut self, input: &[u8], is_last: bool) {
        if !self.valid {
            return;
        }
        match self.decoder.push(input, is_last) {
            Ok(decoded) => self.has_non_ascii |= !decoded.is_ascii(),
            Err(()) => self.valid = false,
        }
    }

    fn is_multibyte_utf8(&self) -> bool {
        self.valid && self.has_non_ascii
    }
}

struct StrictDecoderScratch {
    decoder: StrictDecoderKind,
    output: String,
}

enum StrictDecoderKind {
    Utf8 {
        carry: Vec<u8>,
        joined: Vec<u8>,
    },
    EncodingRs(encoding_rs::Decoder),
    Utf32 {
        little_endian: bool,
        carry: Vec<u8>,
        joined: Vec<u8>,
    },
}

impl StrictDecoderScratch {
    fn new(kind: EncodingKind) -> Self {
        let decoder = match kind {
            EncodingKind::EncodingRs(encoding) if encoding == UTF_8 => StrictDecoderKind::Utf8 {
                carry: Vec::with_capacity(4),
                joined: Vec::with_capacity(DECODE_CHUNK_BYTES + 4),
            },
            EncodingKind::EncodingRs(encoding) => {
                StrictDecoderKind::EncodingRs(encoding.new_decoder_without_bom_handling())
            }
            EncodingKind::Utf32Le => StrictDecoderKind::Utf32 {
                little_endian: true,
                carry: Vec::with_capacity(4),
                joined: Vec::with_capacity(DECODE_CHUNK_BYTES + 4),
            },
            EncodingKind::Utf32Be => StrictDecoderKind::Utf32 {
                little_endian: false,
                carry: Vec::with_capacity(4),
                joined: Vec::with_capacity(DECODE_CHUNK_BYTES + 4),
            },
        };
        Self {
            decoder,
            output: String::with_capacity(DECODE_CHUNK_BYTES),
        }
    }

    fn push(&mut self, input: &[u8], is_last: bool) -> Result<&str, ()> {
        self.output.clear();
        match &mut self.decoder {
            StrictDecoderKind::Utf8 { carry, joined } => {
                joined.clear();
                joined.extend_from_slice(carry);
                carry.clear();
                joined.extend_from_slice(input);
                match std::str::from_utf8(joined) {
                    Ok(text) => self.output.push_str(text),
                    Err(error) if error.error_len().is_none() && !is_last => {
                        let valid_up_to = error.valid_up_to();
                        self.output
                            .push_str(std::str::from_utf8(&joined[..valid_up_to]).map_err(|_| ())?);
                        carry.extend_from_slice(&joined[valid_up_to..]);
                    }
                    Err(_) => return Err(()),
                }
            }
            StrictDecoderKind::EncodingRs(decoder) => {
                let mut consumed = 0_usize;
                loop {
                    let remaining = &input[consumed..];
                    let capacity = decoder
                        .max_utf8_buffer_length_without_replacement(remaining.len())
                        .ok_or(())?
                        .max(4);
                    self.output.reserve(capacity);
                    let (result, read) = decoder.decode_to_string_without_replacement(
                        remaining,
                        &mut self.output,
                        is_last,
                    );
                    consumed = consumed.saturating_add(read);
                    match result {
                        DecoderResult::InputEmpty => break,
                        DecoderResult::OutputFull => continue,
                        DecoderResult::Malformed(_, _) => return Err(()),
                    }
                }
            }
            StrictDecoderKind::Utf32 {
                little_endian,
                carry,
                joined,
            } => {
                joined.clear();
                joined.extend_from_slice(carry);
                carry.clear();
                joined.extend_from_slice(input);
                if is_last && !joined.len().is_multiple_of(4) {
                    return Err(());
                }
                let complete_len = joined.len() / 4 * 4;
                if !is_last {
                    carry.extend_from_slice(&joined[complete_len..]);
                }
                self.output.reserve(complete_len);
                for raw in joined[..complete_len].chunks_exact(4) {
                    let unit = if *little_endian {
                        u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])
                    } else {
                        u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])
                    };
                    self.output.push(char::from_u32(unit).ok_or(())?);
                }
            }
        }
        Ok(&self.output)
    }
}

/// Re-encodes every decoded byte using reusable buffers and compares the entire source.
struct RoundTripVerifier {
    encoder: encoding_rs::Encoder,
    encoded: Vec<u8>,
    expected: Vec<u8>,
    encoded_start: usize,
    expected_start: usize,
    source_cursor: u64,
    compared_cursor: u64,
    #[cfg(test)]
    allocations: usize,
}

impl RoundTripVerifier {
    fn new(encoding: &'static Encoding) -> Self {
        Self {
            encoder: encoding.new_encoder(),
            encoded: Vec::new(),
            expected: Vec::new(),
            encoded_start: 0,
            expected_start: 0,
            source_cursor: 0,
            compared_cursor: 0,
            #[cfg(test)]
            allocations: 0,
        }
    }

    fn observe_source(&mut self, bytes: &[u8]) {
        self.compact_expected();
        self.reserve_expected(bytes.len());
        self.expected.extend_from_slice(bytes);
        self.source_cursor = self.source_cursor.saturating_add(bytes.len() as u64);
    }

    fn push(&mut self, mut text: &str, last: bool) -> bool {
        self.compact_encoded();
        let mut first = true;
        while first || !text.is_empty() {
            first = false;
            let capacity = self
                .encoder
                .max_buffer_length_from_utf8_without_replacement(text.len())
                .unwrap_or(text.len().saturating_mul(4).saturating_add(16))
                .saturating_add(16)
                .max(16);
            self.reserve_encoded(capacity);
            let (result, read) = self.encoder.encode_from_utf8_to_vec_without_replacement(
                text,
                &mut self.encoded,
                last,
            );
            if !self.compare_available() {
                return false;
            }
            text = &text[read..];
            match result {
                EncoderResult::InputEmpty => return text.is_empty(),
                EncoderResult::OutputFull => continue,
                EncoderResult::Unmappable(_) => return false,
            }
        }
        true
    }

    fn finish(&mut self) -> bool {
        self.push("", true)
            && self.compare_available()
            && self.encoded_start == self.encoded.len()
            && self.expected_start == self.expected.len()
            && self.compared_cursor == self.source_cursor
    }

    fn compare_available(&mut self) -> bool {
        let encoded = &self.encoded[self.encoded_start..];
        let expected = &self.expected[self.expected_start..];
        let count = encoded.len().min(expected.len());
        if encoded[..count] != expected[..count] {
            return false;
        }
        self.encoded_start += count;
        self.expected_start += count;
        self.compared_cursor = self.compared_cursor.saturating_add(count as u64);
        true
    }

    fn compact_encoded(&mut self) {
        compact_buffer(&mut self.encoded, &mut self.encoded_start);
    }

    fn compact_expected(&mut self) {
        compact_buffer(&mut self.expected, &mut self.expected_start);
    }

    fn reserve_encoded(&mut self, additional: usize) {
        #[cfg(test)]
        let before = self.encoded.capacity();
        self.encoded.reserve(additional);
        #[cfg(test)]
        if self.encoded.capacity() != before {
            self.allocations = self.allocations.saturating_add(1);
        }
    }

    fn reserve_expected(&mut self, additional: usize) {
        #[cfg(test)]
        let before = self.expected.capacity();
        self.expected.reserve(additional);
        #[cfg(test)]
        if self.expected.capacity() != before {
            self.allocations = self.allocations.saturating_add(1);
        }
    }

    #[cfg(test)]
    fn allocation_count(&self) -> usize {
        self.allocations
    }
}

fn compact_buffer(buffer: &mut Vec<u8>, start: &mut usize) {
    if *start == buffer.len() {
        buffer.clear();
        *start = 0;
    } else if *start >= DECODE_CHUNK_BYTES && *start >= buffer.len() / 2 {
        buffer.copy_within(*start.., 0);
        buffer.truncate(buffer.len() - *start);
        *start = 0;
    }
}

struct LegacySegmentScannerV2 {
    whole_encoding: &'static Encoding,
    pending: Vec<u8>,
    start: usize,
    evidence: SegmentEvidence,
    cache: SegmentCache,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SegmentScan {
    Continue,
    Inconsistent,
    CandidateInvalid,
}

impl LegacySegmentScannerV2 {
    fn new(whole_encoding: &'static Encoding) -> Self {
        Self {
            whole_encoding,
            pending: Vec::with_capacity(LEGACY_SEGMENT_MAX_BYTES * 2),
            start: 0,
            evidence: SegmentEvidence::default(),
            cache: SegmentCache::new(),
        }
    }

    fn push(
        &mut self,
        input: &[u8],
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<SegmentScan, EncodingPipelineFailure> {
        for byte in input {
            self.pending.push(*byte);
            self.evidence.push(*byte);
            if *byte == b'\n' && self.evidence.has_evidence() {
                segment_checkpoint(operation)?;
                let result = self.inspect_pending();
                if result != SegmentScan::Continue {
                    return Ok(result);
                }
                self.clear_pending();
                checkpoint(operation)?;
            } else if self.pending_len() >= LEGACY_SEGMENT_MAX_BYTES {
                segment_checkpoint(operation)?;
                let Some(end) = self.aligned_prefix_len() else {
                    return Ok(SegmentScan::CandidateInvalid);
                };
                let absolute_end = self.start + end;
                let segment = &self.pending[self.start..absolute_end];
                let strong = SegmentEvidence::from_bytes(segment).is_strong_utf8();
                if self
                    .cache
                    .segment_disagrees(self.whole_encoding, segment, strong)
                {
                    return Ok(SegmentScan::Inconsistent);
                }
                self.start = absolute_end;
                self.rebuild_evidence();
                self.compact_pending();
                checkpoint(operation)?;
            }
        }
        Ok(SegmentScan::Continue)
    }

    fn finish(
        &mut self,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<SegmentScan, EncodingPipelineFailure> {
        segment_checkpoint(operation)?;
        let result = self.inspect_pending();
        checkpoint(operation)?;
        Ok(result)
    }

    fn inspect_pending(&mut self) -> SegmentScan {
        if !self.evidence.has_evidence() {
            return SegmentScan::Continue;
        }
        let segment = &self.pending[self.start..];
        if !self.cache.hard_validate(self.whole_encoding, segment) {
            return SegmentScan::CandidateInvalid;
        }
        if self.cache.segment_disagrees(
            self.whole_encoding,
            segment,
            self.evidence.is_strong_utf8(),
        ) {
            SegmentScan::Inconsistent
        } else {
            SegmentScan::Continue
        }
    }

    fn aligned_prefix_len(&mut self) -> Option<usize> {
        let pending_len = self.pending_len();
        for trim in 0..=4 {
            let Some(end) = pending_len.checked_sub(trim) else {
                continue;
            };
            if self.cache.hard_validate(
                self.whole_encoding,
                &self.pending[self.start..self.start + end],
            ) {
                return Some(end);
            }
        }
        None
    }

    fn pending_len(&self) -> usize {
        self.pending.len() - self.start
    }

    fn clear_pending(&mut self) {
        self.pending.clear();
        self.start = 0;
        self.evidence = SegmentEvidence::default();
    }

    fn rebuild_evidence(&mut self) {
        self.evidence = SegmentEvidence::from_bytes(&self.pending[self.start..]);
    }

    fn compact_pending(&mut self) {
        if self.start >= SEGMENT_COMPACT_THRESHOLD
            || (self.start > 0 && self.start >= self.pending.len() / 2)
        {
            self.pending.copy_within(self.start.., 0);
            self.pending.truncate(self.pending.len() - self.start);
            self.start = 0;
        }
    }

    #[cfg(test)]
    fn detector_constructions(&self) -> usize {
        self.cache.detector_constructions
    }
}

#[derive(Default)]
struct SegmentEvidence {
    len: usize,
    non_ascii: usize,
    utf8: Utf8Evidence,
}

impl SegmentEvidence {
    fn from_bytes(bytes: &[u8]) -> Self {
        let mut evidence = Self::default();
        for byte in bytes {
            evidence.push(*byte);
        }
        evidence
    }

    fn push(&mut self, byte: u8) {
        self.len = self.len.saturating_add(1);
        self.non_ascii = self.non_ascii.saturating_add(usize::from(!byte.is_ascii()));
        self.utf8.push(byte);
    }

    fn has_evidence(&self) -> bool {
        self.non_ascii >= LEGACY_EVIDENCE_BYTES || self.is_strong_utf8()
    }

    fn is_strong_utf8(&self) -> bool {
        self.len >= UTF8_SEGMENT_MIN_BYTES
            && self.non_ascii >= UTF8_SEGMENT_MIN_NON_ASCII_BYTES
            && self.utf8.is_complete_and_valid()
    }
}

#[derive(Default)]
struct Utf8Evidence {
    valid: bool,
    initialized: bool,
    remaining: u8,
    next_min: u8,
    next_max: u8,
}

impl Utf8Evidence {
    fn push(&mut self, byte: u8) {
        if !self.initialized {
            self.initialized = true;
            self.valid = true;
        }
        if !self.valid {
            return;
        }
        if self.remaining > 0 {
            if !(self.next_min..=self.next_max).contains(&byte) {
                self.valid = false;
                return;
            }
            self.remaining -= 1;
            self.next_min = 0x80;
            self.next_max = 0xBF;
            return;
        }
        match byte {
            0x00..=0x7F => {}
            0xC2..=0xDF => self.begin(1, 0x80, 0xBF),
            0xE0 => self.begin(2, 0xA0, 0xBF),
            0xE1..=0xEC | 0xEE..=0xEF => self.begin(2, 0x80, 0xBF),
            0xED => self.begin(2, 0x80, 0x9F),
            0xF0 => self.begin(3, 0x90, 0xBF),
            0xF1..=0xF3 => self.begin(3, 0x80, 0xBF),
            0xF4 => self.begin(3, 0x80, 0x8F),
            _ => self.valid = false,
        }
    }

    fn begin(&mut self, remaining: u8, next_min: u8, next_max: u8) {
        self.remaining = remaining;
        self.next_min = next_min;
        self.next_max = next_max;
    }

    fn is_complete_and_valid(&self) -> bool {
        (!self.initialized || self.valid) && self.remaining == 0
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct SegmentKey {
    high: u64,
    low: u64,
    len: usize,
}

struct SegmentCacheEntry {
    key: SegmentKey,
    bytes: Box<[u8]>,
    detector_guess: Option<&'static Encoding>,
    validations: Vec<(&'static Encoding, bool)>,
}

struct SegmentCache {
    entries: VecDeque<SegmentCacheEntry>,
    scratch: SegmentValidationScratch,
    #[cfg(test)]
    detector_constructions: usize,
}

impl SegmentCache {
    fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(SEGMENT_CACHE_CAPACITY),
            scratch: SegmentValidationScratch::default(),
            #[cfg(test)]
            detector_constructions: 0,
        }
    }

    fn hard_validate(&mut self, encoding: &'static Encoding, bytes: &[u8]) -> bool {
        let mut entry = self.take_entry(bytes);
        let result = if let Some((_, result)) = entry
            .validations
            .iter()
            .find(|(stored, _)| std::ptr::eq(*stored, encoding))
        {
            *result
        } else {
            let result = self.scratch.hard_validate(encoding, bytes);
            entry.validations.push((encoding, result));
            result
        };
        self.restore_entry(entry);
        result
    }

    fn segment_disagrees(
        &mut self,
        whole_encoding: &'static Encoding,
        bytes: &[u8],
        strong_utf8: bool,
    ) -> bool {
        if strong_utf8 {
            return true;
        }
        let mut entry = self.take_entry(bytes);
        let candidate = match entry.detector_guess {
            Some(candidate) => candidate,
            None => {
                let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
                detector.feed(bytes, true);
                let candidate = detector.guess(None, Utf8Detection::Deny);
                entry.detector_guess = Some(candidate);
                #[cfg(test)]
                {
                    self.detector_constructions = self.detector_constructions.saturating_add(1);
                }
                candidate
            }
        };
        let result = if std::ptr::eq(candidate, whole_encoding) || candidate.is_single_byte() {
            false
        } else if let Some((_, result)) = entry
            .validations
            .iter()
            .find(|(stored, _)| std::ptr::eq(*stored, candidate))
        {
            *result
        } else {
            let result = self.scratch.hard_validate(candidate, bytes);
            entry.validations.push((candidate, result));
            result
        };
        self.restore_entry(entry);
        result
    }

    fn take_entry(&mut self, bytes: &[u8]) -> SegmentCacheEntry {
        let key = segment_key(bytes);
        if let Some(position) = self
            .entries
            .iter()
            .position(|entry| entry.key == key && entry.bytes.as_ref() == bytes)
        {
            return self
                .entries
                .remove(position)
                .expect("located segment cache entry must remain present");
        }
        SegmentCacheEntry {
            key,
            bytes: bytes.into(),
            detector_guess: None,
            validations: Vec::new(),
        }
    }

    fn restore_entry(&mut self, entry: SegmentCacheEntry) {
        self.entries.push_back(entry);
        if self.entries.len() > SEGMENT_CACHE_CAPACITY {
            self.entries.pop_front();
        }
    }
}

fn segment_key(bytes: &[u8]) -> SegmentKey {
    let mut high = 0xCBF2_9CE4_8422_2325_u64;
    let mut low = 0x9E37_79B9_7F4A_7C15_u64;
    for byte in bytes {
        high ^= u64::from(*byte);
        high = high.wrapping_mul(0x0000_0100_0000_01B3);
        low ^= u64::from(*byte).wrapping_add(high.rotate_left(17));
        low = low.rotate_left(13).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    }
    SegmentKey {
        high,
        low,
        len: bytes.len(),
    }
}

#[derive(Default)]
struct SegmentValidationScratch {
    decoded: String,
    encoded: Vec<u8>,
}

impl SegmentValidationScratch {
    fn hard_validate(&mut self, encoding: &'static Encoding, bytes: &[u8]) -> bool {
        self.decoded.clear();
        let mut decoder = encoding.new_decoder_without_bom_handling();
        let mut consumed = 0_usize;
        loop {
            let remaining = &bytes[consumed..];
            let Some(capacity) =
                decoder.max_utf8_buffer_length_without_replacement(remaining.len())
            else {
                return false;
            };
            self.decoded.reserve(capacity.max(4));
            let (result, read) =
                decoder.decode_to_string_without_replacement(remaining, &mut self.decoded, true);
            consumed = consumed.saturating_add(read);
            match result {
                DecoderResult::InputEmpty => break,
                DecoderResult::OutputFull => continue,
                DecoderResult::Malformed(_, _) => return false,
            }
        }
        if self.decoded.chars().any(is_disallowed_legacy_character) {
            return false;
        }

        self.encoded.clear();
        let mut encoder = encoding.new_encoder();
        let mut remaining = self.decoded.as_str();
        let mut first = true;
        while first || !remaining.is_empty() {
            first = false;
            let capacity = encoder
                .max_buffer_length_from_utf8_without_replacement(remaining.len())
                .unwrap_or(remaining.len().saturating_mul(4).saturating_add(16))
                .saturating_add(16)
                .max(16);
            self.encoded.reserve(capacity);
            let (result, read) = encoder.encode_from_utf8_to_vec_without_replacement(
                remaining,
                &mut self.encoded,
                true,
            );
            remaining = &remaining[read..];
            match result {
                EncoderResult::InputEmpty => break,
                EncoderResult::OutputFull => continue,
                EncoderResult::Unmappable(_) => return false,
            }
        }
        remaining.is_empty() && self.encoded == bytes
    }
}

#[cfg(test)]
pub(super) fn validate_bytes_for_test(
    bytes: &[u8],
    explicit_encoding: Option<&str>,
) -> EncodingDecision {
    validate_source(ByteSource::Bytes(bytes), explicit_encoding, None)
        .unwrap_or_else(|_| panic!("in-memory validation cannot fail with I/O"))
}

#[cfg(test)]
pub(super) fn validate_bytes_with_chunk_limit_for_test(
    bytes: &[u8],
    explicit_encoding: Option<&str>,
    chunk_bytes: usize,
) -> EncodingDecision {
    assert!((1..=DECODE_CHUNK_BYTES).contains(&chunk_bytes));
    let pipeline = EncodingPipeline {
        source: ByteSource::Bytes(bytes),
        operation: None,
        chunk_bytes,
    };
    pipeline
        .validate(
            &bytes[..bytes.len().min(BINARY_PROBE_BYTES)],
            explicit_encoding,
        )
        .unwrap_or_else(|_| panic!("in-memory validation cannot fail with I/O"))
}

#[cfg(test)]
pub(super) fn legacy_metrics_for_test(
    bytes: &[u8],
    encoding: &'static Encoding,
) -> (Option<ValidationStats>, bool, usize, usize) {
    let pipeline = EncodingPipeline {
        source: ByteSource::Bytes(bytes),
        operation: None,
        chunk_bytes: DECODE_CHUNK_BYTES,
    };
    let detected = automatic_detected(encoding);
    let result = pipeline
        .validate_selected(
            &detected,
            PassOptions {
                enforce_legacy_hard_checks: true,
                scan_legacy_segments: Some(encoding),
                probe_raw_utf8: false,
            },
        )
        .unwrap_or_else(|_| panic!("in-memory validation cannot fail with I/O"));
    (
        result.stats,
        result.segments_inconsistent,
        result.roundtrip_allocations,
        result.detector_constructions,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ByteSource, EncodingDecision, EncodingKind, EncodingOrigin, EncodingPipelineFailure,
        EncodingRejection, LegacySegmentScannerV2, RoundTripVerifier, SegmentCache,
        SegmentCacheEntry, SegmentEvidence, SegmentScan, legacy_metrics_for_test, segment_key,
        validate_bytes_for_test, validate_bytes_with_chunk_limit_for_test, validate_source,
    };
    use crate::encoding::reference_v011::{
        ReferenceDecision, ReferenceOrigin, ReferenceRejection, classify_v011,
    };
    use crate::operation::{EpochGuard, RequestWorkGuard, TestStage, WorkCtx, WorkStop};
    use encoding_rs::GBK;
    use rmcp::model::RequestId;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio_util::sync::CancellationToken;

    fn normalize_production(decision: &EncodingDecision) -> ReferenceDecision {
        match decision {
            EncodingDecision::Text(validated) => ReferenceDecision::Text {
                kind: match validated.detected.kind {
                    EncodingKind::EncodingRs(encoding) => encoding.name().to_string(),
                    EncodingKind::Utf32Le => "UTF-32LE".to_string(),
                    EncodingKind::Utf32Be => "UTF-32BE".to_string(),
                },
                bom_len: validated.detected.bom_len as usize,
                source_encoding: validated.detected.source_encoding.map(str::to_string),
                origin: match &validated.detected.origin {
                    EncodingOrigin::Explicit(value) => ReferenceOrigin::Explicit(value.clone()),
                    EncodingOrigin::Bom(value) => ReferenceOrigin::Bom((*value).to_string()),
                    EncodingOrigin::Automatic => ReferenceOrigin::Automatic,
                },
                total_lines: validated.total_lines,
                has_trailing_newline: validated.has_trailing_newline,
                explicit_utf8_warning: validated.explicit_utf8_warning,
                note: validated.transcoding_note(),
            },
            EncodingDecision::Binary => ReferenceDecision::Binary,
            EncodingDecision::Rejected(rejection) => {
                ReferenceDecision::Rejected(normalize_rejection(rejection))
            }
        }
    }

    fn normalize_rejection(rejection: &EncodingRejection) -> ReferenceRejection {
        match rejection {
            EncodingRejection::Ambiguous { candidates } => ReferenceRejection::Ambiguous(
                candidates
                    .iter()
                    .map(|candidate| (*candidate).to_string())
                    .collect(),
            ),
            EncodingRejection::MixedOrInconsistent {
                conflict_hex_offset,
            } => ReferenceRejection::MixedOrInconsistent(*conflict_hex_offset),
            EncodingRejection::Iso2022JpSignature => ReferenceRejection::Iso2022JpSignature,
            EncodingRejection::Undecodable => ReferenceRejection::Undecodable,
            EncodingRejection::BomMismatch { encoding } => {
                ReferenceRejection::BomMismatch((*encoding).to_string())
            }
            EncodingRejection::ExplicitMalformed { encoding } => {
                ReferenceRejection::ExplicitMalformed(encoding.clone())
            }
            EncodingRejection::InvalidLabel { value } => {
                ReferenceRejection::InvalidLabel(value.clone())
            }
        }
    }

    #[test]
    fn utf8_evidence_matches_the_standard_library_for_every_prefix() {
        let mut state = 0xC001_D00D_A5A5_5A5A_u64;
        for length in 0..=512 {
            let mut bytes = Vec::with_capacity(length);
            for _ in 0..length {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                bytes.push((state >> 32) as u8);
                let evidence = SegmentEvidence::from_bytes(&bytes);
                assert_eq!(
                    evidence.utf8.is_complete_and_valid(),
                    std::str::from_utf8(&bytes).is_ok(),
                    "length {}",
                    bytes.len()
                );
            }
        }
    }

    #[test]
    fn repeated_segments_reuse_detector_and_roundtrip_scratch() {
        let mut line = Vec::new();
        for _ in 0..40 {
            line.extend_from_slice(&[0xD6, 0xD0]);
        }
        line.push(b'\n');
        let bytes = line.repeat(4_096);
        let (stats, inconsistent, allocations, detector_constructions) =
            legacy_metrics_for_test(&bytes, GBK);
        assert!(stats.is_some());
        assert!(!inconsistent);
        assert!(
            allocations <= 8,
            "round-trip buffers grew {allocations} times for {} chunks",
            bytes.len().div_ceil(super::DECODE_CHUNK_BYTES)
        );
        assert_eq!(detector_constructions, 1);
    }

    #[test]
    fn unique_segments_construct_at_most_one_detector_each_and_reuse_scratch() {
        const UNIQUE_SEGMENTS: usize = 64;
        let mut segments = Vec::with_capacity(UNIQUE_SEGMENTS);
        for index in 0..UNIQUE_SEGMENTS {
            let mut segment = Vec::with_capacity(96);
            for _ in 0..40 {
                segment.extend_from_slice(&[0xD6, 0xD0]);
            }
            segment.extend_from_slice(format!("-{index:02x}").as_bytes());
            segment.push(b'\n');
            segments.push(segment);
        }
        let mut bytes = Vec::new();
        for _ in 0..2 {
            for segment in &segments {
                bytes.extend_from_slice(segment);
            }
        }

        let (stats, inconsistent, allocations, detector_constructions) =
            legacy_metrics_for_test(&bytes, GBK);
        assert!(stats.is_some());
        assert!(!inconsistent);
        assert!(
            allocations <= 8,
            "round-trip buffers grew {allocations} times"
        );
        assert_eq!(detector_constructions, UNIQUE_SEGMENTS);
    }

    #[test]
    fn invalid_unalignable_segment_cannot_grow_pending_past_the_contract_boundary() {
        let mut scanner = LegacySegmentScannerV2::new(GBK);
        let invalid = vec![0xFF; super::LEGACY_SEGMENT_MAX_BYTES * 8];
        let result = scanner.push(&invalid, None).unwrap();
        assert_eq!(result, SegmentScan::CandidateInvalid);
        assert!(scanner.pending_len() <= super::LEGACY_SEGMENT_MAX_BYTES);
    }

    #[test]
    fn segment_cache_compares_full_bytes_after_a_hash_key_match() {
        let requested = b"requested-segment";
        let colliding_bytes = b"different-segment";
        let mut cache = SegmentCache::new();
        cache.entries.push_back(SegmentCacheEntry {
            key: segment_key(requested),
            bytes: colliding_bytes.as_slice().into(),
            detector_guess: Some(GBK),
            validations: vec![(GBK, true)],
        });

        let entry = cache.take_entry(requested);
        assert_eq!(entry.bytes.as_ref(), requested);
        assert!(entry.detector_guess.is_none());
        assert!(entry.validations.is_empty());
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(
            cache.entries.front().unwrap().bytes.as_ref(),
            colliding_bytes
        );
    }

    #[test]
    fn roundtrip_verifier_compares_every_source_byte() {
        let mut exact = RoundTripVerifier::new(GBK);
        exact.observe_source(&[0xD6, 0xD0]);
        assert!(exact.push("中", false));
        assert!(exact.finish());

        let mut mutated = RoundTripVerifier::new(GBK);
        mutated.observe_source(&[0xD6, 0xD1]);
        assert!(!mutated.push("中", false) || !mutated.finish());
    }

    #[test]
    fn encoding_chunk_checkpoint_observes_request_cancellation() {
        let token = CancellationToken::new();
        let cancel_from_hook = token.clone();
        let chunks = Arc::new(AtomicU64::new(0));
        let chunks_from_hook = Arc::clone(&chunks);
        let hook = Arc::new(move |stage| {
            if stage == TestStage::EncodingChunk {
                let chunk = chunks_from_hook.fetch_add(1, Ordering::AcqRel) + 1;
                if chunk == 2 {
                    cancel_from_hook.cancel();
                }
            }
        });
        let (_guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(71), token, hook);
        let bytes = vec![b'a'; super::DECODE_CHUNK_BYTES * 2];
        let result = validate_source(ByteSource::Bytes(&bytes), None, Some(&operation));
        assert!(matches!(
            result,
            Err(EncodingPipelineFailure::Stopped(WorkStop::RequestCancelled))
        ));
        assert_eq!(chunks.load(Ordering::Acquire), 2);
    }

    #[test]
    fn legacy_segment_checkpoint_stops_without_inspecting_another_segment() {
        let token = CancellationToken::new();
        let cancel_from_hook = token.clone();
        let segments = Arc::new(AtomicU64::new(0));
        let segments_from_hook = Arc::clone(&segments);
        let hook = Arc::new(move |stage| {
            if stage == TestStage::LegacySegment {
                segments_from_hook.fetch_add(1, Ordering::AcqRel);
                cancel_from_hook.cancel();
            }
        });
        let (_guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(73), token, hook);
        let mut bytes = Vec::new();
        for _ in 0..64 {
            bytes.extend_from_slice(&[0xD6, 0xD0]);
        }
        bytes.push(b'\n');
        let result = validate_source(ByteSource::Bytes(&bytes), None, Some(&operation));
        assert!(matches!(
            result,
            Err(EncodingPipelineFailure::Stopped(WorkStop::RequestCancelled))
        ));
        assert_eq!(segments.load(Ordering::Acquire), 1);
    }

    #[test]
    fn candidate_checkpoint_observes_epoch_retirement() {
        let generation = Arc::new(AtomicU64::new(0));
        let generation_from_hook = Arc::clone(&generation);
        let hook = Arc::new(move |stage| {
            if stage == TestStage::CandidateValidation {
                generation_from_hook.store(1, Ordering::Release);
            }
        });
        let (mut guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(72), CancellationToken::new(), hook);
        let work = WorkCtx::speculative(operation, EpochGuard::new(0, generation));
        let result = validate_source(ByteSource::Bytes(b"plain text"), None, Some(&work));
        guard.disarm();
        assert!(matches!(
            result,
            Err(EncodingPipelineFailure::Stopped(WorkStop::EpochRetired))
        ));
    }

    fn reference_corpus() -> Vec<(Vec<u8>, Option<&'static str>)> {
        let mut corpus: Vec<(Vec<u8>, Option<&str>)> = vec![
            (Vec::new(), None),
            (b"plain ASCII\r\nsecond\n".to_vec(), None),
            (b"\xEF\xBB\xBFutf8 bom\n".to_vec(), None),
            (b"\xEF\xBB\xBFbad\0tail".to_vec(), None),
            (b"\xEF\xBB\xBFbad\xFFtail".to_vec(), None),
            (b"\xFF\xFEA\0B\0".to_vec(), None),
            (b"\xFE\xFF\0A\0B".to_vec(), None),
            (vec![0xFF, 0xFE, b'A'], None),
            (vec![0xFF, 0xFE, 0, 0, b'A', 0, 0, 0], None),
            (vec![0, 0, 0xFE, 0xFF, 0, 0, 0, b'A'], None),
            (vec![0, 0, 0xFE, 0xFF, 0], None),
            (b"\x1B$Bstateful-ascii".to_vec(), None),
            (b"invalid\xFFtail".to_vec(), Some("utf-8")),
            (b"plain UTF-8 \xE7\x95\x8C".to_vec(), Some("gbk")),
            (b"A\0B\0".to_vec(), Some("utf-16le")),
            (b"\0A\0B".to_vec(), Some("utf-16be")),
            (vec![b'A', 0, 0, 0, b'B', 0, 0, 0], Some("utf-32le")),
            (vec![0, 0, 0, b'A', 0, 0, 0, b'B'], Some("utf-32be")),
            (b"\xFF\xFEA\0".to_vec(), Some("utf-16le")),
            (b"\xFE\xFF\0A".to_vec(), Some("utf-16be")),
            (vec![0xFF, 0xFE, 0, 0, b'A', 0, 0, 0], Some("utf-32le")),
            (vec![0, 0, 0xFE, 0xFF, 0, 0, 0, b'A'], Some("utf-32be")),
            (b"\x1B$B$\"\x1B(B".to_vec(), Some("iso-2022-jp")),
            (b"plain".to_vec(), Some("not-a-label")),
        ];

        let mut gbk = Vec::new();
        let mut shift_jis = Vec::new();
        let mut big5 = Vec::new();
        let mut euc_kr = Vec::new();
        for _ in 0..96 {
            gbk.extend_from_slice(&[0xD6, 0xD0]);
            shift_jis.extend_from_slice(&[0x93, 0xFA]);
            big5.extend_from_slice(&[0xA4, 0xA4]);
            euc_kr.extend_from_slice(&[0xC7, 0xD1]);
        }
        for bytes in [&gbk, &shift_jis, &big5, &euc_kr] {
            corpus.push((bytes.clone(), None));
        }
        corpus.extend([
            (gbk.clone(), Some("gbk")),
            (shift_jis.clone(), Some("shift_jis")),
            (big5.clone(), Some("big5")),
            (euc_kr.clone(), Some("euc-kr")),
            (vec![0xE9; 40], None),
            (vec![0xE9; 40], Some("windows-1252")),
        ]);

        let strong_utf8 = "界".repeat(11).into_bytes();
        for ascii_padding in [0, 31, 4_095, 65_535] {
            let mut mixed = strong_utf8.clone();
            mixed.extend(std::iter::repeat_n(b'a', ascii_padding));
            mixed.push(0xFF);
            corpus.push((mixed.clone(), None));
            mixed.push(b'\n');
            mixed.extend_from_slice(&gbk);
            corpus.push((mixed, None));
        }
        let mut cross_line = gbk.clone();
        cross_line.push(b'\n');
        cross_line.extend_from_slice(&shift_jis);
        corpus.push((cross_line, None));
        let mut same_line = gbk.clone();
        same_line.extend_from_slice(&shift_jis);
        corpus.push((same_line, None));

        for length in [31, 32, 33, 4_095, 4_096, 4_097] {
            let mut bytes = Vec::with_capacity(length + 64);
            while bytes.len() + 2 <= length {
                bytes.extend_from_slice(&[0xD6, 0xD0]);
            }
            bytes.resize(length, b'a');
            bytes.push(b'\n');
            corpus.push((bytes, None));
        }
        for non_ascii in [7, 8, 9, 31, 32, 33] {
            let mut bytes = b"ascii-prefix-which-is-long-enough-".to_vec();
            bytes.extend(std::iter::repeat_n(0xE9, non_ascii));
            bytes.push(b'\n');
            corpus.push((bytes, None));
        }

        let mut utf8_boundary = vec![b'a'; super::DECODE_CHUNK_BYTES - 1];
        utf8_boundary.extend_from_slice("界\r\n".as_bytes());
        corpus.push((utf8_boundary, None));
        let mut gbk_boundary = vec![b'a'; super::DECODE_CHUNK_BYTES - 1];
        gbk_boundary.extend_from_slice(&[0xD6, 0xD0, b'\r', b'\n']);
        gbk_boundary.extend_from_slice(&gbk);
        corpus.push((gbk_boundary, None));
        let mut escape_boundary = vec![b'a'; super::DECODE_CHUNK_BYTES - 1];
        escape_boundary.extend_from_slice(b"\x1B$Btail");
        corpus.push((escape_boundary, None));

        let mut state = 0xA5A5_5A5A_1234_5678_u64;
        for case in 0..384 {
            let length = case * 17 % 2_049;
            let mut bytes = Vec::with_capacity(length);
            for _ in 0..length {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                bytes.push((state >> 32) as u8);
            }
            corpus.push((bytes, None));
        }

        corpus
    }

    #[test]
    fn v011_reference_differential_covers_trust_tree_and_boundaries() {
        for (case, (bytes, explicit)) in reference_corpus().into_iter().enumerate() {
            let expected = classify_v011(&bytes, explicit);
            let actual = normalize_production(&validate_bytes_for_test(&bytes, explicit));
            assert_eq!(
                actual, expected,
                "differential case {case}, explicit={explicit:?}"
            );
        }
    }

    #[test]
    fn v011_reference_differential_is_invariant_to_reader_chunking() {
        for (case, (fixture, explicit)) in reference_corpus().into_iter().enumerate() {
            let expected = classify_v011(&fixture, explicit);
            for chunk_bytes in [1, 2, 3, 7, 31, 4_095, 65_535, 65_536] {
                let actual = normalize_production(&validate_bytes_with_chunk_limit_for_test(
                    &fixture,
                    explicit,
                    chunk_bytes,
                ));
                assert_eq!(
                    actual, expected,
                    "case={case}, explicit={explicit:?}, chunk_bytes={chunk_bytes}"
                );
            }
        }
    }
}
