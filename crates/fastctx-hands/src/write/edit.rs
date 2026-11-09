use super::blocks::{self, Block};
use super::excerpts::excerpts;
use super::storage::{PublishedEncoding, Target};
use super::{EditBlocksRequest, EditRequest, WriteError, WriteRequest, WriteResult, Writer};
use crate::encoding::file::Source;
use fastctx_types::files::SeenFiles;
use std::ops::Range;
use std::path::Path;

pub(super) fn edit(writer: &Writer, request: EditRequest, seen: &SeenFiles) -> Result<WriteResult, WriteError> {
    edit_file(
        writer,
        &request.file_path,
        &[Block { old: &request.old_string, new: &request.new_string, markers: None }],
        request.replace_all,
        None,
        request.encoding.as_deref(),
        seen,
    )
}

pub(super) fn edit_blocks(
    writer: &Writer,
    request: EditBlocksRequest,
    seen: &SeenFiles,
) -> Result<WriteResult, WriteError> {
    let blocks = blocks::parse(&request.input)?;
    edit_file(
        writer,
        &request.file_path,
        &blocks,
        request.replace_all,
        Some(&request.input),
        request.encoding.as_deref(),
        seen,
    )
}

#[allow(clippy::too_many_arguments)]
fn edit_file(
    writer: &Writer,
    path: &Path,
    blocks: &[Block<'_>],
    replace_all: bool,
    // The SEARCH/REPLACE input the blocks were read from, when they were.
    input: Option<&str>,
    encoding: Option<&str>,
    seen: &SeenFiles,
) -> Result<WriteResult, WriteError> {
    let target = Target::new(path, writer)?;
    let before = target.required()?;
    let stale = target.stale(&before, seen)?;
    let selection = match encoding {
        Some(encoding) => Some(encoding),
        None => target.encoding(&before, seen)?,
    };
    let mut document = Source::decode(before.bytes.clone(), selection)?;
    // A marker line of the file copied into a block of the same marker
    // length would be read as a boundary: the input alone cannot tell.
    if let Some(input) = input
        && let Some(reason) = blocks::ambiguity(&document.text, blocks, input)
    {
        return Err(format!("{}: {reason}", path.display()).into());
    }
    let numbered = input.is_some();
    let original_text_hash = crate::files::content_hash(document.text.as_bytes());
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
    let after = &document.raw;
    let mut changes = Vec::new();
    if *after != before.bytes && crate::files::content_hash(document.text.as_bytes()) != original_text_hash {
        target
            .publish(
                Some(&before),
                after,
                PublishedEncoding { before_selection: selection, label: document.label(), selection },
                writer,
                &mut changes,
            )
            .map_err(|message| WriteError { note_prefix: 0, message, changes: changes.clone() })?;
    } else {
        ranges.clear();
    }
    let outcome = if changes.is_empty() { "no changes".into() } else { super::counted(ranges.len()) };
    let mut text = writer.file_head("edit", path, &outcome);
    if stale {
        text.push_str("\nThe file changed since you last viewed it; the edit matched its current contents.");
    }
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut note_prefix = text.len();
    text.push_str(&excerpts(&document.text, &ranges));
    if text[note_prefix..].starts_with('\n') {
        note_prefix += 1;
    }
    Ok(WriteResult {
        note_prefix,
        text,
        changes,
        changed_since_seen: if stale { vec![path.to_path_buf()] } else { Vec::new() },
    })
}

pub(super) fn replace(
    document: &mut Source,
    block: &Block<'_>,
    replace_all: bool,
    all_option: &str,
    ranges: &mut Vec<Range<usize>>,
) -> Result<(), String> {
    let replacements = fastctx_types::write::literal_edit::plan(
        &document.text,
        block.old,
        block.new,
        replace_all,
        fastctx_types::write::literal_edit::Options { all_option, issue: document.issue(), max_bytes: usize::MAX },
        |offset| document.stateful_at(offset),
    )?;
    let applied = document.replace(&replacements)?;
    // Remap earlier excerpts once per block. Remapping after every occurrence
    // would make --all quadratic in the number of matches.
    for range in ranges.iter_mut() {
        range.start = relocated(range.start, &applied, false);
        range.end = relocated(range.end, &applied, true);
    }
    ranges.extend(applied.into_iter().map(|(_, after)| after));
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
    let selection = match request.encoding.as_deref() {
        Some(encoding) => Some(encoding),
        None => before.as_ref().map(|before| target.encoding(before, seen)).transpose()?.flatten(),
    };
    let (after, encoding) = match &before {
        Some(before) => {
            target.whole_overwrite(before, seen)?;
            let document = Source::decode(before.bytes.clone(), selection)?;
            (document.whole(&request.content)?, document.label())
        }
        None => {
            let encoding =
                crate::encoding::explicit(selection.unwrap_or("utf-8"), &[]).map_err(|error| error.to_string())?;
            (encoding.encode(&request.content).map_err(|error| error.to_string())?, Some(encoding.label()))
        }
    };
    target.create_parent(writer)?;
    let mut changes = Vec::new();
    target
        .publish(
            before.as_ref(),
            &after,
            PublishedEncoding { before_selection: selection, label: encoding, selection },
            writer,
            &mut changes,
        )
        .map_err(|message| WriteError { note_prefix: 0, message, changes: changes.clone() })?;
    let outcome = if before.is_some() { "replaced" } else { "created" };
    let text = writer.file_head("write", &request.file_path, outcome);
    Ok(WriteResult { note_prefix: text.len(), text, changes, changed_since_seen: Vec::new() })
}
