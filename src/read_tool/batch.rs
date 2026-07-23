//! Request-ordered text batching with one shared token budget and exact continuations.

use super::{BatchReadEntry, ReadRequest, image_file, pdf, text_file};
use crate::binary::detect_binary_type;
use crate::budget::{READ_TOKEN_BUDGET_ENV, TokenBudget, estimate_tokens, tool_token_budget};
use crate::encoding::canonical_encoding_label;
use crate::model::ToolResponse;
use crate::paths::{
    canonical_existing, display_path, io_error_message, missing_read_file_message, parse_input_path,
};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::io::Read;

const MAX_BATCH_FILES: usize = 32;

#[derive(Clone, Debug, Serialize)]
struct ContinuationEntry {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encoding: Option<String>,
}

struct PreparedEntry {
    path: String,
    outcome: PreparedOutcome,
}

enum PreparedOutcome {
    Content(text_file::BatchTextContent),
    Message(String),
}

pub(super) fn read_text_files(mut request: ReadRequest) -> ToolResponse {
    let entries = request
        .files
        .take()
        .expect("batch shape was validated by read_file");
    if !(1..=MAX_BATCH_FILES).contains(&entries.len()) {
        return ToolResponse::error(format!(
            "Invalid files value: expected 1 to 32 entries, got {}.",
            entries.len()
        ));
    }
    for (parameter, present) in [
        ("offset", request.offset.is_some()),
        ("limit", request.limit.is_some()),
        ("encoding", request.encoding.is_some()),
    ] {
        if present {
            return ToolResponse::error(format!(
                "The top-level {parameter} parameter cannot be combined with files; set it inside the files entries instead."
            ));
        }
    }
    for (parameter, present) in [
        ("pages", request.pages.is_some()),
        ("pdf_mode", request.pdf_mode.is_some()),
        ("view", request.view.is_some()),
    ] {
        if present {
            return ToolResponse::error(format!(
                "The {parameter} parameter cannot be combined with files; PDFs, images, and hex view are single-file reads."
            ));
        }
    }
    if let Err(error) = validate_entries(&entries) {
        return ToolResponse::error(error);
    }
    let budget = match tool_token_budget(READ_TOKEN_BUDGET_ENV) {
        Ok(budget) => budget,
        Err(error) => return ToolResponse::error(error),
    };
    pack_entries(entries, budget)
}

fn validate_entries(entries: &[BatchReadEntry]) -> Result<(), String> {
    let mut seen = HashSet::with_capacity(entries.len());
    for entry in entries {
        if entry.offset == Some(0) {
            return Err("Invalid offset value: 0. Expected an integer >= 1.".to_string());
        }
        if entry.limit == Some(0) {
            return Err("Invalid limit value: 0. Expected an integer >= 1.".to_string());
        }
        let parsed = parse_input_path(&entry.path);
        if !parsed.is_absolute() {
            return Err(missing_read_file_message(&entry.path));
        }
        if let Some(encoding) = entry.encoding.as_deref()
            && let Err(rejection) = canonical_encoding_label(encoding)
        {
            return Err(rejection.message(""));
        }
        let key_path = canonical_existing(&parsed).unwrap_or(parsed);
        let mut key = display_path(&key_path);
        #[cfg(windows)]
        key.make_ascii_lowercase();
        if !seen.insert(key) {
            return Err(format!(
                "Duplicate path in files: {}. List each file once.",
                continuation_path(&entry.path)
            ));
        }
    }
    Ok(())
}

fn pack_entries(entries: Vec<BatchReadEntry>, budget: TokenBudget) -> ToolResponse {
    let total = entries.len();
    let mut progress = entries
        .iter()
        .map(ContinuationEntry::from_request)
        .map(Some)
        .collect::<Vec<_>>();
    let mut segments = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        let prepared = prepare_entry(entry, budget.value);
        match prepared.outcome {
            PreparedOutcome::Message(message) => {
                let segment = format!("=== {} ===\n{message}", prepared.path);
                let mut proposed = progress.clone();
                proposed[index] = None;
                if !candidate_fits(&segments, &segment, &proposed, total, budget.value) {
                    if segments.is_empty() {
                        return budget_too_small(budget);
                    }
                    break;
                }
                segments.push(segment);
                progress = proposed;
            }
            PreparedOutcome::Content(content) => {
                let shown = largest_fitting_prefix(
                    &segments,
                    &prepared.path,
                    entry,
                    &content,
                    &progress,
                    index,
                    total,
                    budget.value,
                );
                if shown == 0 {
                    if segments.is_empty() {
                        return budget_too_small(budget);
                    }
                    break;
                }
                let proposed = progress_after(entry, &content, shown);
                let segment = content_segment(&prepared.path, &content, shown);
                progress[index] = proposed;
                segments.push(segment);
                if shown < content.lines.len() || !content.slice_complete {
                    break;
                }
            }
        }
    }

    ToolResponse::text(render_response(&segments, &progress, total))
}

fn prepare_entry(entry: &BatchReadEntry, collection_budget: usize) -> PreparedEntry {
    let parsed = parse_input_path(&entry.path);
    let input_display = display_path(&parsed);
    let metadata = match fs::metadata(&parsed) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PreparedEntry {
                path: input_display,
                outcome: PreparedOutcome::Message(missing_read_file_message(&entry.path)),
            };
        }
        Err(error) => {
            return PreparedEntry {
                path: input_display,
                outcome: PreparedOutcome::Message(io_error_message(&parsed, &error)),
            };
        }
    };
    let path = canonical_existing(&parsed).unwrap_or(parsed);
    let path_display = display_path(&path);
    if metadata.is_dir() {
        return PreparedEntry {
            path: path_display.clone(),
            outcome: PreparedOutcome::Message(format!(
                "{path_display} is a directory, not a file. Use the glob tool to list its contents."
            )),
        };
    }
    if !metadata.is_file() {
        return PreparedEntry {
            path: path_display.clone(),
            outcome: PreparedOutcome::Message(format!(
                "Cannot read non-regular file: {path_display}. Only regular files are supported."
            )),
        };
    }
    let mut prefix = Vec::new();
    if let Err(error) =
        fs::File::open(&path).and_then(|file| file.take(8 * 1024).read_to_end(&mut prefix))
    {
        return PreparedEntry {
            path: path_display,
            outcome: PreparedOutcome::Message(io_error_message(&path, &error)),
        };
    }
    if pdf::is_pdf(&path, &prefix) {
        return PreparedEntry {
            path: path_display,
            outcome: PreparedOutcome::Message(
                "PDF files cannot be included in files. Read this file separately with file_path and optional pages/pdf_mode."
                    .to_string(),
            ),
        };
    }
    if image_file::detect_image_mime(&path, &prefix).is_some() {
        return PreparedEntry {
            path: path_display,
            outcome: PreparedOutcome::Message(
                "Image files cannot be included in files. Read this file separately with file_path."
                    .to_string(),
            ),
        };
    }
    let outcome = match text_file::read_batch_text_file(
        &path,
        &path_display,
        entry.offset,
        entry.limit,
        entry.encoding.as_deref(),
        detect_binary_type(&prefix),
        collection_budget,
    ) {
        Ok(content) => PreparedOutcome::Content(content),
        Err(message) => PreparedOutcome::Message(message),
    };
    PreparedEntry {
        path: path_display,
        outcome,
    }
}

#[allow(clippy::too_many_arguments)]
fn largest_fitting_prefix(
    segments: &[String],
    path: &str,
    entry: &BatchReadEntry,
    content: &text_file::BatchTextContent,
    progress: &[Option<ContinuationEntry>],
    index: usize,
    total: usize,
    budget: usize,
) -> usize {
    let maximum = content.lines.len();
    let fits = |shown: usize| {
        let mut proposed = progress.to_vec();
        proposed[index] = progress_after(entry, content, shown);
        let segment = content_segment(path, content, shown);
        candidate_fits(segments, &segment, &proposed, total, budget)
    };

    let mut best = if fits(maximum) { maximum } else { 0 };
    if maximum <= 1 {
        return best;
    }
    let mut low = 1;
    let mut high = maximum - 1;
    while low <= high {
        let shown = low + (high - low) / 2;
        if fits(shown) {
            best = best.max(shown);
            low = shown.saturating_add(1);
        } else if shown == 1 {
            break;
        } else {
            high = shown - 1;
        }
    }
    best
}

fn progress_after(
    entry: &BatchReadEntry,
    content: &text_file::BatchTextContent,
    shown: usize,
) -> Option<ContinuationEntry> {
    let last = content.first.saturating_add(shown.saturating_sub(1));
    if last >= content.total_lines {
        return None;
    }
    Some(ContinuationEntry {
        path: continuation_path(&entry.path),
        offset: Some(last.saturating_add(1)),
        limit: entry
            .limit
            .and_then(|limit| limit.checked_sub(shown))
            .filter(|remaining| *remaining > 0),
        encoding: entry.encoding.clone(),
    })
}

fn content_segment(path: &str, content: &text_file::BatchTextContent, shown: usize) -> String {
    let last = content.first.saturating_add(shown.saturating_sub(1));
    let header = if content.total_is_known {
        format!(
            "=== {path} (lines {}-{last} of {}) ===",
            content.first, content.total_lines
        )
    } else {
        format!("=== {path} (lines {}-{last}) ===", content.first)
    };
    let mut lines = Vec::with_capacity(shown + 2);
    lines.push(header);
    if let Some(note) = &content.transcoding_note {
        lines.push(note.clone());
    }
    lines.extend(content.lines[..shown].iter().cloned());
    lines.join("\n")
}

fn candidate_fits(
    segments: &[String],
    candidate: &str,
    progress: &[Option<ContinuationEntry>],
    total: usize,
    budget: usize,
) -> bool {
    let mut proposed = segments.to_vec();
    proposed.push(candidate.to_string());
    estimate_tokens(&render_response(&proposed, progress, total)) <= budget
}

fn render_response(
    segments: &[String],
    progress: &[Option<ContinuationEntry>],
    total: usize,
) -> String {
    let terminal = batch_terminal(progress, total);
    if segments.is_empty() {
        terminal
    } else {
        format!("{}\n\n{terminal}", segments.join("\n\n"))
    }
}

fn batch_terminal(progress: &[Option<ContinuationEntry>], total: usize) -> String {
    let pending = progress.iter().flatten().collect::<Vec<_>>();
    if pending.is_empty() {
        let noun = if total == 1 { "file" } else { "files" };
        return format!("(Complete: {total} {noun} processed.)");
    }
    let json = serde_json::to_string(&pending).expect("continuation entries serialize");
    let processed = total - pending.len();
    format!("(Partial: {processed} of {total} files processed. Continue with files={json}.)")
}

fn budget_too_small(budget: TokenBudget) -> ToolResponse {
    ToolResponse::error(format!(
        "{}={} is too small to return the required continuation note. Increase it and retry.",
        budget.variable, budget.value
    ))
}

fn continuation_path(input: &str) -> String {
    display_path(&parse_input_path(input))
}

impl ContinuationEntry {
    fn from_request(entry: &BatchReadEntry) -> Self {
        Self {
            path: continuation_path(&entry.path),
            offset: entry.offset,
            limit: entry.limit,
            encoding: entry.encoding.clone(),
        }
    }
}
