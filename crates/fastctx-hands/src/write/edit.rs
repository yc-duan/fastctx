use super::blocks::{self, Block};
use super::excerpts::excerpts;
use super::storage::{PublishedEncoding, Target};
use super::{EditBlocksRequest, EditRequest, WriteError, WriteRequest, WriteResult, Writer};
use crate::encoding::file::{Replacement, Source};
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
    if block.old.is_empty() {
        return Err("SEARCH is empty; use write to create or replace a whole file".into());
    }
    // A SEARCH written in a here-document has LF line endings whatever the
    // file has, so both are matched with each CRLF read as LF; the matches
    // are mapped back to the file's own text, whose line endings stay.
    let plain = Plain::of(&document.text);
    let old = block.old.replace("\r\n", "\n");
    let Some(first) = plain.text.find(&old) else {
        if let Some(issue) = document.issue() {
            return Err(format!("SEARCH was not found; the file has lines that cannot be edited: {issue}"));
        }
        // View and edit decode alike, so text copied from `fastctx view`
        // is found; a miss is a difference in the text itself.
        return Err(
            "SEARCH was not found; view the current file and copy its text exactly, including whitespace".into()
        );
    };
    // Count overlapping occurrences too: "aa" occurs twice in "aaa".
    let next = first + old.chars().next().expect("nonempty pattern").len_utf8();
    if !replace_all && plain.text[next..].contains(&old) {
        return Err(format!("SEARCH occurs more than once; include more context, or use {all_option}"));
    }
    let matches: Vec<_> = plain
        .text
        .match_indices(&old)
        .map(|(start, text)| plain.original(start)..plain.original(start + text.len()))
        .collect();
    let dominant = crate::text::dominant_ending(&document.text);
    let mut replacements = Vec::new();
    for matched in matches {
        let original = &document.text[matched.clone()];
        if Plain::of(original).text == Plain::of(block.new).text {
            continue;
        }
        let ending = crate::text::replacement_ending(original, dominant);
        let replacement = crate::text::newlines(block.new, ending);
        let prefix = original
            .chars()
            .zip(replacement.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let suffix = original[prefix..]
            .chars()
            .rev()
            .zip(replacement[prefix..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        // Stateful context may contain switches needed to express the change.
        let (prefix, suffix) = if document.stateful_at(matched.start) { (0, 0) } else { (prefix, suffix) };
        replacements.push(Replacement {
            text: matched.start + prefix..matched.end - suffix,
            content: replacement[prefix..replacement.len() - suffix].into(),
            context: matched,
        });
    }
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

/// A text with each CRLF read as LF, and where the LF of each such pair
/// landed in it, so that an offset in it maps back to the text it came from.
struct Plain {
    text: String,
    joined: Vec<usize>,
}

impl Plain {
    fn of(text: &str) -> Self {
        let mut plain = String::with_capacity(text.len());
        let mut joined = Vec::new();
        let mut last = 0;
        for (at, _) in text.match_indices("\r\n") {
            plain.push_str(&text[last..at]);
            joined.push(plain.len());
            plain.push('\n');
            last = at + 2;
        }
        plain.push_str(&text[last..]);
        Self { text: plain, joined }
    }

    /// The offset in the original text of a boundary in the plain one. A
    /// boundary just before a joined LF lands before its CR, so a match never
    /// starts or ends between the two.
    fn original(&self, offset: usize) -> usize {
        offset + self.joined.partition_point(|&lf| lf < offset)
    }
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
