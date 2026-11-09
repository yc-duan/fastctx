//! Merge independent line edits against their common base, preserving source
//! bytes and separating concurrently appended lines at unterminated boundaries.

use imara_diff::{Algorithm, Diff, InternedInput};
use std::ops::Range;

#[derive(PartialEq)]
struct Edit<'a> {
    range: Range<usize>,
    lines: &'a [&'a str],
}

fn content(line: &str) -> &str {
    line.strip_suffix("\r\n").or_else(|| line.strip_suffix('\n')).unwrap_or(line)
}

fn edits<'a>(base: &[&str], changed: &'a [&'a str]) -> Vec<Edit<'a>> {
    let mut input = InternedInput::default();
    input.update_before(base.iter().copied().map(content));
    input.update_after(changed.iter().copied().map(content));
    // Histogram repeatedly scans large tails when unique lines trade places.
    let mut diff = Diff::compute(Algorithm::Myers, &input);
    diff.postprocess_lines(&input);
    let mut edits = Vec::new();
    let (mut before, mut after) = (0, 0);
    for hunk in diff.hunks() {
        let start = hunk.before.start as usize;
        // Alignment ignores terminators; their actual byte changes remain edits.
        while before < start {
            if base[before] != changed[after] {
                edits.push(Edit { range: before..before + 1, lines: &changed[after..after + 1] });
            }
            before += 1;
            after += 1;
        }
        before = hunk.before.end as usize;
        after = hunk.after.end as usize;
        edits.push(Edit { range: start..before, lines: &changed[hunk.after.start as usize..after] });
    }
    while before < base.len() {
        if base[before] != changed[after] {
            edits.push(Edit { range: before..before + 1, lines: &changed[after..after + 1] });
        }
        before += 1;
        after += 1;
    }
    edits
}

#[derive(Default)]
struct Output {
    text: String,
    crlf: bool,
}

impl Output {
    fn push<'a>(&mut self, lines: impl Iterator<Item = &'a str>) {
        let mut endings = (0_usize, 0_usize);
        let mut nonempty = false;
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if !nonempty && !self.text.is_empty() && !self.text.ends_with('\n') {
                self.text.push_str(if self.crlf { "\r\n" } else { "\n" });
            }
            nonempty = true;
            if line.ends_with("\r\n") {
                endings.0 += 1;
            } else if line.ends_with('\n') {
                endings.1 += 1;
            }
            self.text.push_str(line);
        }
        if nonempty {
            self.crlf = endings.0 > endings.1;
        }
    }
}

fn appended<'a>(base: &str, changed: &'a str) -> Option<&'a str> {
    let tail = changed.strip_prefix(base)?;
    if base.is_empty() || base.ends_with('\n') {
        Some(tail)
    } else {
        tail.strip_prefix("\r\n").or_else(|| tail.strip_prefix('\n')).filter(|tail| !tail.is_empty())
    }
}

/// Legacy and disconnected submissions preserve a conflicting incoming file as
/// the next revision; the previous hub revision remains in the archive.
pub fn merge(base: &str, hub: &str, file: &str) -> String {
    try_merge(base, hub, file).unwrap_or_else(|| file.into())
}

/// Merges independent line changes. An overlap is a conflict, not permission
/// to overwrite the hub. Concurrent appends retain the hub's arrival first.
pub fn try_merge(base: &str, hub: &str, file: &str) -> Option<String> {
    if hub == base || hub == file {
        return Some(file.into());
    }
    if file == base {
        return Some(hub.into());
    }
    if let (Some(hub_tail), Some(file_tail)) = (appended(base, hub), appended(base, file)) {
        let mut result = Output::default();
        result.push(hub[..hub.len() - hub_tail.len()].split_inclusive('\n'));
        result.push(hub_tail.split_inclusive('\n'));
        result.push(file_tail.split_inclusive('\n'));
        return Some(result.text);
    }
    let original: Vec<_> = base.split_inclusive('\n').collect();
    let hub_lines: Vec<_> = hub.split_inclusive('\n').collect();
    let file_lines: Vec<_> = file.split_inclusive('\n').collect();
    let hub_edits = edits(&original, &hub_lines);
    let file_edits = edits(&original, &file_lines);
    let mut hub_edits = hub_edits.iter().peekable();
    let mut file_edits = file_edits.iter().peekable();
    let mut result = Output::default();
    let mut position = 0;
    // Each step consumes at least one edit. Both inputs are ordered and disjoint.
    loop {
        let edit = match (hub_edits.peek(), file_edits.peek()) {
            (Some(hub), Some(file)) if hub == file => {
                file_edits.next();
                hub_edits.next().unwrap()
            }
            (Some(hub), Some(file)) => {
                let hub_first = (hub.range.start, hub.range.end) < (file.range.start, file.range.end);
                let (first, second) = if hub_first { (hub, file) } else { (file, hub) };
                if first.range.end > second.range.start || first.range.is_empty() && first.range == second.range {
                    return None;
                }
                if hub_first { hub_edits.next().unwrap() } else { file_edits.next().unwrap() }
            }
            (Some(_), None) => hub_edits.next().unwrap(),
            (None, Some(_)) => file_edits.next().unwrap(),
            (None, None) => break,
        };
        result.push(original[position..edit.range.start].iter().copied());
        result.push(edit.lines.iter().copied());
        position = edit.range.end;
    }
    result.push(original[position..].iter().copied());
    Some(result.text)
}

/// The submission identity is determined by the original edit, not by a
/// merged result or by a retry time. Account incarnation scopes its receipt.
pub fn submission(owner: &fastctx_types::Address, base: Option<u64>, markdown: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(serde_json::to_vec(&(owner, base, markdown)).expect("plain values serialize")))
}
