//! Project filtering, deterministic ordering, and resumable paging for the glob tool.

use crate::bounded_sort::sort_cancelable;
use crate::budget::{
    ErrorBudgetAdapter, ErrorClass, GLOB_TOKEN_BUDGET_ENV, error_budget_hint, tool_token_budget,
};
use crate::file_executor::GrepGlobExecutor;
use crate::glob_filter::{GlobPatterns, PathGlobFilter};
use crate::model::ToolResponse;
use crate::operation::{OpError, OperationCtx, RequestWorkGuard};
use crate::path_codec::{PathRecord, ResolvedRoot, RootRequirement, resolve_search_root};
use crate::render_plan::{LineRenderGraph, RenderPlanError};
use crate::skip_report::{SkipTally, detail_line, terminal_with_skips};
use crate::traversal::{
    SkippedPaths, TraversalCollection, TraversalFailure, TraversalLimit, collect_walk_batched,
};
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use std::fs;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1_000;
const MAX_RESULTS: usize = 100_000;
const TOO_MANY_MATCHES_ERROR: &str =
    "Too many matches: over 100000 files matched. Narrow the pattern or path.";

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

/// JSON schema wrapper for `Option<SortMode>` that always emits a `type` field,
/// required by strict LLM APIs (e.g. Gemini 3.1 Pro) that reject enums without
/// an explicit `type` (#25).
pub(crate) fn sort_field_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::schema::Schema {
    schemars::schema::SchemaObject {
        instance_type: Some(schemars::schema::InstanceType::String.into()),
        enum_values: Some(vec![
            serde_json::Value::String("path".to_string()),
            serde_json::Value::String("modified".to_string()),
        ]),
        ..Default::default()
    }
    .into()
}

/// Parameters for the glob tool.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct GlobRequest {
    /// One glob or a list of globs to match files. Prefix exclusions with `!`, e.g. ["**/*.rs", "!tests/**"]; negative-only patterns list every other file.
    pub pattern: GlobPatterns,
    /// Directory to search; omit for the session working directory.
    #[schemars(description = crate::model_guidance::local_path_description(
        "Directory to search. Omit for the session working directory; when provided, it must name an existing directory."
    ))]
    pub path: Option<String>,
    /// "project" respects .gitignore/.ignore, includes hidden files, excludes .git (same traversal as grep). "all" disables all filtering.
    pub filter_mode: Option<FilterMode>,
    /// "path" = byte-order path sort. "modified" = most recently modified first.
    #[schemars(with = "crate::glob_tool::sort_field_schema")]
    pub sort: Option<SortMode>,
    /// Skip the first N results — for paging.
    pub offset: Option<usize>,
    /// Max results per page (1-1000).
    #[schemars(range(min = 1, max = 1_000))]
    pub limit: Option<usize>,
}

#[derive(Debug, Eq, PartialEq)]
struct MatchEntry {
    path: PathRecord,
}

/// Finds files within a caller-owned cancellation scope.
///
/// Cancellation is checked throughout traversal, collection, sorting,
/// rendering, and token verification. A cancelled operation returns an error
/// response and never exposes a partial success body.
pub fn glob_files(request: GlobRequest, cancellation: CancellationToken) -> ToolResponse {
    let (mut guard, operation) = RequestWorkGuard::new(
        rmcp::model::RequestId::String(Arc::from("direct-glob")),
        cancellation,
    );
    let response = glob_files_with_execution(request, operation, GrepGlobExecutor::shared());
    guard.disarm();
    response
}

fn glob_files_with_execution(
    request: GlobRequest,
    operation: OperationCtx,
    executor: Arc<GrepGlobExecutor>,
) -> ToolResponse {
    let adapter = ErrorBudgetAdapter::new(
        error_budget_hint(GLOB_TOKEN_BUDGET_ENV),
        GLOB_TOKEN_BUDGET_ENV,
    );
    let budget = match tool_token_budget(GLOB_TOKEN_BUDGET_ENV) {
        Ok(budget) => budget,
        Err(message) => return adapter.error(ErrorClass::Budget, message),
    };
    adapter.adapt(glob_files_with_execution_unadapted(
        request,
        budget.value,
        budget.variable,
        &operation,
        &executor,
    ))
}

fn glob_files_with_execution_unadapted(
    request: GlobRequest,
    budget: usize,
    budget_variable: &str,
    operation: &OperationCtx,
    executor: &Arc<GrepGlobExecutor>,
) -> ToolResponse {
    if operation.check().is_err() {
        return ToolResponse::error("Request cancelled.");
    }
    let root = match resolve_search_root(request.path.as_deref(), RootRequirement::Directory) {
        Ok(root) => root,
        Err(message) => return ToolResponse::error(message),
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
    let sort = request.sort.unwrap_or_default();
    let collected = match collect_matches(
        &root,
        &matcher,
        request.filter_mode.unwrap_or_default(),
        sort,
        operation,
        executor,
    ) {
        Ok(collected) => collected,
        Err(message) => return ToolResponse::error(message),
    };
    let report = SkipReport::new(&collected.skipped);
    let matches = match sort_cancelable(
        collected.items,
        move |left, right| compare_match_entries(sort, left, right),
        Some(operation),
        Some(executor),
    ) {
        Ok(sorted) => sorted.items,
        Err(error) => return ToolResponse::error(error.to_string()),
    };
    format_matches(
        &matches,
        &report,
        request.offset.unwrap_or(0),
        limit,
        budget,
        budget_variable,
        Some(operation),
    )
}

/// Runs glob on the server's request cancellation scope and shared executor.
pub(crate) fn glob_files_cancellable(
    operation: OperationCtx,
    executor: Arc<GrepGlobExecutor>,
    request: GlobRequest,
) -> Result<ToolResponse, OpError> {
    let work = operation.inline_work();
    work.check_inline()?;
    let response = glob_files_with_execution(request, operation.clone(), executor);
    work.check_inline()?;
    Ok(response)
}

fn build_matcher(patterns: &GlobPatterns) -> Result<PathGlobFilter, String> {
    PathGlobFilter::compile(patterns, true).map_err(|error| glob_error(&error))
}

fn glob_error(error: &impl std::fmt::Display) -> String {
    format!("Invalid glob pattern: {error}. Use forms like \"**/*.rs\" or \"src/**/*.ts\".")
}

fn collect_matches(
    root: &ResolvedRoot,
    matcher: &PathGlobFilter,
    filter_mode: FilterMode,
    sort: SortMode,
    operation: &OperationCtx,
    executor: &Arc<GrepGlobExecutor>,
) -> Result<TraversalCollection<MatchEntry>, String> {
    if operation.check().is_err() {
        return Err("Request cancelled.".to_string());
    }
    let mut builder = WalkBuilder::new(&root.native);
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
    collect_walk_batched(
        builder,
        &root.native,
        Some(operation),
        Some(executor),
        Some(TraversalLimit {
            maximum: MAX_RESULTS,
            message: TOO_MANY_MATCHES_ERROR,
        }),
        |entry| {
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file() || file_type.is_symlink())
            {
                return Ok(None);
            }
            evaluate_match(root, entry, matcher, sort)
        },
    )
}

fn compare_match_entries(
    sort: SortMode,
    left: &MatchEntry,
    right: &MatchEntry,
) -> std::cmp::Ordering {
    match sort {
        SortMode::Path => left
            .path
            .display
            .as_bytes()
            .cmp(right.path.display.as_bytes())
            .then_with(|| left.path.native_key.cmp(&right.path.native_key)),
        SortMode::Modified => right.path.modified.cmp(&left.path.modified).then_with(|| {
            left.path
                .display
                .as_bytes()
                .cmp(right.path.display.as_bytes())
                .then_with(|| left.path.native_key.cmp(&right.path.native_key))
        }),
    }
}

fn evaluate_match(
    root: &ResolvedRoot,
    entry: &ignore::DirEntry,
    matcher: &PathGlobFilter,
    sort: SortMode,
) -> Result<Option<MatchEntry>, TraversalFailure> {
    let path = entry.path();
    let preliminary = PathRecord::without_metadata(path, &root.native);
    if !matcher.is_match(preliminary.relative_match.as_ref()) {
        return Ok(None);
    }
    if sort == SortMode::Path
        && entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
    {
        return Ok(Some(MatchEntry { path: preliminary }));
    }
    let metadata = if entry
        .file_type()
        .is_some_and(|file_type| file_type.is_symlink())
    {
        // Symlinks keep the follow-and-check semantics (broken links skip).
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(TraversalFailure::from_io(path, &error)),
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
                Err(error) => return Err(TraversalFailure::from_io(path, &error)),
            },
        }
    };
    let record =
        PathRecord::from_metadata(path, &root.native, &metadata, sort == SortMode::Modified)
            .map_err(|error| TraversalFailure::from_io(path, &error))?;
    Ok(Some(MatchEntry { path: record }))
}

/// A note set that fits the budget, with the token count that proved it.
type FittedNotes = Option<(Vec<Arc<str>>, usize)>;

/// The paths this walk never entered, ready to be rendered alongside results.
struct SkipReport {
    details: Vec<Arc<str>>,
    tally: SkipTally,
}

impl SkipReport {
    fn new(skipped: &SkippedPaths) -> Self {
        let details = skipped
            .listed()
            .map(|path| Arc::<str>::from(detail_line(path.display, path.reason)))
            .collect::<Vec<_>>();
        Self {
            tally: SkipTally {
                files: 0,
                unreachable: skipped.total(),
                listed: details.len(),
            },
            details,
        }
    }

    fn notes(&self, shown_details: usize, terminal: &str) -> Option<Vec<Arc<str>>> {
        let terminal = terminal_with_skips(terminal, &self.tally, shown_details)?;
        let mut notes = self.details[..shown_details].to_vec();
        notes.push(Arc::from(terminal));
        Some(notes)
    }
}

fn format_matches(
    matches: &[MatchEntry],
    report: &SkipReport,
    offset: usize,
    limit: usize,
    budget: usize,
    budget_variable: &str,
    operation: Option<&OperationCtx>,
) -> ToolResponse {
    let total = matches.len();
    if total == 0 {
        return status_response(
            "(Complete: no files matched.)".to_string(),
            report,
            budget,
            budget_variable,
            operation,
        );
    }
    if offset >= total {
        let verb = if total == 1 { "exists" } else { "exist" };
        return status_response(
            format!(
                "(Complete: no files at offset={offset}; only {} {verb}.)",
                counted(total, "file", "files")
            ),
            report,
            budget,
            budget_variable,
            operation,
        );
    }

    let maximum = limit.min(total - offset);
    let lines = matches[offset..offset + maximum]
        .iter()
        .map(|entry| Arc::clone(&entry.path.display))
        .collect::<Vec<_>>();
    let mut graph = match LineRenderGraph::new(
        lines,
        operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint),
    ) {
        Ok(graph) => graph,
        Err(error) => return render_plan_failure(error),
    };
    // The body is sized against a bare skip tally so results never lose room to
    // the detail lines describing what is missing; detail fills what is left.
    for shown in (1..=maximum).rev() {
        let terminal = glob_terminal(offset, shown, total);
        let Some(notes) = report.notes(0, &terminal) else {
            return render_plan_failure(RenderPlanError::InvalidTerminal);
        };
        let tokens = match graph.probe_notes(
            shown,
            &notes,
            operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint),
        ) {
            Ok(tokens) => tokens,
            Err(error) => return render_plan_failure(error),
        };
        if tokens <= budget {
            return finish_with_skips(
                &mut graph,
                shown,
                &terminal,
                report,
                budget,
                budget_variable,
                operation,
            );
        }
    }
    budget_too_small(budget, budget_variable)
}

/// Renders a fixed body with as many skip details as the remaining budget holds.
///
/// The caller has already proven the zero-detail form fits, so the search only
/// has to find how much detail fits on top of it.
fn finish_with_skips(
    graph: &mut LineRenderGraph,
    shown: usize,
    terminal: &str,
    report: &SkipReport,
    budget: usize,
    budget_variable: &str,
    operation: Option<&OperationCtx>,
) -> ToolResponse {
    let work = operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint);
    let probe =
        |graph: &mut LineRenderGraph, details: usize| -> Result<FittedNotes, RenderPlanError> {
            let notes = report
                .notes(details, terminal)
                .ok_or(RenderPlanError::InvalidTerminal)?;
            let tokens = graph.probe_notes(shown, &notes, work)?;
            Ok((tokens <= budget).then_some((notes, tokens)))
        };

    let full = report.details.len();
    let mut best = match probe(graph, full) {
        Ok(Some(hit)) => Some(hit),
        Ok(None) => None,
        Err(error) => return render_plan_failure(error),
    };
    if best.is_none() && full > 0 {
        let (mut low, mut high) = (0_usize, full - 1);
        while low <= high {
            let middle = low + (high - low) / 2;
            match probe(graph, middle) {
                Ok(Some(hit)) => {
                    best = Some(hit);
                    low = middle + 1;
                }
                Ok(None) => {
                    if middle == 0 {
                        break;
                    }
                    high = middle - 1;
                }
                Err(error) => return render_plan_failure(error),
            }
        }
    }
    let Some((notes, tokens)) = best else {
        return budget_too_small(budget, budget_variable);
    };
    match graph.finish(shown, &notes, tokens, budget, work) {
        Ok(rendered) => {
            debug_assert!(rendered.tokens <= budget);
            ToolResponse::text(rendered.text)
        }
        Err(error) => render_plan_failure(error),
    }
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

/// Emits a body-less result. An empty match set is exactly when unreachable
/// paths matter most, so the skip report travels with it.
fn status_response(
    status: String,
    report: &SkipReport,
    budget: usize,
    budget_variable: &str,
    operation: Option<&OperationCtx>,
) -> ToolResponse {
    let mut graph = match LineRenderGraph::new(
        Vec::new(),
        operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint),
    ) {
        Ok(graph) => graph,
        Err(error) => return render_plan_failure(error),
    };
    finish_with_skips(
        &mut graph,
        0,
        &status,
        report,
        budget,
        budget_variable,
        operation,
    )
}

fn render_plan_failure(error: RenderPlanError) -> ToolResponse {
    if error.is_cancelled() {
        ToolResponse::error("Request cancelled.")
    } else {
        ToolResponse::error(format!("Internal glob rendering failure: {error}"))
    }
}

fn budget_too_small(budget: usize, budget_variable: &str) -> ToolResponse {
    ErrorBudgetAdapter::new(budget, budget_variable).error(
        ErrorClass::Budget,
        format!(
            "{budget_variable}={budget} is too small to return the required glob truncation note. Increase it and retry."
        ),
    )
}
