//! Explicit ignore policy, deterministic ordering, metadata rendering, and resumable paging.

use crate::bounded_sort::sort_cancelable;
use crate::budget::{
    ErrorBudgetAdapter, ErrorClass, GLOB_TOKEN_BUDGET_ENV, error_budget_hint, tool_token_budget,
};
use crate::file_executor::GrepGlobExecutor;
use crate::glob_filter::{GlobPatterns, PathGlobFilter};
use crate::head_note::{CoverageTotal, CoveredRange, HeadMetric, HeadNote};
use crate::model::ToolResponse;
use crate::operation::{OpError, OperationCtx, RequestWorkGuard};
use crate::path_codec::{PathRecord, ResolvedRoot, RootRequirement, resolve_search_root};
use crate::render_plan::{LineRenderGraph, RenderPlanError};
use crate::skip_report::{SkipTally, detail_line};
use crate::traversal::{
    SkippedPaths, TraversalCollection, TraversalFailure, TraversalLimit, collect_walk_batched,
};
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::sync::Arc;
use std::time::SystemTime;
use time::{OffsetDateTime, macros::format_description};
use tokio_util::sync::CancellationToken;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1_000;
const MAX_RESULTS: usize = 100_000;
const TOO_MANY_MATCHES_ERROR: &str =
    "Too many matches: over 100000 files matched. Narrow the pattern or path.";

/// Ignore-file policy used by glob traversal.
#[derive(Clone, Copy, Debug, Default, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    /// Respect plain `.ignore` files while keeping every other file visible.
    #[default]
    Ignore,
    /// Disable plain `.ignore` filtering.
    All,
}

impl<'de> Deserialize<'de> for FilterMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "ignore" | "project" => Ok(Self::Ignore),
            "all" => Ok(Self::All),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["ignore", "all"],
            )),
        }
    }
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

/// Presentation used for each matched file.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GlobOutputMode {
    /// Return only the canonical model-facing path.
    #[default]
    Paths,
    /// Return one compact JSON object with path, byte size, and UTC modification time.
    Details,
}

/// Parameters for the glob tool.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct GlobRequest {
    /// Globs to match files, e.g. ["**/*.rs", "!tests/**"]. A leading `!` excludes and always wins; negative-only patterns list every other file.
    // Published as a plain string array rather than the string-or-array union the type
    // accepts. A union is the one construct no provider subset takes: Gemini rejects a
    // node that carries `anyOf` beside a `description`, and every parameter here has
    // one. Deserialization is unchanged — a bare string still parses. (2026-08-17)
    #[schemars(with = "Vec<String>")]
    pub pattern: GlobPatterns,
    /// Directory to search; omit for the session working directory.
    #[schemars(description = crate::model_guidance::local_path_description(
        "Directory to search. Omit for the session working directory; when provided, it must name an existing directory."
    ))]
    pub path: Option<String>,
    /// "ignore" respects only plain .ignore files; "all" disables that filtering. Both include hidden files and .git, and neither reads any Git ignore source. The legacy value "project" is accepted as "ignore" but is not published.
    pub filter_mode: Option<FilterMode>,
    /// "path" = byte-order path sort. "modified" = most recently modified first.
    pub sort: Option<SortMode>,
    /// "paths" (default) returns one path per line. "details" returns one compact JSON object per line as {"path":"...","bytes":123,"modified":"YYYY-MM-DDTHH:MM:SS.NNNNNNNNNZ"}.
    pub output_mode: Option<GlobOutputMode>,
    /// Skip the first N results — for paging.
    pub offset: Option<usize>,
    /// Max results per page (1-1000).
    #[schemars(range(min = 1, max = 1_000))]
    pub limit: Option<usize>,
}

#[derive(Debug, Eq, PartialEq)]
struct MatchEntry {
    path: PathRecord,
    rendered: Arc<str>,
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
    let output_mode = request.output_mode.unwrap_or_default();
    let collected = match collect_matches(
        &root,
        &matcher,
        request.filter_mode.unwrap_or_default(),
        sort,
        output_mode,
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
    output_mode: GlobOutputMode,
    operation: &OperationCtx,
    executor: &Arc<GrepGlobExecutor>,
) -> Result<TraversalCollection<MatchEntry>, String> {
    if operation.check().is_err() {
        return Err("Request cancelled.".to_string());
    }
    let mut builder = WalkBuilder::new(&root.native);
    match filter_mode {
        FilterMode::Ignore => {
            builder
                .standard_filters(false)
                .parents(true)
                .hidden(false)
                .ignore(true)
                .git_ignore(false)
                .git_global(false)
                .git_exclude(false)
                .follow_links(false);
        }
        FilterMode::All => {
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
            evaluate_match(root, entry, matcher, sort, output_mode)
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
    output_mode: GlobOutputMode,
) -> Result<Option<MatchEntry>, TraversalFailure> {
    let path = entry.path();
    let preliminary = PathRecord::without_metadata(path, &root.native);
    if !matcher.is_match(preliminary.relative_match.as_ref()) {
        return Ok(None);
    }
    if sort == SortMode::Path
        && output_mode == GlobOutputMode::Paths
        && entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
    {
        let rendered = Arc::clone(&preliminary.display);
        return Ok(Some(MatchEntry {
            path: preliminary,
            rendered,
        }));
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
    let include_modified = sort == SortMode::Modified || output_mode == GlobOutputMode::Details;
    let record = PathRecord::from_metadata(path, &root.native, &metadata, include_modified)
        .map_err(|error| TraversalFailure::from_io(path, &error))?;
    let rendered = if output_mode == GlobOutputMode::Details {
        format_match_details(&record).map_err(|error| TraversalFailure::from_io(path, &error))?
    } else {
        Arc::clone(&record.display)
    };
    Ok(Some(MatchEntry {
        path: record,
        rendered,
    }))
}

#[derive(Serialize)]
struct MatchDetails<'a> {
    path: &'a str,
    bytes: u64,
    modified: &'a str,
}

fn format_match_details(record: &PathRecord) -> io::Result<Arc<str>> {
    let bytes = record
        .traversal_len_hint
        .ok_or_else(|| io::Error::other("glob detail metadata is missing the file size"))?;
    let modified = record
        .modified
        .ok_or_else(|| io::Error::other("glob detail metadata is missing the modification time"))?;
    let modified = format_modified_utc(modified)?;
    serde_json::to_string(&MatchDetails {
        path: &record.display,
        bytes,
        modified: &modified,
    })
    .map(Arc::from)
    .map_err(|error| io::Error::other(format!("cannot serialize glob details: {error}")))
}

fn format_modified_utc(value: SystemTime) -> io::Result<String> {
    let nanoseconds = match value.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => {
            i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos())
        }
        Err(error) => {
            let duration = error.duration();
            -(i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos()))
        }
    };
    OffsetDateTime::from_unix_timestamp_nanos(nanoseconds)
        .map_err(|error| {
            io::Error::other(format!("file modification time is out of range: {error}"))
        })?
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]Z"
        ))
        .map_err(|error| io::Error::other(format!("cannot format file modification time: {error}")))
}

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

    fn head(&self, metric: HeadMetric, shown_details: usize) -> String {
        let mut note = HeadNote::new("glob", metric);
        if let Some(fact) = self.tally.fact(shown_details) {
            note = note.fact(fact);
        }
        note.render()
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
            HeadMetric::count(0, "file", "files"),
            report,
            budget,
            budget_variable,
            operation,
        );
    }
    if offset >= total {
        return status_response(
            HeadMetric::event(format!(
                "0 files; {total} {} exist",
                if total == 1 { "file" } else { "files" }
            )),
            report,
            budget,
            budget_variable,
            operation,
        );
    }

    let maximum = limit.min(total - offset);
    let lines = matches[offset..offset + maximum]
        .iter()
        .map(|entry| Arc::clone(&entry.rendered))
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
        let metric = glob_metric(offset, shown, total);
        let head = report.head(metric.clone(), 0);
        let tokens = match graph.probe_head(
            shown,
            &head,
            &[] as &[Arc<str>],
            operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint),
        ) {
            Ok(tokens) => tokens,
            Err(error) => return render_plan_failure(error),
        };
        if tokens <= budget {
            return finish_with_skips(
                &mut graph,
                shown,
                metric,
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
    metric: HeadMetric,
    report: &SkipReport,
    budget: usize,
    budget_variable: &str,
    operation: Option<&OperationCtx>,
) -> ToolResponse {
    let work = operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint);
    let probe = |graph: &mut LineRenderGraph,
                 details: usize|
     -> Result<Option<(usize, String, usize)>, RenderPlanError> {
        let head = report.head(metric.clone(), details);
        let tokens = graph.probe_head(shown, &head, &report.details[..details], work)?;
        Ok((tokens <= budget).then_some((details, head, tokens)))
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
    let Some((shown_details, head, tokens)) = best else {
        return budget_too_small(budget, budget_variable);
    };
    match graph.finish_head(
        shown,
        &head,
        &report.details[..shown_details],
        tokens,
        budget,
        work,
    ) {
        Ok(rendered) => {
            debug_assert!(rendered.tokens <= budget);
            ToolResponse::text(rendered.text)
        }
        Err(error) => render_plan_failure(error),
    }
}

fn glob_metric(offset: usize, shown: usize, total: usize) -> HeadMetric {
    HeadMetric::Coverage {
        unit: "files",
        ranges: vec![CoveredRange::new(offset + 1, offset + shown)],
        total: CoverageTotal::Exact(total),
    }
}

/// Emits a body-less result. An empty match set is exactly when unreachable
/// paths matter most, so the skip report travels with it.
fn status_response(
    metric: HeadMetric,
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
        metric,
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
            "{budget_variable}={budget} is too small to return the glob head note and one result. That budget is fixed for this session; retrying cannot raise it."
        ),
    )
}
