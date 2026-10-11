//! A shared interpretation of original file bytes, including read-only lines.

use std::io::{Cursor, Read};
use std::ops::Range;

use super::{EncodingError, TextEncoding, evidence, iso2022, line, mapping::Mapping};
pub(crate) use line::{content, excerpt, visit};

/// A text range before a replacement and where it is after.
pub type Moved = (Range<usize>, Range<usize>);

pub(crate) struct Plan {
    pub(super) fixed: Option<TextEncoding>,
    pub(super) choices: Vec<TextEncoding>,
    pub(super) basis: &'static str,
    pub(super) ordered: bool,
    pub(super) legacy: Vec<TextEncoding>,
}

impl Plan {
    pub(crate) fn line_endings(&self) -> (Vec<u8>, Vec<u8>) {
        let codec = self.fixed.unwrap_or_else(TextEncoding::utf8);
        (codec.encode("\n").expect("newline"), codec.encode("\r\n").expect("CRLF"))
    }

    pub(crate) fn is_utf8(&self) -> bool {
        self.fixed.is_some_and(|codec| codec.label() == "UTF-8")
    }
    pub fn inspect(reader: &mut impl Read, selection: Option<&str>) -> Result<Self, String> {
        evidence::inspect(reader, selection)
    }

    pub(super) fn fixed(codec: TextEncoding, basis: &'static str) -> Self {
        Self { fixed: Some(codec), choices: Vec::new(), basis, ordered: false, legacy: Vec::new() }
    }

    pub fn signature(&self) -> Option<TextEncoding> {
        self.fixed.into_iter().chain(self.choices.iter().copied()).find(|codec| codec.bom_len() > 0)
    }
}

struct Segment {
    bytes: Range<usize>,
    text: Range<usize>,
    codec: Option<TextEncoding>,
    issue: Option<String>,
}

pub(crate) struct Source {
    pub raw: Vec<u8>,
    pub text: String,
    segments: Vec<Segment>,
    initial: Option<TextEncoding>,
    bom: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct Replacement {
    pub text: Range<usize>,
    pub content: String,
    pub context: Range<usize>,
}

impl Source {
    pub fn decode(raw: Vec<u8>, selection: Option<&str>) -> Result<Self, String> {
        let plan = Plan::inspect(&mut Cursor::new(&raw), selection)?;
        let bom = raw[..plan.signature().map_or(0, |codec| codec.bom_len())].to_vec();
        let mut text = String::new();
        let mut segments: Vec<Segment> = Vec::new();
        visit(Cursor::new(&raw), &plan, |line| {
            let start = text.len();
            text.push_str(&line.text.text().map_err(|e| e.to_string())?);
            if let Some(previous) = segments.last_mut()
                && previous.codec == line.codec
                && previous.issue == line.issue
            {
                previous.bytes.end = line.bytes.end;
                previous.text.end = text.len();
            } else {
                segments.push(Segment {
                    bytes: line.bytes,
                    text: start..text.len(),
                    codec: line.codec,
                    issue: line.issue,
                });
            }
            Ok(())
        })?;
        Ok(Self { raw, text, segments, initial: plan.fixed.or_else(|| plan.signature()), bom })
    }

    fn segment_at(&self, offset: usize, insertion: bool) -> usize {
        let index = self.segments.partition_point(|segment| segment.text.end <= offset);
        if index == self.segments.len() && insertion { index.saturating_sub(1) } else { index }
    }

    pub fn stateful_at(&self, offset: usize) -> bool {
        self.segments
            .iter()
            .find(|segment| segment.text.contains(&offset))
            .and_then(|segment| segment.codec)
            .is_some_and(TextEncoding::is_stateful)
    }

    pub fn issue(&self) -> Option<&str> {
        self.segments.iter().find_map(|segment| segment.issue.as_deref())
    }

    pub fn label(&self) -> Option<&'static str> {
        let mut codec = self.initial.map(TextEncoding::without_bom);
        for segment in &self.segments {
            let current = segment.codec?;
            if codec.is_some_and(|prior| prior != current) {
                return None;
            }
            codec = Some(current);
        }
        Some(codec.unwrap_or_else(TextEncoding::utf8).label())
    }

    pub fn whole(&self, text: &str) -> Result<Vec<u8>, String> {
        if let Some(segment) = self.segments.iter().find(|segment| segment.issue.is_some()) {
            return Err(format!(
                "write cannot replace this file, since one of its lines cannot be read: {}",
                segment.issue.as_deref().expect("issue")
            ));
        }
        let Some(label) = self.label() else {
            let boundary = self
                .segments
                .windows(2)
                .find(|pair| pair[0].codec != pair[1].codec)
                .map_or(self.bom.len(), |pair| pair[1].bytes.start);
            return Err(format!(
                "write cannot assign one original encoding to this mixed-encoding file at byte {boundary} (hex {}); use separate edit blocks within its encoding segments, or explicitly convert the file before writing it",
                hex(&self.raw[boundary..self.raw.len().min(boundary + 1)])
            ));
        };
        let codec = super::explicit(label, &[]).map_err(|e| e.to_string())?;
        let text = crate::text::newlines(text, crate::text::dominant_ending(&self.text));
        let encoded =
            codec.encode(&text).map_err(|error| describe_error(&error, &self.raw[self.bom.len()..], self.bom.len()))?;
        let mut bytes = self.bom.clone();
        bytes.extend(encoded);
        Ok(bytes)
    }

    /// Returns the new text ranges. Every unaffected original byte is copied.
    pub fn replace(&mut self, replacements: &[Replacement]) -> Result<Vec<Moved>, String> {
        if replacements.is_empty() {
            return Ok(Vec::new());
        }
        let mut boundaries = std::collections::BTreeMap::<usize, Vec<usize>>::new();
        for replacement in replacements {
            let index = self.segment_at(replacement.text.start, replacement.text.is_empty());
            if let Some(segment) = self.segments.get(index) {
                let wanted = boundaries.entry(index).or_default();
                wanted.push(replacement.text.start - segment.text.start);
                wanted.push(replacement.text.end - segment.text.start);
            }
        }
        let mut replacements = replacements.to_vec();
        struct Edit {
            segment: usize,
            before: Range<usize>,
            after: Range<usize>,
            content: Vec<u8>,
        }
        let mut edits = Vec::new();
        let mut mappings = std::collections::BTreeMap::new();
        for replacement in &mut replacements {
            let index = self.segment_at(replacement.text.start, replacement.text.is_empty());
            let segment = self.segments.get(index).ok_or_else(|| {
                "No original encoding exists at the replacement boundary; use write for an empty file".to_string()
            })?;
            if replacement.text.end > segment.text.end {
                return Err(format!(
                    "REPLACE crosses an encoding boundary at byte {} (hex {}); split the change into edit blocks within each encoding segment",
                    segment.bytes.end,
                    hex(&self.raw[segment.bytes.end..self.raw.len().min(segment.bytes.end + 1)])
                ));
            }
            let codec = segment.codec.ok_or_else(|| {
                format!(
                    "that line cannot be edited: {}",
                    segment.issue.as_deref().unwrap_or("its encoding is undecided")
                )
            })?;
            if let std::collections::btree_map::Entry::Vacant(entry) = mappings.entry(index) {
                entry.insert(Mapping::new(
                    codec,
                    &self.raw[segment.bytes.clone()],
                    &self.text[segment.text.clone()],
                    segment.bytes.start,
                    &boundaries[&index],
                )?);
            }
            let local = replacement.text.start - segment.text.start..replacement.text.end - segment.text.start;
            let expanded = mappings[&index].expand(local.clone());
            let expanded_file = expanded.start + segment.text.start..expanded.end + segment.text.start;
            if expanded != local
                && expanded_file.start >= replacement.context.start
                && expanded_file.end <= replacement.context.end
            {
                replacement.content = format!(
                    "{}{}{}",
                    &self.text[expanded_file.start..replacement.text.start],
                    replacement.content,
                    &self.text[replacement.text.end..expanded_file.end]
                );
                replacement.text = expanded_file;
            }
            let local = replacement.text.start - segment.text.start..replacement.text.end - segment.text.start;
            let mapped = mappings[&index].range(local, codec, &self.raw[segment.bytes.clone()], segment.bytes.start)?;
            let content = if codec.is_stateful() {
                iso2022::replacement(codec, &self.raw[segment.bytes.clone()], mapped.clone(), &replacement.content)
            } else {
                codec.encode(&replacement.content)
            }
            .map_err(|error| {
                describe_error(
                    &error,
                    &self.raw[segment.bytes.start + mapped.start..],
                    segment.bytes.start + mapped.start,
                )
            })?;
            edits.push(Edit {
                segment: index,
                before: segment.bytes.start + mapped.start..segment.bytes.start + mapped.end,
                after: 0..0,
                content,
            });
        }
        let mut raw = Vec::new();
        let mut text = String::new();
        let mut raw_at = 0;
        let mut text_at = 0;
        let mut after_text = Vec::new();
        for (edit, replacement) in edits.iter_mut().zip(&replacements) {
            raw.extend_from_slice(&self.raw[raw_at..edit.before.start]);
            let start = raw.len();
            raw.extend_from_slice(&edit.content);
            edit.after = start..raw.len();
            raw_at = edit.before.end;
            text.push_str(&self.text[text_at..replacement.text.start]);
            let start = text.len();
            text.push_str(&replacement.content);
            after_text.push(start..text.len());
            text_at = replacement.text.end;
        }
        raw.extend_from_slice(&self.raw[raw_at..]);
        text.push_str(&self.text[text_at..]);
        let mut lengths: Vec<_> = self.segments.iter().map(|s| (s.bytes.len(), s.text.len())).collect();
        for (edit, replacement) in edits.iter().zip(&replacements) {
            let length = &mut lengths[edit.segment];
            length.0 = length.0 - edit.before.len() + edit.content.len();
            length.1 = length.1 - replacement.text.len() + replacement.content.len();
        }
        let mut shifted = Vec::new();
        let mut raw_start = self.bom.len();
        let mut text_start = 0;
        for (byte_length, text_length) in lengths {
            shifted.push((raw_start..raw_start + byte_length, text_start..text_start + text_length));
            raw_start += byte_length;
            text_start += text_length;
        }
        for (index, (segment, (bytes, range))) in self.segments.iter().zip(&shifted).enumerate() {
            if let Some(codec) = segment.codec {
                let decoded = codec.decode_at(&raw[bytes.clone()], bytes.start).map_err(|error| {
                    if codec.is_stateful() {
                        {
                            let original = edits.iter().find(|edit| edit.segment == index).expect("changed stateful segment").before.start;
                            format!("Replacement starts at original byte {original} (hex {}); proposed stream: {error}; this ISO-2022-JP splice cannot retain the surrounding switch bytes and remain strictly valid. Include adjacent characters in SEARCH to put their switches inside the replacement, or use write for an intentional whole-file rewrite", hex(&self.raw[original..self.raw.len().min(original + 1)]))
                        }
                    } else { error }
                })?;
                if decoded != text[range.clone()] {
                    return Err(format!(
                        "{} cannot join this replacement at byte {} without changing surrounding text; include more context or explicitly convert the file",
                        codec.label(),
                        bytes.start
                    ));
                }
            }
        }
        for (segment, (bytes, range)) in self.segments.iter_mut().zip(shifted) {
            segment.bytes = bytes;
            segment.text = range;
        }
        self.raw = raw;
        self.text = text;
        Ok(replacements.into_iter().zip(after_text).map(|(replacement, after)| (replacement.text, after)).collect())
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "EOF".into();
    }
    bytes.iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(" ")
}

pub(crate) fn describe_error(error: &EncodingError, raw: &[u8], base: usize) -> String {
    match error {
        EncodingError::Malformed { encoding, offset } => {
            let at = (*offset).min(raw.len());
            format!(
                "{encoding} does not read the bytes from byte {} (hex {}), so this text cannot be edited; xxd shows the bytes, or view the file with --encoding to name its encoding",
                base + offset,
                hex(&raw[at..raw.len().min(at + 1)])
            )
        }
        EncodingError::Unrepresentable { encoding, character } => {
            format!(
                "the file is {encoding}, which has no {character:?}; write the replacement in characters it has, or convert the file first"
            )
        }
        _ => error.to_string(),
    }
}
