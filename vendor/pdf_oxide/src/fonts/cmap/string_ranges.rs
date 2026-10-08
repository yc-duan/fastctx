//! Modified by fastctx: bfrange destinations are UTF-16BE strings, not scalars.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct StringRange {
    pub start: u32,
    pub end: u32,
    base: u32,
    target: Vec<u16>,
}

impl StringRange {
    pub fn parse(line: &str) -> Option<Self> {
        if line.contains('[') {
            return None;
        }
        static RE: std::sync::LazyLock<Regex> =
            std::sync::LazyLock::new(|| Regex::new(r"<([^>]*)>\s*<([^>]*)>\s*<([^>]*)>").unwrap());
        let captures = RE.captures(line)?;
        let hex = |index: usize| captures[index].chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let start = u32::from_str_radix(&hex(1), 16).ok()?;
        let end = u32::from_str_radix(&hex(2), 16).ok()?;
        let target = hex(3);
        if start > end || target.is_empty() || target.len() % 4 != 0 {
            return None;
        }
        let target = target
            .as_bytes()
            .chunks_exact(4)
            .map(|chunk| u16::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok())
            .collect::<Option<Vec<_>>>()?;
        Some(Self { start, end, base: start, target })
    }

    pub fn get(&self, code: u32) -> Option<String> {
        let mut target = self.target.clone();
        // CMap string increments carry across the original big-endian bytes.
        // Resolve just the requested code; a large range need not be expanded
        // or silently stopped after an arbitrary number of entries.
        let mut carry = u64::from(code - self.base);
        for unit in target.iter_mut().rev() {
            let sum = u64::from(*unit) + carry;
            *unit = sum as u16;
            carry = sum >> 16;
        }
        if carry != 0 {
            return None;
        }
        String::from_utf16(&target).ok()
    }
}

impl CMap {
    pub(super) fn insert_string_range(&mut self, range: StringRange) {
        self.chars.retain(|&code, _| code < range.start || code > range.end);
        if self.string_ranges.last().is_none_or(|last| last.end < range.start) {
            self.string_ranges.push(range);
            return;
        }
        self.exclude_string_range(range.start, range.end);
        self.string_ranges.push(range);
        self.string_ranges.sort_by_key(|range| range.start);
    }

    pub(super) fn exclude_string_range(&mut self, start: u32, end: u32) {
        let old = std::mem::take(&mut self.string_ranges);
        for range in old {
            if range.end < start || range.start > end {
                self.string_ranges.push(range);
                continue;
            }
            if range.start < start {
                let mut left = range.clone();
                left.end = start - 1;
                self.string_ranges.push(left);
            }
            if range.end > end {
                let mut right = range;
                right.start = end + 1;
                self.string_ranges.push(right);
            }
        }
    }
}
