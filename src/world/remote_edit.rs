//! `remote_write` and `remote_edit` as they run on the machine that holds the file, plus the
//! content hash every World file operation reports so the caller's session can insist on
//! "inspected before changed" without the host remembering anything.
//!
//! The contract mirrors Claude Code's Write and Edit tools. Where those rely on the host
//! having tracked a Read, these carry the SHA-256 the caller last saw and compare it here,
//! right before the write: a compare-and-swap across machines.

use crate::control::paths::ControlPaths;
use crate::edit::document::TextDocument;
use crate::head_note::{HeadMetric, HeadNote};
use crate::paths::{canonical_existing, display_path, parse_input_path};
use crate::world::node::executor::VerbOutcome;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// Arguments of `remote_write` as they reach the holding machine.
#[derive(Debug, Deserialize)]
pub(crate) struct WriteArgs {
    pub(crate) path: String,
    pub(crate) content: String,
    /// The hash the caller's session recorded for this file; absent when it never inspected it.
    #[serde(default)]
    pub(crate) expected_sha256: Option<String>,
}

/// Arguments of `remote_edit` as they reach the holding machine.
#[derive(Debug, Deserialize)]
pub(crate) struct EditArgs {
    pub(crate) path: String,
    pub(crate) old_string: String,
    pub(crate) new_string: String,
    #[serde(default)]
    pub(crate) replace_all: bool,
    #[serde(default)]
    pub(crate) expected_sha256: Option<String>,
}

/// Lines of context shown on each side of an edit in the response body.
const CONTEXT_LINES: usize = 2;
/// Edits whose context is shown; the head note carries the count of the rest.
const SHOWN_EDITS: usize = 3;

/// The facts a World call reports about a file it read or wrote: its SHA-256 and canonical
/// path. Empty when the file cannot be read, so an inspect that failed records nothing.
pub(crate) fn file_facts(path: &str) -> BTreeMap<String, serde_json::Value> {
    let parsed = parse_input_path(path);
    let canonical = canonical_existing(&parsed).unwrap_or(parsed);
    let mut facts = BTreeMap::new();
    if let Ok(hash) = sha256_of(&canonical) {
        facts.insert("sha256".to_string(), serde_json::json!(hash));
        facts.insert(
            "path".to_string(),
            serde_json::json!(display_path(&canonical)),
        );
    }
    facts
}

/// Streams a file through SHA-256.
pub(crate) fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("Cannot read {}: {error}", display_path(path)))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Cannot read {}: {error}", display_path(path)))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn facts_for(path: &Path, sha256: &str) -> BTreeMap<String, serde_json::Value> {
    let canonical = canonical_existing(path).unwrap_or_else(|_| path.to_path_buf());
    BTreeMap::from([
        ("sha256".to_string(), serde_json::json!(sha256)),
        (
            "path".to_string(),
            serde_json::json!(display_path(&canonical)),
        ),
    ])
}

fn inspect_first(path: &str) -> VerbOutcome {
    VerbOutcome::error_text(format!(
        "Inspect {path} with inspect_local_file before changing it."
    ))
}

fn changed_since(path: &str) -> VerbOutcome {
    VerbOutcome::error_text(format!(
        "{path} changed since it was inspected; inspect it again."
    ))
}

/// Creates or wholly replaces a file. Replacing requires the caller to have inspected the
/// current content; creating does not.
pub(crate) fn write(args: WriteArgs) -> VerbOutcome {
    let path = parse_input_path(&args.path);
    let shown = display_path(&path);
    if !path.is_absolute() {
        return VerbOutcome::error_text(format!(
            "The path must be absolute on that machine: {shown}"
        ));
    }
    let existing = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return VerbOutcome::error_text(format!("Cannot write {shown}: {error}"));
        }
    };
    let unix_mode = match &existing {
        Some(metadata) => {
            if !metadata.is_file() {
                return VerbOutcome::error_text(format!(
                    "{shown} is not a regular file; remote_write only writes files."
                ));
            }
            let Some(expected) = args.expected_sha256.as_deref() else {
                return inspect_first(&shown);
            };
            match sha256_of(&path) {
                Ok(current) if current == expected => {}
                Ok(_) => return changed_since(&shown),
                Err(error) => return VerbOutcome::error_text(error),
            }
            crate::control::transaction::existing_unix_mode(&path)
        }
        None => {
            if let Some(parent) = path.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                return VerbOutcome::error_text(format!(
                    "Cannot create the directory of {shown}: {error}"
                ));
            }
            None
        }
    };
    let bytes = args.content.as_bytes();
    if let Err(error) = crate::control::transaction::atomic_replace(&path, bytes, unix_mode, false)
    {
        return VerbOutcome::error_text(error);
    }
    let sha256 = hex::encode(Sha256::digest(bytes));
    VerbOutcome {
        response: HeadNote::new(
            shown,
            HeadMetric::event(format!("{} bytes written", bytes.len())),
        )
        .into_text_response(""),
        facts: facts_for(&path, &sha256),
    }
}

/// Replaces `old_string` in a file the caller inspected, once or everywhere, preserving every
/// byte outside the matches: encoding, BOM, and the file's own line endings.
pub(crate) fn edit(args: EditArgs, control_paths: &ControlPaths) -> VerbOutcome {
    let limit = match crate::control::settings::load(control_paths)
        .and_then(|settings| settings.replace_file_limit_mib())
    {
        Ok(limit) => limit,
        Err(error) => {
            return VerbOutcome::error_text(format!(
                "Cannot use edit settings on this node: {error}."
            ));
        }
    };
    let document = match TextDocument::open(&args.path, None, limit) {
        Ok(document) => document,
        Err(error) => return VerbOutcome::error_text(error),
    };
    let shown = document.display_path();
    let Some(expected) = args.expected_sha256.as_deref() else {
        return inspect_first(&shown);
    };
    if document.revision() != expected {
        return changed_since(&shown);
    }
    if args.old_string.is_empty() {
        return VerbOutcome::error_text("old_string must not be empty.");
    }
    let logical = document.logical_text();
    let matches = logical
        .match_indices(&args.old_string)
        .map(|(start, _)| start)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return VerbOutcome::error_text(format!("old_string was not found in {shown}."));
    }
    if matches.len() > 1 && !args.replace_all {
        return VerbOutcome::error_text(format!(
            "old_string matches {} places in {shown}; include more surrounding lines or set replace_all.",
            matches.len()
        ));
    }
    let replacement = match document.encode_for_target(&args.new_string) {
        Ok(bytes) => bytes,
        Err(error) => return VerbOutcome::error_text(error),
    };
    let raw = document.original_bytes();
    let mut cursor = match document.raw_offset_cursor() {
        Ok(cursor) => cursor,
        Err(error) => return VerbOutcome::error_text(error),
    };
    let mut new_raw = Vec::with_capacity(raw.len() + replacement.len());
    let mut copied_to = 0;
    for start in &matches {
        let (raw_start, raw_end) = match (
            cursor.advance_to(*start),
            cursor.advance_to(start + args.old_string.len()),
        ) {
            (Ok(raw_start), Ok(raw_end)) => (raw_start, raw_end),
            (Err(error), _) | (_, Err(error)) => return VerbOutcome::error_text(error),
        };
        new_raw.extend_from_slice(&raw[copied_to..raw_start]);
        new_raw.extend_from_slice(&replacement);
        copied_to = raw_end;
    }
    new_raw.extend_from_slice(&raw[copied_to..]);
    let new_logical = logical.replacen(
        &args.old_string,
        &args.new_string,
        if args.replace_all { usize::MAX } else { 1 },
    );
    let body = context_body(&new_logical, &matches, &args.old_string, &args.new_string);
    let path = document.target_path().to_path_buf();
    if let Err(error) = document.commit(&new_raw) {
        return VerbOutcome::error_text(error);
    }
    let sha256 = hex::encode(Sha256::digest(&new_raw));
    VerbOutcome {
        response: HeadNote::new(
            shown,
            HeadMetric::count(matches.len(), "replacement", "replacements"),
        )
        .into_text_response(&body),
        facts: facts_for(&path, &sha256),
    }
}

/// The changed regions of the new text with a little context, in `inspect_local_file`'s
/// `N<tab>content` shape, so the model sees what it wrote without another call.
fn context_body(
    new_text: &str,
    old_starts: &[usize],
    old_string: &str,
    new_string: &str,
) -> String {
    let lines = new_text.split('\n').collect::<Vec<_>>();
    let mut shown = Vec::new();
    let mut last_end = 0;
    let mut shift = 0_i64;
    for start in old_starts.iter().take(SHOWN_EDITS) {
        let new_start = (*start as i64 + shift).max(0) as usize;
        shift += new_string.len() as i64 - old_string.len() as i64;
        let first_line = new_text[..new_start.min(new_text.len())]
            .matches('\n')
            .count();
        let last_line = first_line + new_string.matches('\n').count();
        let from = first_line.saturating_sub(CONTEXT_LINES).max(last_end);
        let to = (last_line + CONTEXT_LINES + 1).min(lines.len());
        if from >= to {
            continue;
        }
        if !shown.is_empty() && from > last_end {
            shown.push("...".to_string());
        }
        for (index, line) in lines.iter().enumerate().take(to).skip(from) {
            shown.push(format!("{}\t{line}", index + 1));
        }
        last_end = to;
    }
    shown.join("\n")
}
