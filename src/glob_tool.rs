//! Project filtering, deterministic ordering, and resumable paging for the glob tool.

use crate::budget::{GLOB_TOKEN_BUDGET_ENV, assemble_text, estimate_tokens, tool_token_budget};
use crate::model::ToolResponse;
use crate::paths::{
    canonical_existing, display_path, io_error_message, missing_search_path_message,
    parse_input_path,
};
use crate::traversal::{project_walk_error_path, walk_thread_count};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::{WalkBuilder, WalkState};
use schemars::JsonSchema;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1_000;
const MAX_RESULTS: usize = 100_000;

/// Project filtering policy used by glob traversal.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    /// Respect ignore files, include hidden files, and exclude `.git`.
    #[default]
    Project,
    /// Disable ignore, hidden-file, and `.git` filtering.
    All,
}

/// Deterministic ordering for glob results.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    /// Sort by absolute-path bytes in ascending order.
    #[default]
    Path,
    /// Sort by modification time descending, then by path bytes ascending.
    Modified,
}

/// Parameters for the glob tool.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct GlobRequest {
    /// The glob pattern to match files against, e.g. "**/*.rs".
    pub pattern: String,
    /// Absolute path of the directory to search in. Omit for the session working directory. Must be a valid directory if provided.
    pub path: Option<String>,
    /// "project" respects .gitignore/.ignore, includes hidden files, excludes .git (same traversal as grep). "all" disables all filtering.
    pub filter_mode: Option<FilterMode>,
    /// "path" = byte-order path sort. "modified" = most recently modified first.
    pub sort: Option<SortMode>,
    /// Skip the first N results — for paging.
    pub offset: Option<usize>,
    /// Max results per page (1-1000).
    #[schemars(range(min = 1, max = 1_000))]
    pub limit: Option<usize>,
}

#[derive(Debug, Eq, PartialEq)]
struct MatchEntry {
    path: String,
    modified: SystemTime,
}

/// Finds files according to the filtering, ordering, and paging contract.
pub fn glob_files(request: GlobRequest) -> ToolResponse {
    let root = match resolve_root(request.path.as_deref()) {
        Ok(root) => root,
        Err(response) => return response,
    };
    let matcher = match build_matcher(&request.pattern) {
        Ok(matcher) => matcher,
        Err(message) => return ToolResponse::error(message),
    };
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return ToolResponse::error(format!(
            "Invalid limit value: {limit}. Expected an integer from 1 to 1000."
        ));
    }
    let budget = match tool_token_budget(GLOB_TOKEN_BUDGET_ENV) {
        Ok(budget) => budget,
        Err(message) => return ToolResponse::error(message),
    };
    let mut matches =
        match collect_matches(&root, &matcher, request.filter_mode.unwrap_or_default()) {
            Ok(matches) => matches,
            Err(message) => return ToolResponse::error(message),
        };
    match request.sort.unwrap_or_default() {
        SortMode::Path => {
            matches.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()))
        }
        SortMode::Modified => matches.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| left.path.as_bytes().cmp(right.path.as_bytes()))
        }),
    }
    format_matches(
        &matches,
        request.offset.unwrap_or(0),
        limit,
        budget.value,
        budget.variable,
    )
}

fn resolve_root(input: Option<&str>) -> Result<PathBuf, ToolResponse> {
    let root = match input {
        Some(input) => {
            let parsed = parse_input_path(input);
            if !parsed.is_absolute() || !parsed.exists() {
                return Err(ToolResponse::error(missing_search_path_message(input)));
            }
            let metadata = fs::metadata(&parsed)
                .map_err(|error| ToolResponse::error(io_error_message(&parsed, &error)))?;
            if !metadata.is_dir() {
                return Err(ToolResponse::error(format!(
                    "Path is not a directory: {}",
                    display_path(&parsed)
                )));
            }
            canonical_existing(&parsed).unwrap_or(parsed)
        }
        None => {
            let current = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            canonical_existing(&current).unwrap_or(current)
        }
    };
    Ok(root)
}

fn build_matcher(pattern: &str) -> Result<GlobSet, String> {
    let glob = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| glob_error(&error))?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    builder.build().map_err(|error| glob_error(&error))
}

fn glob_error(error: &impl std::fmt::Display) -> String {
    format!("Invalid glob pattern: {error}. Use forms like \"**/*.rs\" or \"src/**/*.ts\".")
}

fn collect_matches(
    root: &Path,
    matcher: &GlobSet,
    filter_mode: FilterMode,
) -> Result<Vec<MatchEntry>, String> {
    let mut builder = WalkBuilder::new(root);
    match filter_mode {
        FilterMode::Project => {
            builder
                .hidden(false)
                .ignore(true)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .follow_links(false)
                .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git");
        }
        FilterMode::All => {
            // standard_filters(false) + hidden(false) matches the previous
            // unfiltered walkdir semantics: no ignore files, hidden and .git
            // contents included, links not followed.
            builder
                .standard_filters(false)
                .hidden(false)
                .follow_links(false);
        }
    }
    builder.threads(walk_thread_count());
    // Determinism comes from the post-collection sort; the capacity error
    // fires at the same "one match past MAX_RESULTS" boundary as before.
    let collected = Mutex::new(Vec::new());
    let failure = Mutex::new(None::<String>);
    builder.build_parallel().run(|| {
        Box::new(|entry| {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    let message = if let Some(io_error) = error.io_error() {
                        let path = project_walk_error_path(&error).unwrap_or(root);
                        io_error_message(path, io_error)
                    } else {
                        format!("Cannot traverse path: {error}")
                    };
                    record_glob_failure(&failure, message);
                    return WalkState::Quit;
                }
            };
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file() || file_type.is_symlink())
            {
                return WalkState::Continue;
            }
            match evaluate_match(root, &entry, matcher) {
                Ok(Some(matched)) => {
                    let mut sink = collected.lock().expect("glob sink poisoned");
                    if let Err(message) = ensure_result_capacity(sink.len()) {
                        record_glob_failure(&failure, message);
                        return WalkState::Quit;
                    }
                    sink.push(matched);
                    WalkState::Continue
                }
                Ok(None) => WalkState::Continue,
                Err(message) => {
                    record_glob_failure(&failure, message);
                    WalkState::Quit
                }
            }
        })
    });
    if let Some(message) = failure.into_inner().expect("glob failure poisoned") {
        return Err(message);
    }
    Ok(collected.into_inner().expect("glob sink poisoned"))
}

/// Keeps the first failure; later racing failures are equivalent fail-fast picks.
fn record_glob_failure(failure: &Mutex<Option<String>>, message: String) {
    let mut slot = failure.lock().expect("glob failure poisoned");
    if slot.is_none() {
        *slot = Some(message);
    }
}

/// Stat-then-match, preserving the sequential path's error surface: metadata
/// failures abort the walk even for files the pattern would not match.
fn evaluate_match(
    root: &Path,
    entry: &ignore::DirEntry,
    matcher: &GlobSet,
) -> Result<Option<MatchEntry>, String> {
    let path = entry.path();
    let metadata = if entry
        .file_type()
        .is_some_and(|file_type| file_type.is_symlink())
    {
        // Symlinks keep the follow-and-check semantics (broken links skip).
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error_message(path, &error)),
        }
    } else {
        match entry.metadata() {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Ok(None),
            // Fall back to the plain stat so rare metadata failures keep the
            // exact error/skip semantics of the original lookup.
            Err(_) => match fs::metadata(path) {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => return Ok(None),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(io_error_message(path, &error)),
            },
        }
    };
    let relative = path.strip_prefix(root).unwrap_or(path);
    if !matcher.is_match(display_path(relative)) {
        return Ok(None);
    }
    let modified = metadata
        .modified()
        .map_err(|error| io_error_message(path, &error))?;
    Ok(Some(MatchEntry {
        path: display_path(path),
        modified,
    }))
}

fn ensure_result_capacity(current: usize) -> Result<(), String> {
    if current >= MAX_RESULTS {
        Err("Too many matches: over 100000 files matched. Narrow the pattern or path.".to_string())
    } else {
        Ok(())
    }
}

fn format_matches(
    matches: &[MatchEntry],
    offset: usize,
    limit: usize,
    budget: usize,
    budget_variable: &str,
) -> ToolResponse {
    let total = matches.len();
    if total == 0 {
        return status_response(
            "(Complete: no files matched.)".to_string(),
            budget,
            budget_variable,
        );
    }
    if offset >= total {
        let verb = if total == 1 { "exists" } else { "exist" };
        return status_response(
            format!(
                "(Complete: no files at offset={offset}; only {} {verb}.)",
                counted(total, "file", "files")
            ),
            budget,
            budget_variable,
        );
    }

    let maximum = limit.min(total - offset);
    for shown in (1..=maximum).rev() {
        let lines = matches[offset..offset + shown]
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let terminal = glob_terminal(offset, shown, total);
        let output = assemble_text(&lines, &[terminal]);
        if estimate_tokens(&output) <= budget {
            return ToolResponse::text(output);
        }
    }
    budget_too_small(budget, budget_variable)
}

fn glob_terminal(offset: usize, shown: usize, total: usize) -> String {
    let range = entry_range(offset + 1, shown);
    if offset + shown < total {
        format!(
            "(Partial: {range} of {total} shown. Continue with offset={}.)",
            offset + shown
        )
    } else if offset == 0 {
        format!("(Complete: all {} shown.)", counted(total, "file", "files"))
    } else {
        format!("(Complete: {range} of {total} shown; end of results.)")
    }
}

fn entry_range(first: usize, shown: usize) -> String {
    if shown == 1 {
        format!("file {first}")
    } else {
        format!("files {first}-{}", first + shown - 1)
    }
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{count} {noun}")
}

fn status_response(status: String, budget: usize, budget_variable: &str) -> ToolResponse {
    if estimate_tokens(&status) <= budget {
        ToolResponse::text(status)
    } else {
        budget_too_small(budget, budget_variable)
    }
}

fn budget_too_small(budget: usize, budget_variable: &str) -> ToolResponse {
    ToolResponse::error(format!(
        "{budget_variable}={budget} is too small to return the required glob truncation note. Increase it and retry."
    ))
}

#[cfg(test)]
mod tests {
    use super::{MatchEntry, ensure_result_capacity, format_matches};
    use crate::ToolContent;
    use std::time::SystemTime;

    #[test]
    fn token_budget_keeps_the_page_prefix_and_returns_an_exact_offset() {
        let matches = (1..=3)
            .map(|index| MatchEntry {
                path: format!("{index}-{}", "x".repeat(100)),
                modified: SystemTime::UNIX_EPOCH,
            })
            .collect::<Vec<_>>();
        let response = format_matches(&matches, 0, 3, 55, "FASTCTX_TOKEN_BUDGET");
        assert!(!response.is_error, "{response:?}");
        let ToolContent::Text(output) = &response.content[0] else {
            panic!("expected text");
        };
        assert_eq!(
            output,
            &format!(
                "1-{xs}\n2-{xs}\n\n(Partial: files 1-2 of 3 shown. Continue with offset=2.)",
                xs = "x".repeat(100)
            )
        );
    }

    #[test]
    fn tiny_budget_fails_instead_of_returning_an_empty_success() {
        let matches = vec![MatchEntry {
            path: "/a/very/long/path.txt".to_string(),
            modified: SystemTime::UNIX_EPOCH,
        }];
        let response = format_matches(&matches, 0, 1, 1, "FASTCTX_TOKEN_BUDGET");
        assert!(response.is_error);
        assert_eq!(
            response.content,
            vec![ToolContent::Text(
                "FASTCTX_TOKEN_BUDGET=1 is too small to return the required glob truncation note. Increase it and retry."
                    .to_string()
            )]
        );
    }

    #[test]
    fn result_cap_has_the_exact_actionable_error() {
        assert!(ensure_result_capacity(99_999).is_ok());
        assert_eq!(
            ensure_result_capacity(100_000).unwrap_err(),
            "Too many matches: over 100000 files matched. Narrow the pattern or path."
        );
    }
}
