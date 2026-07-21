//! Project filtering, deterministic ordering, and resumable paging for the glob tool.

use crate::bounded_sort::sort_cancelable;
use crate::budget::{
    ErrorBudgetAdapter, ErrorClass, GLOB_TOKEN_BUDGET_ENV, error_budget_hint, tool_token_budget,
};
use crate::file_executor::GrepGlobExecutor;
use crate::model::ToolResponse;
use crate::operation::{OpError, OperationCtx, RequestWorkGuard};
use crate::path_codec::{PathRecord, ResolvedRoot, RootRequirement, resolve_search_root};
use crate::render_plan::{LineRenderGraph, RenderPlanError};
use crate::traversal::{TraversalFailure, TraversalLimit, collect_walk_batched};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use std::fs;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
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
    path: PathRecord,
}

#[cfg(test)]
#[derive(Default)]
struct GlobCollectionProbe {
    metadata_lookups: AtomicUsize,
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
        #[cfg(test)]
        None,
    ))
}

fn glob_files_with_execution_unadapted(
    request: GlobRequest,
    budget: usize,
    budget_variable: &str,
    operation: &OperationCtx,
    executor: &Arc<GrepGlobExecutor>,
    #[cfg(test)] collection_probe: Option<&GlobCollectionProbe>,
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
    let matches = match collect_matches(
        &root,
        &matcher,
        request.filter_mode.unwrap_or_default(),
        sort,
        operation,
        executor,
        #[cfg(test)]
        collection_probe,
    ) {
        Ok(matches) => matches,
        Err(message) => return ToolResponse::error(message),
    };
    let matches = match sort_cancelable(
        matches,
        move |left, right| compare_match_entries(sort, left, right),
        Some(operation),
        Some(executor),
    ) {
        Ok(sorted) => sorted.items,
        Err(error) => return ToolResponse::error(error.to_string()),
    };
    format_matches(
        &matches,
        request.offset.unwrap_or(0),
        limit,
        budget,
        budget_variable,
        Some(operation),
        #[cfg(test)]
        None,
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
    root: &ResolvedRoot,
    matcher: &GlobSet,
    filter_mode: FilterMode,
    sort: SortMode,
    operation: &OperationCtx,
    executor: &Arc<GrepGlobExecutor>,
    #[cfg(test)] collection_probe: Option<&GlobCollectionProbe>,
) -> Result<Vec<MatchEntry>, String> {
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
            evaluate_match(
                root,
                entry,
                matcher,
                sort,
                #[cfg(test)]
                collection_probe,
            )
        },
    )
    .map(|collected| collected.items)
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
    matcher: &GlobSet,
    sort: SortMode,
    #[cfg(test)] collection_probe: Option<&GlobCollectionProbe>,
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
        #[cfg(test)]
        record_metadata_lookup(collection_probe);
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(TraversalFailure::from_io(path, &error)),
        }
    } else {
        #[cfg(test)]
        record_metadata_lookup(collection_probe);
        match entry.metadata() {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Ok(None),
            // Fall back to the plain stat so rare metadata failures keep the
            // exact error/skip semantics of the original lookup.
            Err(_) => {
                #[cfg(test)]
                record_metadata_lookup(collection_probe);
                match fs::metadata(path) {
                    Ok(metadata) if metadata.is_file() => metadata,
                    Ok(_) => return Ok(None),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(TraversalFailure::from_io(path, &error)),
                }
            }
        }
    };
    let record =
        PathRecord::from_metadata(path, &root.native, &metadata, sort == SortMode::Modified)
            .map_err(|error| TraversalFailure::from_io(path, &error))?;
    Ok(Some(MatchEntry { path: record }))
}

#[cfg(test)]
fn record_metadata_lookup(probe: Option<&GlobCollectionProbe>) {
    if let Some(probe) = probe {
        probe.metadata_lookups.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
fn ensure_result_capacity(current: usize) -> Result<(), String> {
    if current >= MAX_RESULTS {
        Err(TOO_MANY_MATCHES_ERROR.to_string())
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
    operation: Option<&OperationCtx>,
    #[cfg(test)] metrics_out: Option<&mut crate::render_plan::RenderPlanMetrics>,
) -> ToolResponse {
    let total = matches.len();
    if total == 0 {
        return status_response(
            "(Complete: no files matched.)".to_string(),
            budget,
            budget_variable,
            operation,
            #[cfg(test)]
            metrics_out,
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
            operation,
            #[cfg(test)]
            metrics_out,
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
    for shown in (1..=maximum).rev() {
        let terminal = glob_terminal(offset, shown, total);
        let notes = [terminal];
        let tokens = match graph.probe_notes(
            shown,
            &notes,
            operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint),
        ) {
            Ok(tokens) => tokens,
            Err(error) => return render_plan_failure(error),
        };
        if tokens <= budget {
            let rendered = match graph.finish(
                shown,
                &notes,
                tokens,
                budget,
                operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint),
            ) {
                Ok(rendered) => rendered,
                Err(error) => return render_plan_failure(error),
            };
            debug_assert!(rendered.tokens <= budget);
            #[cfg(test)]
            if let Some(metrics_out) = metrics_out {
                *metrics_out = graph.metrics();
            }
            return ToolResponse::text(rendered.text);
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

fn status_response(
    status: String,
    budget: usize,
    budget_variable: &str,
    operation: Option<&OperationCtx>,
    #[cfg(test)] metrics_out: Option<&mut crate::render_plan::RenderPlanMetrics>,
) -> ToolResponse {
    let mut graph = match LineRenderGraph::new(
        Vec::new(),
        operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint),
    ) {
        Ok(graph) => graph,
        Err(error) => return render_plan_failure(error),
    };
    let notes = [status];
    let tokens = match graph.probe_notes(
        0,
        &notes,
        operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint),
    ) {
        Ok(tokens) => tokens,
        Err(error) => return render_plan_failure(error),
    };
    if tokens > budget {
        return budget_too_small(budget, budget_variable);
    }
    let rendered = match graph.finish(
        0,
        &notes,
        tokens,
        budget,
        operation.map(|operation| operation as &dyn crate::operation::WorkCheckpoint),
    ) {
        Ok(rendered) => rendered,
        Err(error) => return render_plan_failure(error),
    };
    #[cfg(test)]
    if let Some(metrics_out) = metrics_out {
        *metrics_out = graph.metrics();
    }
    ToolResponse::text(rendered.text)
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

#[cfg(test)]
mod tests {
    use super::{
        FilterMode, GlobCollectionProbe, GlobRequest, MatchEntry, SortMode, build_matcher,
        collect_matches, ensure_result_capacity, format_matches,
        glob_files_with_execution_unadapted,
    };
    use crate::file_executor::{GrepGlobExecutor, LedgerSnapshot};
    use crate::operation::RequestWorkGuard;
    use crate::path_codec::{PathRecord, RootRequirement, resolve_search_root};
    use crate::render_plan::RenderPlanMetrics;
    use crate::{ToolContent, ToolResponse};
    use rmcp::model::RequestId;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::SystemTime;
    use tokio_util::sync::CancellationToken;

    fn match_entry(path: String) -> MatchEntry {
        let mut record = PathRecord::without_metadata(Path::new(&path), Path::new(""));
        record.modified = Some(SystemTime::UNIX_EPOCH);
        MatchEntry { path: record }
    }

    fn glob_with_parallelism(
        request: GlobRequest,
        parallelism: usize,
    ) -> (ToolResponse, LedgerSnapshot, LedgerSnapshot) {
        let (mut guard, operation) = RequestWorkGuard::new(
            RequestId::String(Arc::from(format!("glob-p{parallelism}"))),
            CancellationToken::new(),
        );
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(parallelism));
        let response = glob_files_with_execution_unadapted(
            request,
            100_000_000,
            "FASTCTX_TOKEN_BUDGET",
            &operation,
            &executor,
            None,
        );
        guard.disarm();
        executor.wait_for_test_quiescence();
        (
            response,
            executor.test_burst_ledger(),
            executor.test_ticket_ledger(),
        )
    }

    fn assert_released_once(ledger: LedgerSnapshot) {
        assert_eq!(ledger.allocated, ledger.released);
        assert_eq!(ledger.live, 0);
        assert_eq!(ledger.duplicate_releases, 0);
    }

    fn response_path_lines(response: &ToolResponse) -> Vec<String> {
        assert!(!response.is_error, "{response:?}");
        let [ToolContent::Text(text)] = response.content.as_slice() else {
            panic!("expected one text response");
        };
        let body = text
            .split_once("\n\n")
            .map_or(text.as_str(), |(body, _)| body);
        if body.starts_with('(') {
            Vec::new()
        } else {
            body.lines().map(str::to_string).collect()
        }
    }

    fn safe_fixture_display(path: &Path) -> String {
        let native = path.to_string_lossy();
        #[cfg(windows)]
        let native = native.strip_prefix(r"\\?\").unwrap_or(native.as_ref());
        native.replace('\\', "/")
    }

    #[test]
    fn token_budget_keeps_the_page_prefix_and_returns_an_exact_offset() {
        let matches = (1..=3)
            .map(|index| match_entry(format!("{index}-{}", "x".repeat(100))))
            .collect::<Vec<_>>();
        let response = format_matches(&matches, 0, 3, 55, "FASTCTX_TOKEN_BUDGET", None, None);
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
        let matches = vec![match_entry("/a/very/long/path.txt".to_string())];
        let response = format_matches(&matches, 0, 1, 1, "FASTCTX_TOKEN_BUDGET", None, None);
        assert!(response.is_error);
        let [ToolContent::Text(text)] = response.content.as_slice() else {
            panic!("expected one text error");
        };
        assert!(
            tiktoken_rs::o200k_base_singleton()
                .encode_ordinary(text)
                .len()
                <= 1
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

    #[test]
    fn render_work_and_full_tokenization_are_linear_at_every_public_limit() {
        let matches = (0..1_000)
            .map(|index| match_entry(format!("/root/{index:04}.txt")))
            .collect::<Vec<_>>();

        for limit in [100, 250, 500, 1_000] {
            let mut metrics = RenderPlanMetrics::default();
            let response = format_matches(
                &matches,
                0,
                limit,
                usize::MAX,
                "FASTCTX_TOKEN_BUDGET",
                None,
                Some(&mut metrics),
            );
            assert!(!response.is_error, "{response:?}");
            assert_eq!(metrics.render_units_built, limit);
            assert_eq!(metrics.full_tokenizer_calls, 1);
            assert_eq!(metrics.token_suffix_probes, 1);
            assert!(metrics.token_prefix_appends <= limit * 2);
            assert_eq!(
                metrics.render_bytes_built,
                matches[..limit]
                    .iter()
                    .map(|entry| entry.path.display.len())
                    .sum::<usize>()
            );
        }
    }

    #[test]
    fn glob_filter_runs_before_metadata_and_path_sort_avoids_mtime_stat() {
        let fixture = tempfile::tempdir().unwrap();
        fs::File::create(fixture.path().join("selected.txt")).unwrap();
        for index in 0..512 {
            fs::File::create(fixture.path().join(format!("ignored-{index:03}.bin"))).unwrap();
        }
        let root_input = fixture.path().to_string_lossy().into_owned();
        let root = resolve_search_root(Some(&root_input), RootRequirement::Directory).unwrap();
        let matcher = build_matcher("*.txt").unwrap();
        let (mut guard, operation) = RequestWorkGuard::new(
            RequestId::String(Arc::from("glob-filter-before-metadata")),
            CancellationToken::new(),
        );
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(4));

        let path_probe = GlobCollectionProbe::default();
        let path_matches = collect_matches(
            &root,
            &matcher,
            FilterMode::All,
            SortMode::Path,
            &operation,
            &executor,
            Some(&path_probe),
        )
        .unwrap();
        assert_eq!(path_matches.len(), 1);
        assert_eq!(path_probe.metadata_lookups.load(Ordering::Relaxed), 0);

        let modified_probe = GlobCollectionProbe::default();
        let modified_matches = collect_matches(
            &root,
            &matcher,
            FilterMode::All,
            SortMode::Modified,
            &operation,
            &executor,
            Some(&modified_probe),
        )
        .unwrap();
        assert_eq!(modified_matches.len(), 1);
        assert_eq!(modified_probe.metadata_lookups.load(Ordering::Relaxed), 1);

        guard.disarm();
        executor.wait_for_test_quiescence();
    }

    #[test]
    fn p1_and_p4_pages_match_an_independent_full_sort_without_gaps_or_duplicates() {
        let fixture = tempfile::tempdir().unwrap();
        let mut created = Vec::new();
        for directory_index in 0..17 {
            let directory = fixture.path().join(format!("batch-{directory_index:02}"));
            fs::create_dir(&directory).unwrap();
            for file_index in 0..247 {
                let path = directory.join(format!("item-{file_index:03}.txt"));
                fs::File::create(&path).unwrap();
                let modified = fs::metadata(&path).unwrap().modified().unwrap();
                created.push((
                    modified,
                    safe_fixture_display(&fs::canonicalize(&path).unwrap()),
                ));
            }
        }

        for sort in [SortMode::Path, SortMode::Modified] {
            let mut oracle = created.clone();
            match sort {
                SortMode::Path => {
                    oracle.sort_by(|left, right| left.1.as_bytes().cmp(right.1.as_bytes()))
                }
                SortMode::Modified => oracle.sort_by(|left, right| {
                    right
                        .0
                        .cmp(&left.0)
                        .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
                }),
            }
            let oracle = oracle
                .into_iter()
                .map(|(_, display)| display)
                .collect::<Vec<_>>();
            let mut reconstructed = Vec::new();
            for offset in (0..oracle.len()).step_by(1_000) {
                let request = GlobRequest {
                    pattern: "**/*.txt".to_string(),
                    path: Some(fixture.path().to_string_lossy().into_owned()),
                    filter_mode: Some(FilterMode::All),
                    sort: Some(sort),
                    offset: Some(offset),
                    limit: Some(1_000),
                };
                let (serial, serial_burst, serial_tickets) =
                    glob_with_parallelism(request.clone(), 1);
                let (parallel, parallel_burst, parallel_tickets) =
                    glob_with_parallelism(request, 4);
                assert_eq!(parallel, serial);
                let lines = response_path_lines(&parallel);
                let end = (offset + 1_000).min(oracle.len());
                assert_eq!(lines, oracle[offset..end]);
                reconstructed.extend(lines);
                for ledger in [
                    serial_burst,
                    serial_tickets,
                    parallel_burst,
                    parallel_tickets,
                ] {
                    assert_released_once(ledger);
                }
            }
            assert_eq!(reconstructed, oracle);

            let arbitrary_offset = 113;
            let arbitrary_limit = 257;
            let arbitrary = GlobRequest {
                pattern: "**/*.txt".to_string(),
                path: Some(fixture.path().to_string_lossy().into_owned()),
                filter_mode: Some(FilterMode::All),
                sort: Some(sort),
                offset: Some(arbitrary_offset),
                limit: Some(arbitrary_limit),
            };
            let (serial, _, _) = glob_with_parallelism(arbitrary.clone(), 1);
            let (parallel, burst, tickets) = glob_with_parallelism(arbitrary, 4);
            assert_eq!(parallel, serial);
            assert_eq!(
                response_path_lines(&parallel),
                oracle[arbitrary_offset..arbitrary_offset + arbitrary_limit]
            );
            assert_released_once(burst);
            assert_released_once(tickets);
        }
    }
}
