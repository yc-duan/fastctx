//! Exact text replacement and whole-file writes.

use super::document::Document;
use super::storage::{Target, lock};
use super::{
    ChangedSinceObservation, EditRequest, WriteContext, WriteError, WriteRequest, WriteResult,
    content_hash, head_note,
};
use std::fmt::Write as _;
use std::path::Path;

/// Match current decoded text exactly; stale session observations are reported but do not veto it.
pub fn edit(request: EditRequest, context: &WriteContext<'_>) -> Result<WriteResult, WriteError> {
    let target = Target::new(Path::new(&request.file_path), context)?;
    if request.old_string.is_empty() {
        return Err(
            "old_string must be non-empty; use write to create or replace a whole file."
                .to_string()
                .into(),
        );
    }
    if request.old_string == request.new_string {
        return Err(
            "old_string and new_string are identical; no edit was requested."
                .to_string()
                .into(),
        );
    }
    let _locks = lock(&[&target])?;
    let before = target.read_required()?;
    let document = Document::decode(&before, &target.requested, context)?;
    let Some(first) = document.text.find(&request.old_string) else {
        return Err(format!(
            "old_string was not found exactly in {}. Supply the current text with more context.",
            request.file_path
        )
        .into());
    };
    let next = first
        + request
            .old_string
            .chars()
            .next()
            .expect("nonempty old_string")
            .len_utf8();
    if !request.replace_all && document.text[next..].contains(&request.old_string) {
        return Err(format!("old_string occurs more than once in {}. Supply more context for a unique match, or set replace_all to true.", request.file_path).into());
    }
    let replacement = document.newlines(&request.new_string);
    let mut updated = String::new();
    let mut ranges = Vec::new();
    let mut offset = 0;
    for (start, matched) in document.text.match_indices(&request.old_string) {
        let end = start + matched.len();
        if (document.text.as_bytes().get(start) == Some(&b'\n')
            && start > 0
            && document.text.as_bytes()[start - 1] == b'\r')
            || (document.text.as_bytes().get(end) == Some(&b'\n')
                && document.text.as_bytes()[end - 1] == b'\r')
        {
            return Err("old_string splits a CRLF line ending. Include the complete line ending and more context.".to_string().into());
        }
        updated.push_str(&document.text[offset..start]);
        let changed_start = updated.len();
        updated.push_str(&replacement);
        ranges.push(changed_start..updated.len());
        offset = start + matched.len();
        if !request.replace_all {
            break;
        }
    }
    updated.push_str(&document.text[offset..]);
    let after = document.encode(&updated)?;
    let before_hash = content_hash(&before);
    let stale = context.observed_hash.filter(|hash| *hash != before_hash);
    let change = target.publish(Some(&before), &after, context)?;
    let mut text = head_note("edit", context, ranges.len());
    let mut changed_since_observation = Vec::new();
    if let Some(observed_hash) = stale {
        text.push_str("\nThe file changed since you last viewed it; the edit matched its current contents.");
        changed_since_observation.push(ChangedSinceObservation {
            file_path: target.requested,
            observed_hash: observed_hash.into(),
            current_hash: before_hash,
        });
    }
    text.push('\n');
    text.push_str(&request.file_path);
    text.push_str(&excerpts(&updated, &ranges));
    Ok(WriteResult {
        text,
        changes: vec![change],
        changed_since_observation,
    })
}

/// Refuse stale observed revisions only when replacing an existing file.
pub fn write(request: WriteRequest, context: &WriteContext<'_>) -> Result<WriteResult, WriteError> {
    let target = Target::new(Path::new(&request.file_path), context)?;
    target.create_parent(context)?;
    let _locks = lock(&[&target])?;
    let before = target.read()?;
    let after = if let Some(before) = &before {
        if context
            .observed_hash
            .is_some_and(|observed| observed != content_hash(before))
        {
            return Err(format!("{} changed since you last viewed it. Whole-file overwrite refused; view it again with `fastctx view` before writing.", request.file_path).into());
        }
        Document::decode(before, &target.requested, context)?
            .encode_replacement(&request.content)?
    } else {
        request.content.into_bytes()
    };
    let change = target.publish(before.as_deref(), &after, context)?;
    let action = if before.is_some() {
        "Updated"
    } else {
        "Created"
    };
    Ok(WriteResult {
        text: format!(
            "{}\n{action} {}",
            head_note("write", context, 1),
            request.file_path
        ),
        changes: vec![change],
        changed_since_observation: Vec::new(),
    })
}

fn excerpts(text: &str, changes: &[std::ops::Range<usize>]) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let mut starts = vec![0];
    let mut index = 0;
    while index < text.len() {
        match text.as_bytes()[index] {
            b'\r' if text.as_bytes().get(index + 1) == Some(&b'\n') => {
                index += 2;
                starts.push(index);
            }
            b'\r' | b'\n' => {
                index += 1;
                starts.push(index);
            }
            _ => index += 1,
        }
    }
    let mut windows: Vec<std::ops::Range<usize>> = Vec::new();
    for changed in changes {
        let start = starts.partition_point(|offset| *offset <= changed.start) - 1;
        let end = starts.partition_point(|offset| *offset <= changed.end) - 1;
        // Two neighboring lines on each side provide the requested edit excerpt.
        let window = start.saturating_sub(2)..(end + 3).min(lines.len());
        if let Some(previous) = windows.last_mut()
            && window.start <= previous.end
        {
            previous.end = previous.end.max(window.end);
        } else {
            windows.push(window);
        }
    }
    let mut result = String::new();
    for (index, window) in windows.iter().enumerate() {
        if index > 0 {
            result.push_str("\n...");
        }
        for line in window.clone() {
            let _ = write!(result, "\n{}: {}", line + 1, lines[line]);
        }
    }
    if lines.is_empty() {
        result.push_str("\n(empty file)");
    }
    result
}
