use std::ops::Range;

use super::file::{describe_error, hex};
use super::{TextDecoder, TextEncoding};

/// One indivisible encoded unit may yield more than one Unicode scalar (Big5).
/// Only units touching requested boundaries are retained, not one entry per character.
struct Unit {
    text: Range<usize>,
    bytes: Range<usize>,
}

pub(super) struct Mapping {
    units: Vec<Unit>,
    length: usize,
    direct: bool,
}

impl Mapping {
    pub fn new(
        codec: TextEncoding,
        bytes: &[u8],
        expected: &str,
        base: usize,
        boundaries: &[usize],
    ) -> Result<Self, String> {
        if codec == TextEncoding::utf8() {
            return Ok(Self { units: Vec::new(), length: bytes.len(), direct: true });
        }
        let mut wanted = boundaries.to_vec();
        wanted.sort_unstable();
        wanted.dedup();
        let mut next = 0;
        let mut decoder = TextDecoder::new(codec.without_bom());
        let mut units = Vec::new();
        let mut at = 0;
        let mut start = 0;
        let mut text_offset = 0;
        while at < bytes.len() {
            if codec.is_stateful() && bytes[at] == 0x1b {
                let end = at + 3;
                decoder.feed(&bytes[at..end], end == bytes.len()).map_err(|e| describe_error(&e, bytes, base))?;
                at = end;
                start = end;
                continue;
            }
            let text =
                decoder.feed(&bytes[at..at + 1], at + 1 == bytes.len()).map_err(|e| describe_error(&e, bytes, base))?;
            at += 1;
            if !text.is_empty() {
                if expected.get(text_offset..text_offset + text.len()) != Some(&text) {
                    return Err(format!(
                        "Decoded text changed while mapping {} at byte {}; view the file again",
                        codec.label(),
                        base + start
                    ));
                }
                while next < wanted.len() && wanted[next] < text_offset {
                    next += 1;
                }
                if wanted.get(next).is_some_and(|&boundary| boundary <= text_offset + text.len()) {
                    units.push(Unit { text: text_offset..text_offset + text.len(), bytes: start..at });
                }
                text_offset += text.len();
                start = at;
            }
        }
        if text_offset != expected.len() {
            return Err("Decoded text and original bytes disagree; view the file again".into());
        }
        Ok(Self { units, length: bytes.len(), direct: false })
    }

    pub fn expand(&self, range: Range<usize>) -> Range<usize> {
        if self.direct {
            return range;
        }
        let first = self.units.partition_point(|unit| unit.text.end <= range.start);
        let last = self.units.partition_point(|unit| unit.text.end < range.end);
        let start = self.units.get(first).map_or(range.start, |unit| unit.text.start.min(range.start));
        let end = self.units.get(last).map_or(range.end, |unit| {
            if range.end > unit.text.start { unit.text.end.max(range.end) } else { range.end }
        });
        start..end
    }

    pub fn range(
        &self,
        range: Range<usize>,
        codec: TextEncoding,
        raw: &[u8],
        base: usize,
    ) -> Result<Range<usize>, String> {
        if self.direct {
            return Ok(range);
        }
        let first = self.units.partition_point(|unit| unit.text.end <= range.start);
        let last = self.units.partition_point(|unit| unit.text.end <= range.end);
        let start = self.units.get(first).map_or(self.length, |u| u.bytes.start);
        let end = last.checked_sub(1).map_or(start, |i| self.units[i].bytes.end);
        let start_ok = self.units.get(first).is_none_or(|u| u.text.start == range.start);
        let end_ok = range.is_empty() || last.checked_sub(1).is_some_and(|i| self.units[i].text.end == range.end);
        if !start_ok || !end_ok {
            return Err(format!(
                "SEARCH splits an indivisible {} encoded unit at byte {} (hex {}); include the complete decoded character sequence in SEARCH",
                codec.label(),
                base + start,
                hex(&raw[start..self.units.get(first).map_or(start, |u| u.bytes.end)])
            ));
        }
        Ok(start..if range.is_empty() { start } else { end })
    }
}
