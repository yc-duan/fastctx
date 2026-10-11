//! Codex syntax and matching; the World's publication and observation policy.

mod matching;
mod parser;
mod seek_sequence;
mod streaming_parser;

use super::document::Document;
use super::storage::{PublishedEncoding, Target};
use super::{ApplyPatchRequest, FileChange, WriteError, WriteResult, Writer};
use crate::files::content_hash;
use fastctx_types::files::{SeenFile, SeenFiles, SeenVersion};
use parser::Hunk;

pub(super) fn apply_patch(
    writer: &Writer,
    request: ApplyPatchRequest,
    seen: &SeenFiles,
) -> Result<WriteResult, WriteError> {
    let hunks = parser::parse_patch(&request.input).map_err(|error| match error {
        parser::ParseError::InvalidPatchError(message) => format!("Invalid patch: {message}"),
        parser::ParseError::InvalidHunkError { message, line_number } => {
            format!("Invalid patch hunk on line {line_number}: {message}")
        }
    })?;
    if hunks.is_empty() {
        return Err("No files were modified.".to_string().into());
    }
    let mut changes = Vec::new();
    let mut stale = Vec::new();
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    // Commits from earlier sections are current knowledge within this request,
    // not evidence that the resident viewed the file. Nothing is written to seen.
    let mut known = seen.clone();
    for mut hunk in hunks.iter().cloned() {
        let applied =
            normalize_paths(&mut hunk).and_then(|()| apply_hunk(&hunk, writer, &mut known, &mut changes, &mut stale));
        if let Err(mut message) = applied {
            for path in &stale {
                message.push_str(&format!("\n{} changed since you last viewed it.", path.display()));
            }
            return Err(WriteError { note_prefix: 0, message, changes });
        }
        match hunk {
            Hunk::AddFile { .. } => added.push(hunk.path().to_path_buf()),
            Hunk::UpdateFile { .. } => modified.push(hunk.path().to_path_buf()),
            Hunk::DeleteFile { .. } => deleted.push(hunk.path().to_path_buf()),
        }
    }
    let mut text = writer.head("apply_patch", hunks.len());
    text.push_str("\nSuccess. Updated the following files:");
    for (marker, paths) in [('A', added), ('M', modified), ('D', deleted)] {
        for path in paths {
            text.push_str(&format!("\n{marker} {}", path.display()));
        }
    }
    for path in &stale {
        text.push_str(&format!(
            "\n{} changed since you last viewed it; the patch matched its current contents.",
            path.display()
        ));
    }
    Ok(WriteResult { note_prefix: text.len(), text, changes, changed_since_seen: stale })
}

fn normalize_paths(hunk: &mut Hunk) -> Result<(), String> {
    let path = match hunk {
        Hunk::AddFile { path, .. } | Hunk::DeleteFile { path } => path,
        Hunk::UpdateFile { path, move_path, .. } => {
            if let Some(destination) = move_path {
                *destination = super::absolute_path(destination)?;
            }
            path
        }
    };
    *path = super::absolute_path(path)?;
    Ok(())
}

fn apply_hunk(
    hunk: &Hunk,
    writer: &Writer,
    seen: &mut SeenFiles,
    changes: &mut Vec<FileChange>,
    stale: &mut Vec<std::path::PathBuf>,
) -> Result<(), String> {
    match hunk {
        Hunk::AddFile { path, contents } => {
            let target = Target::new(path, writer)?;
            let before = target.read()?;
            let selection =
                before.as_ref().map(|before| target.encoding(before, seen)).transpose()?.flatten().map(str::to_owned);
            let (after, label) = match &before {
                Some(before) => {
                    target.whole_overwrite(before, seen)?;
                    let doc = Document::decode(&before.bytes, selection.as_deref())
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                    (doc.replace(contents)?, doc.label())
                }
                None => (contents.as_bytes().to_vec(), "UTF-8"),
            };
            target.create_parent(writer)?;
            target.publish(
                before.as_ref(),
                &after,
                PublishedEncoding {
                    before_selection: selection.as_deref(),
                    label: Some(label),
                    selection: selection.as_deref(),
                },
                writer,
                changes,
            )?;
            remember(&target, &after, selection.as_deref(), seen);
            Ok(())
        }
        Hunk::DeleteFile { path } => {
            let target = Target::new(path, writer)?;
            let before = target.required()?;
            target.delete(&before, target.encoding(&before, seen)?, writer, changes)?;
            seen.remove(&target.resolved);
            Ok(())
        }
        Hunk::UpdateFile { path, move_path, chunks } => {
            let source = Target::new(path, writer)?;
            // Validate the move destination before any mutation of this hunk.
            let destination = move_path.as_ref().map(|path| Target::new(path, writer)).transpose()?;
            let before = source.required()?;
            if source.stale(&before, seen)? {
                stale.push(path.clone());
            }
            let selection = source.encoding(&before, seen)?.map(str::to_owned);
            let document = Document::decode(&before.bytes, selection.as_deref())
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let updated = matching::apply(&document.text, &path.to_string_lossy(), chunks, document.eol)?;
            let after = document.encode(&updated)?;
            if let Some(destination) = destination {
                let overwritten = destination.read()?;
                if source.resolved != destination.resolved
                    && let Some(overwritten) = &overwritten
                {
                    destination.whole_overwrite(overwritten, seen)?;
                }
                destination.create_parent(writer)?;
                source.verify(Some(&before), writer)?;
                let before_selection =
                    overwritten.as_ref().map(|before| destination.encoding(before, seen)).transpose()?.flatten();
                destination.publish(
                    overwritten.as_ref(),
                    &after,
                    PublishedEncoding {
                        before_selection,
                        label: Some(document.label()),
                        selection: selection.as_deref(),
                    },
                    writer,
                    changes,
                )?;
                remember(&destination, &after, selection.as_deref(), seen);
                // Codex writes the destination, then unlinks the source even for self moves.
                if source.resolved == destination.resolved {
                    let current = source.required()?;
                    if current.bytes != after {
                        return Err(format!(
                            "{} changed after the move destination was published; the source was not deleted",
                            path.display()
                        ));
                    }
                    source.delete(&current, selection.as_deref(), writer, changes)?;
                } else {
                    source.delete(&before, selection.as_deref(), writer, changes)?;
                }
                seen.remove(&source.resolved);
            } else {
                source.publish(
                    Some(&before),
                    &after,
                    PublishedEncoding {
                        before_selection: selection.as_deref(),
                        label: Some(document.label()),
                        selection: selection.as_deref(),
                    },
                    writer,
                    changes,
                )?;
                remember(&source, &after, selection.as_deref(), seen);
            }
            Ok(())
        }
    }
}

fn remember(target: &Target, bytes: &[u8], selection: Option<&str>, seen: &mut SeenFiles) {
    seen.insert(
        target.resolved.clone(),
        SeenFile { version: SeenVersion::Full { sha256: content_hash(bytes) }, encoding: selection.map(str::to_owned) },
    );
}
