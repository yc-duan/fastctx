//! Request-ordered text batching with one shared token budget and exact continuations.

use super::{BatchReadEntry, ReadRequest, image_file, pdf, text_file};
use crate::binary::detect_binary_type;
use crate::budget::{READ_TOKEN_BUDGET_ENV, TokenBudget, estimate_tokens, tool_token_budget};
use crate::encoding::canonical_encoding_label;
use crate::model::ToolResponse;
use crate::paths::{
    canonical_existing, display_path, io_error_message, is_local_file_uri_input,
    missing_read_file_message, parse_input_path, parse_local_path_input,
};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::io::Read;

const MAX_BATCH_ENTRIES: usize = 32;

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
    let mut entries = request
        .files
        .take()
        .expect("batch shape was validated by read_file");
    if !(1..=MAX_BATCH_ENTRIES).contains(&entries.len()) {
        return ToolResponse::error(format!(
            "Invalid files value: expected 1 to 32 entries, got {}.",
            entries.len()
        ));
    }
    for (parameter, present) in [
        ("offset", request.offset.is_some()),
        ("encoding", request.encoding.is_some()),
    ] {
        if present {
            return ToolResponse::error(format!(
                "The top-level {parameter} parameter cannot be combined with files; set it inside the files entries instead."
            ));
        }
    }
    if request.limit == Some(0) {
        return ToolResponse::error("Invalid limit value: 0. Expected an integer >= 1.");
    }
    if let Some(default_limit) = request.limit {
        for entry in &mut entries {
            entry.limit.get_or_insert(default_limit);
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
    let entries = match validate_entries(entries) {
        Ok(entries) => entries,
        Err(error) => return ToolResponse::error(error),
    };
    let budget = match tool_token_budget(READ_TOKEN_BUDGET_ENV) {
        Ok(budget) => budget,
        Err(error) => return ToolResponse::error(error),
    };
    pack_entries(entries, budget)
}

fn validate_entries(mut entries: Vec<BatchReadEntry>) -> Result<Vec<BatchReadEntry>, String> {
    let mut seen = HashSet::with_capacity(entries.len());
    for entry in &mut entries {
        if entry.offset == Some(0) {
            return Err("Invalid offset value: 0. Expected an integer >= 1.".to_string());
        }
        if entry.limit == Some(0) {
            return Err("Invalid limit value: 0. Expected an integer >= 1.".to_string());
        }
        let from_uri = is_local_file_uri_input(&entry.path);
        let parsed = parse_local_path_input(&entry.path)?;
        if let Some(encoding) = entry.encoding.as_deref()
            && let Err(rejection) = canonical_encoding_label(encoding)
        {
            return Err(rejection.message(""));
        }
        // A relative entry is an existence problem, not a request-shape one, so it is
        // reported in its own segment and never discards its neighbors. Keeping it out
        // of canonicalization also stops it from resolving into a false duplicate.
        // The URL crate may spell a Windows file URI with an 8.3 component even when the
        // equivalent native input used its long name. Canonicalization expands that platform
        // alias; Unix keeps the URI's lexical symlink spelling so it matches the plain input.
        let normalized_input_path = if cfg!(windows) && parsed.is_absolute() {
            canonical_existing(&parsed).unwrap_or_else(|_| parsed.clone())
        } else {
            parsed.clone()
        };
        let normalized_input = display_path(&normalized_input_path);
        let key_path = if parsed.is_absolute() {
            canonical_existing(&parsed).unwrap_or_else(|_| parsed.clone())
        } else {
            parsed.clone()
        };
        let key_path = display_path(&key_path);
        entry.path = if from_uri {
            normalized_input
        } else {
            continuation_path(&entry.path)
        };
        #[cfg(windows)]
        let key_path = key_path.to_ascii_lowercase();
        let key = (key_path, entry.offset, entry.limit, entry.encoding.clone());
        if !seen.insert(key) {
            return Err(format!(
                "Duplicate files entry: two entries request the exact same text interval for {} (including offset, limit, and encoding).",
                entry.path
            ));
        }
    }
    Ok(entries)
}

fn pack_entries(entries: Vec<BatchReadEntry>, budget: TokenBudget) -> ToolResponse {
    let total = entries.len();
    let mut progress = entries
        .iter()
        .map(ContinuationEntry::from_request)
        .map(Some)
        .collect::<Vec<_>>();
    let mut segments = Vec::new();
    // How many leading entries this response has fully settled (content shown in
    // full or an inline problem reported). A budget break stops the loop, so every
    // entry after the break was never attempted and must not be counted against
    // the ones already delivered (#32).
    let mut delivered = 0_usize;
    // The index whose content is only partially shown, if any; it stays in the
    // continuation array but counts as processed for the tally.
    let mut partially_shown = false;

    for (index, entry) in entries.iter().enumerate() {
        let prepared = prepare_entry(entry, budget.value);
        match prepared.outcome {
            PreparedOutcome::Message(message) => {
                let segment = format!("=== {} ===\n{message}", prepared.path);
                let mut proposed = progress.clone();
                proposed[index] = None;
                if !candidate_fits(
                    &segments,
                    &segment,
                    &proposed,
                    total,
                    delivered,
                    partially_shown,
                    budget.value,
                ) {
                    if segments.is_empty() {
                        return budget_too_small(budget);
                    }
                    break;
                }
                segments.push(segment);
                progress = proposed;
                delivered += 1;
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
                    partially_shown = true;
                    break;
                }
                delivered += 1;
            }
        }
    }

    ToolResponse::text(render_response(
        &segments,
        &progress,
        total,
        delivered,
        partially_shown,
    ))
}

fn prepare_entry(entry: &BatchReadEntry, collection_budget: usize) -> PreparedEntry {
    let parsed = parse_input_path(&entry.path);
    let input_display = display_path(&parsed);
    if !parsed.is_absolute() {
        return PreparedEntry {
            path: input_display,
            outcome: PreparedOutcome::Message(missing_read_file_message(&entry.path)),
        };
    }
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
        // During the binary search this entry is by definition the partially
        // shown one; the delivered count comes from the caller via `segments`.
        candidate_fits(
            segments,
            &segment,
            &proposed,
            total,
            segments.len(),
            true,
            budget,
        )
    };

    // Probing the whole slice first is what makes the search sound: dropping the last line
    // can also drop this file from the continuation array, so `fits` is only monotonic
    // below `maximum`. Returning here also keeps the common all-fits entry at one probe
    // instead of a full binary search that re-tokenizes the whole response each step.
    if fits(maximum) {
        return maximum;
    }
    if maximum <= 1 {
        return 0;
    }
    let mut best = 0;
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
    // The continuation's limit counts the requested window, not the file: the next
    // call should read exactly the lines this request promised but did not show.
    // A limit that already ran past EOF carries no remainder, so it is dropped —
    // an explicit cap of "whatever is left" is what an omitted limit already means.
    let remaining_lines = content.total_lines - last;
    let limit = entry.limit.and_then(|limit| {
        let requested_end = content.first.saturating_add(limit.saturating_sub(1));
        let value = requested_end.min(content.total_lines) - last;
        (value > 0 && value < remaining_lines).then_some(value)
    });
    Some(ContinuationEntry {
        path: entry.path.clone(),
        offset: Some(last.saturating_add(1)),
        limit,
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
    delivered: usize,
    partially_shown: bool,
    budget: usize,
) -> bool {
    let mut proposed = segments.to_vec();
    proposed.push(candidate.to_string());
    estimate_tokens(&render_response(
        &proposed,
        progress,
        total,
        delivered,
        partially_shown,
    )) <= budget
}

fn render_response(
    segments: &[String],
    progress: &[Option<ContinuationEntry>],
    total: usize,
    delivered: usize,
    partially_shown: bool,
) -> String {
    let terminal = batch_terminal(progress, total, delivered, partially_shown);
    if segments.is_empty() {
        terminal
    } else {
        format!("{}\n\n{terminal}", segments.join("\n\n"))
    }
}

fn batch_terminal(
    progress: &[Option<ContinuationEntry>],
    total: usize,
    delivered: usize,
    partially_shown: bool,
) -> String {
    let pending = progress.iter().flatten().collect::<Vec<_>>();
    if pending.is_empty() {
        let noun = if total == 1 { "entry" } else { "entries" };
        return format!("(Complete: {total} {noun} processed.)");
    }
    // `delivered` counts fully settled entries; a partially shown entry counts as
    // processed too, because its continuation carries the exact resume point. The
    // remainder of `total` was never attempted — the budget broke before it — so
    // saying "0 of N" for them would tell the model its delivered content did not
    // happen (#32).
    let processed = if partially_shown {
        delivered + 1
    } else {
        delivered
    };
    let noun = if processed == 1 { "entry" } else { "entries" };
    let json = serde_json::to_string(&pending).expect("continuation entries serialize");
    format!(
        "(Partial: {processed} {noun} in progress, {total} requested. Continue with files={json}.)"
    )
}

fn budget_too_small(budget: TokenBudget) -> ToolResponse {
    ToolResponse::error(format!(
        "{}={} is too small to return the required continuation note. Increase it and retry.",
        budget.variable, budget.value
    ))
}

impl ContinuationEntry {
    fn from_request(entry: &BatchReadEntry) -> Self {
        Self {
            path: entry.path.clone(),
            offset: entry.offset,
            limit: entry.limit,
            encoding: entry.encoding.clone(),
        }
    }
}

fn continuation_path(input: &str) -> String {
    display_path(&parse_input_path(input))
}
