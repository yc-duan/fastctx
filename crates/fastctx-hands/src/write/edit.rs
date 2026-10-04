use super::document::Document;
use super::excerpts::excerpts;
use super::storage::Target;
use super::{EditRequest, WriteError, WriteRequest, WriteResult, Writer};
use fastctx_types::files::SeenFiles;

pub(super) fn edit(
    writer: &Writer,
    request: EditRequest,
    seen: &SeenFiles,
) -> Result<WriteResult, WriteError> {
    let target = Target::new(&request.file_path, writer)?;
    if request.old_string.is_empty() {
        return Err(
            "old_string must be non-empty; use write to create or replace a whole file"
                .to_string()
                .into(),
        );
    }
    if request.old_string == request.new_string {
        return Err("old_string and new_string are identical".to_string().into());
    }
    let before = target.required()?;
    let stale = target.stale(&before, seen)?;
    let document = Document::decode(&before.bytes, target.encoding(&before, seen)?)?;
    let Some(first) = document.text.find(&request.old_string) else {
        return Err(format!(
            "old_string was not found exactly in {}. Supply the current text with more context",
            request.file_path.display()
        )
        .into());
    };
    // Count overlapping occurrences too: "aa" occurs twice in "aaa".
    let next = first
        + request
            .old_string
            .chars()
            .next()
            .expect("nonempty pattern")
            .len_utf8();
    if !request.replace_all && document.text[next..].contains(&request.old_string) {
        return Err(format!("old_string occurs more than once in {}. Supply more context, or set replace_all to true", request.file_path.display()).into());
    }
    let replacement = document.newlines(&request.new_string);
    let mut updated = String::new();
    let mut ranges = Vec::new();
    let mut offset = 0;
    for (start, matched) in document.text.match_indices(&request.old_string) {
        let end = start + matched.len();
        if [start, end].into_iter().any(|i| {
            i > 0
                && document.text.as_bytes().get(i) == Some(&b'\n')
                && document.text.as_bytes()[i - 1] == b'\r'
        }) {
            return Err(
                "old_string splits a CRLF line ending; include the complete line ending"
                    .to_string()
                    .into(),
            );
        }
        updated.push_str(&document.text[offset..start]);
        let changed_start = updated.len();
        updated.push_str(&replacement);
        ranges.push(changed_start..updated.len());
        offset = end;
        if !request.replace_all {
            break;
        }
    }
    updated.push_str(&document.text[offset..]);
    let after = document.encode(&updated)?;
    let mut changes = Vec::new();
    target
        .publish(Some(&before), &after, writer, &mut changes)
        .map_err(|message| WriteError {
            message,
            changes: changes.clone(),
        })?;
    let mut text = writer.head("edit", ranges.len());
    if stale {
        text.push_str(
            "\nThe file changed since you last viewed it; the edit matched its current contents.",
        );
    }
    text.push_str(&format!(
        "\n{}{}",
        request.file_path.display(),
        excerpts(&updated, &ranges)
    ));
    Ok(WriteResult {
        text,
        changes,
        changed_since_seen: if stale {
            vec![request.file_path]
        } else {
            Vec::new()
        },
    })
}

pub(super) fn write(
    writer: &Writer,
    request: WriteRequest,
    seen: &SeenFiles,
) -> Result<WriteResult, WriteError> {
    let target = Target::new(&request.file_path, writer)?;
    let before = target.read()?;
    let after = match &before {
        Some(before) => {
            target.whole_overwrite(before, seen)?;
            Document::decode(&before.bytes, target.encoding(before, seen)?)?
                .replace(&request.content)?
        }
        None => request.content.into_bytes(),
    };
    target.create_parent(writer)?;
    let mut changes = Vec::new();
    target
        .publish(before.as_ref(), &after, writer, &mut changes)
        .map_err(|message| WriteError {
            message,
            changes: changes.clone(),
        })?;
    let action = if before.is_some() {
        "Updated"
    } else {
        "Created"
    };
    Ok(WriteResult {
        text: format!(
            "{}\n{action} {}",
            writer.head("write", 1),
            request.file_path.display()
        ),
        changes,
        changed_since_seen: Vec::new(),
    })
}
