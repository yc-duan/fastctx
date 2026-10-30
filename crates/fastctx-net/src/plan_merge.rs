//! Merge line edits against their common base. Overlapping, unequal edits
//! choose the file as a whole; its competing hub revision is already archived.

use imara_diff::{Algorithm, Diff, InternedInput};
use std::ops::Range;

#[derive(PartialEq)]
struct Edit<'a> {
    range: Range<usize>,
    lines: Vec<&'a str>,
}

fn edits<'a>(base: &[&str], changed: &[&'a str]) -> Vec<Edit<'a>> {
    let mut input = InternedInput::default();
    input.update_before(base.iter().copied());
    input.update_after(changed.iter().copied());
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    diff.hunks()
        .map(|hunk| Edit {
            range: hunk.before.start as usize..hunk.before.end as usize,
            lines: changed[hunk.after.start as usize..hunk.after.end as usize].to_vec(),
        })
        .collect()
}

pub fn merge(base: &str, hub: &str, file: &str) -> String {
    if hub == base || hub == file {
        return file.into();
    }
    if file == base {
        return hub.into();
    }
    let original: Vec<_> = base.split_inclusive('\n').collect();
    let hub_lines: Vec<_> = hub.split_inclusive('\n').collect();
    let file_lines: Vec<_> = file.split_inclusive('\n').collect();
    let mut merged = edits(&original, &hub_lines);
    for edit in edits(&original, &file_lines) {
        if merged.contains(&edit) {
            continue;
        }
        if merged.iter().any(|other| {
            edit.range.start < other.range.end && other.range.start < edit.range.end
                || edit.range.is_empty() && other.range.is_empty() && edit.range.start == other.range.start
                || edit.range.is_empty() && other.range.start < edit.range.start && edit.range.start < other.range.end
                || other.range.is_empty() && edit.range.start < other.range.start && other.range.start < edit.range.end
        }) {
            return file.into();
        }
        merged.push(edit);
    }
    merged.sort_by_key(|edit| (edit.range.start, edit.range.end));
    let mut result = String::new();
    let mut position = 0;
    for edit in merged {
        for line in &original[position..edit.range.start] {
            result.push_str(line);
        }
        for line in edit.lines {
            result.push_str(line);
        }
        position = edit.range.end;
    }
    for line in &original[position..] {
        result.push_str(line);
    }
    result
}

/// The submission identity is determined by the original edit, not by a
/// merged result or by a retry time. Account incarnation scopes its receipt.
pub fn submission(owner: &fastctx_types::Address, base: Option<u64>, markdown: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(serde_json::to_vec(&(owner, base, markdown)).expect("plain values serialize")))
}
