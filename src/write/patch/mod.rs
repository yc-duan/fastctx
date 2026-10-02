//! Codex patch syntax and matching with encoding-preserving atomic file writes.

mod matching;
mod parser;
mod seek_sequence;
mod streaming_parser;

use super::document::Document;
use super::storage::{Target, lock};
use super::{ApplyPatchRequest, FileChange, WriteContext, WriteError, WriteResult, head_note};
use parser::Hunk;

/// Apply sections in order. Earlier committed sections are included in any subsequent error.
pub fn apply_patch(
    request: ApplyPatchRequest,
    context: &WriteContext<'_>,
) -> Result<WriteResult, WriteError> {
    let hunks = parser::parse_patch(&request.input).map_err(|error| error.to_string())?;
    if hunks.is_empty() {
        return Err("No files were modified.".to_string().into());
    }
    // Authorization is checked for the complete request before any mutation.
    for hunk in &hunks {
        let source = match hunk {
            Hunk::AddFile { path, .. }
            | Hunk::DeleteFile { path }
            | Hunk::UpdateFile { path, .. } => path,
        };
        Target::new(source, context)?;
        Target::new(hunk.path(), context)?;
    }
    let mut changes = Vec::new();
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    for hunk in &hunks {
        if let Err(message) = apply_hunk(hunk, context, &mut changes) {
            return Err(WriteError { message, changes });
        }
        match hunk {
            Hunk::AddFile { .. } => added.push(hunk.path()),
            Hunk::UpdateFile { .. } => modified.push(hunk.path()),
            Hunk::DeleteFile { .. } => deleted.push(hunk.path()),
        }
    }
    let mut text = head_note("apply_patch", context, hunks.len());
    for (marker, paths) in [('A', added), ('M', modified), ('D', deleted)] {
        for path in paths {
            text.push_str(&format!("\n{marker} {}", path.display()));
        }
    }
    Ok(WriteResult {
        text,
        changes,
        changed_since_observation: Vec::new(),
    })
}

fn apply_hunk(
    hunk: &Hunk,
    context: &WriteContext<'_>,
    changes: &mut Vec<FileChange>,
) -> Result<(), String> {
    match hunk {
        Hunk::AddFile { path, contents } => {
            let target = Target::new(path, context)?;
            target.create_parent(context)?;
            let _locks = lock(&[&target])?;
            let before = target.read()?;
            let after = match &before {
                Some(raw) => Document::decode(raw, &target.requested, context)?
                    .encode_replacement(contents)?,
                None => contents.as_bytes().to_vec(),
            };
            changes.push(target.publish(before.as_deref(), &after, context)?);
        }
        Hunk::DeleteFile { path } => {
            let target = Target::new(path, context)?;
            let _locks = lock(&[&target])?;
            let before = target.read_required()?;
            changes.push(target.delete(&before, context)?);
        }
        Hunk::UpdateFile {
            path,
            move_path,
            chunks,
        } => {
            let source = Target::new(path, context)?;
            let destination = move_path
                .as_ref()
                .map(|path| Target::new(path, context))
                .transpose()?;
            let _source_locks = if destination.is_none() {
                Some(lock(&[&source])?)
            } else {
                None
            };
            let before = source.read_required()?;
            let document = Document::decode(&before, &source.requested, context)?;
            let updated = matching::apply(
                &document.text,
                &path.to_string_lossy(),
                chunks,
                document.eol,
            )?;
            let after = document.encode(&updated)?;
            if let Some(destination) = destination {
                // Derive the move before creating directories, as Codex does. Recheck
                // the snapshot under both locks before publishing the destination.
                destination.create_parent(context)?;
                let _move_locks = lock(&[&source, &destination])?;
                let overwritten = destination.read()?;
                source.verify(Some(&before), context)?;
                changes.push(destination.publish(overwritten.as_deref(), &after, context)?);
                // Codex writes the destination before removing the source, including self moves.
                let delete_bytes = if source.resolved == destination.resolved {
                    &after
                } else {
                    &before
                };
                changes.push(source.delete(delete_bytes, context)?);
            } else {
                changes.push(source.publish(Some(&before), &after, context)?);
            }
        }
    }
    Ok(())
}
