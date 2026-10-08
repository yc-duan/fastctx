use super::blocks::{self, Block};
use super::document::Document;
use super::excerpts::excerpts;
use super::storage::Target;
use super::{EditBlocksRequest, EditRequest, WriteError, WriteRequest, WriteResult, Writer};
use fastctx_types::files::SeenFiles;
use std::ops::Range;
use std::path::Path;

pub(super) fn edit(writer: &Writer, request: EditRequest, seen: &SeenFiles) -> Result<WriteResult, WriteError> {
    edit_file(
        writer,
        &request.file_path,
        &[Block { old: &request.old_string, new: &request.new_string }],
        request.replace_all,
        false,
        seen,
    )
}

pub(super) fn edit_blocks(
    writer: &Writer,
    request: EditBlocksRequest,
    seen: &SeenFiles,
) -> Result<WriteResult, WriteError> {
    let blocks = blocks::parse(&request.input)?;
    edit_file(writer, &request.file_path, &blocks, request.replace_all, true, seen)
}

fn edit_file(
    writer: &Writer,
    path: &Path,
    blocks: &[Block<'_>],
    replace_all: bool,
    numbered: bool,
    seen: &SeenFiles,
) -> Result<WriteResult, WriteError> {
    let target = Target::new(path, writer)?;
    let before = target.required()?;
    let stale = target.stale(&before, seen)?;
    let mut document = Document::decode(&before.bytes, target.encoding(&before, seen)?)?;
    // Literal markers can form a syntactically valid second block. Parsing the
    // input alone cannot distinguish that collision from an intended batch.
    if numbered && blocks::contains_marker(&document.text) {
        return Err(format!(
            "{} contains a SEARCH/REPLACE marker on a whole line; edit cannot distinguish literal markers from block boundaries. Use write for this file",
            path.display()
        ).into());
    }
    let mut ranges = Vec::new();
    let all_option = if numbered { "--all" } else { "replace_all: true" };
    for (index, block) in blocks.iter().enumerate() {
        replace(&mut document, block, replace_all, all_option, &mut ranges).map_err(|reason| {
            if numbered {
                format!("Block {} in {}: {reason}", index + 1, path.display())
            } else {
                format!("{}: {reason}", path.display())
            }
        })?;
    }
    let after = document.encode(&document.text)?;
    let mut changes = Vec::new();
    target
        .publish(Some(&before), &after, document.label(), writer, &mut changes)
        .map_err(|message| WriteError { message, changes: changes.clone() })?;
    let mut text = writer.head("edit", ranges.len());
    if stale {
        text.push_str("\nThe file changed since you last viewed it; the edit matched its current contents.");
    }
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    text.push_str(&format!("\n{}{}", path.display(), excerpts(&document.text, &ranges)));
    Ok(WriteResult { text, changes, changed_since_seen: if stale { vec![path.to_path_buf()] } else { Vec::new() } })
}

fn replace(
    document: &mut Document,
    block: &Block<'_>,
    replace_all: bool,
    all_option: &str,
    ranges: &mut Vec<Range<usize>>,
) -> Result<(), String> {
    if block.old.is_empty() {
        return Err("SEARCH is empty; use write to create or replace a whole file".into());
    }
    if block.old == block.new {
        return Err("SEARCH and REPLACE are identical; supply a different replacement".into());
    }
    let Some(first) = document.text.find(block.old) else {
        return Err("SEARCH was not found exactly; view the current file and copy its text, including whitespace and line endings".into());
    };
    // Count overlapping occurrences too: "aa" occurs twice in "aaa".
    let next = first + block.old.chars().next().expect("nonempty pattern").len_utf8();
    if !replace_all && document.text[next..].contains(block.old) {
        return Err(format!("SEARCH occurs more than once; include more context, or use {all_option}"));
    }
    let matches: Vec<_> =
        document.text.match_indices(block.old).map(|(start, text)| start..start + text.len()).collect();
    for matched in &matches {
        if [matched.start, matched.end].into_iter().any(|i| {
            i > 0 && document.text.as_bytes().get(i) == Some(&b'\n') && document.text.as_bytes()[i - 1] == b'\r'
        }) {
            return Err("SEARCH splits a CRLF line ending; include the complete line ending".into());
        }
    }
    let replacement = document.newlines(block.new);
    document.encode(&replacement)?;
    let mut updated = String::new();
    let mut applied = Vec::new();
    let mut offset = 0;
    for matched in matches {
        updated.push_str(&document.text[offset..matched.start]);
        let start = updated.len();
        updated.push_str(&replacement);
        offset = matched.end;
        applied.push((matched, start..updated.len()));
    }
    updated.push_str(&document.text[offset..]);
    // Remap earlier excerpts once per block. Remapping after every occurrence
    // would make --all quadratic in the number of matches.
    for range in ranges.iter_mut() {
        range.start = relocated(range.start, &applied, false);
        range.end = relocated(range.end, &applied, true);
    }
    ranges.extend(applied.into_iter().map(|(_, after)| after));
    document.text = updated;
    Ok(())
}

fn relocated(offset: usize, edits: &[(Range<usize>, Range<usize>)], right: bool) -> usize {
    let index = edits.partition_point(|(before, _)| before.end <= offset);
    if let Some((before, after)) = edits.get(index)
        && before.start < offset
    {
        return if right { after.end } else { after.start };
    }
    match index.checked_sub(1).map(|previous| &edits[previous]) {
        Some((before, after)) => offset - before.end + after.end,
        None => offset,
    }
}

pub(super) fn write(writer: &Writer, request: WriteRequest, seen: &SeenFiles) -> Result<WriteResult, WriteError> {
    let target = Target::new(&request.file_path, writer)?;
    let before = target.read()?;
    let (after, encoding) = match &before {
        Some(before) => {
            target.whole_overwrite(before, seen)?;
            let document = Document::decode(&before.bytes, target.encoding(before, seen)?)?;
            (document.replace(&request.content)?, document.label())
        }
        None => (request.content.into_bytes(), "UTF-8"),
    };
    target.create_parent(writer)?;
    let mut changes = Vec::new();
    target
        .publish(before.as_ref(), &after, encoding, writer, &mut changes)
        .map_err(|message| WriteError { message, changes: changes.clone() })?;
    let action = if before.is_some() { "Updated" } else { "Created" };
    Ok(WriteResult {
        text: format!("{}\n{action} {}", writer.head("write", 1), request.file_path.display()),
        changes,
        changed_since_seen: Vec::new(),
    })
}
