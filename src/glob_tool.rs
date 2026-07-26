//! Project filtering, deterministic ordering, and resumable paging for the glob tool.

use crate::bounded_sort::sort_cancelable;
use crate::budget::{
    ErrorBudgetAdapter, ErrorClass, GLOB_TOKEN_BUDGET_ENV, error_budget_hint, tool_token_budget,
};
use crate::file_executor::GrepGlobExecutor;
use crate::model::ToolResponse;
use crate::operation::{OpError, OperationCtx, RequestWorkGuard};
use crate::path_codec::{
    PathRecord, ResolvedRoot, RootRequirement, resolve_search_root_with_scope,
};
use crate::paths::ReadScope;
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
    glob_files_with_scope(request, cancellation, &ReadScope::unrestricted())
}

pub(crate) fn glob_files_with_scope(
    request: GlobRequest,
    cancellation: CancellationToken,
    scope: &ReadScope,
) -> ToolResponse {
    let (mut guard, operation) = RequestWorkGuard::new(
        rmcp::model::RequestId::String(Arc::from("direct-glob")),
        cancellation,
    );
    let response =
        glob_files_with_execution_scoped(request, operation, GrepGlobExecutor::shared(), scope);
    guard.disarm();
    response
}

fn glob_files_with_execution_scoped(
    request: GlobRequest,
    operation: OperationCtx,
    executor: Arc<GrepGlobExecutor>,
    scope: &ReadScope,
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
        scope,
    ))
}

fn glob_files_with_execution_unadapted(
    request: GlobRequest,
    budget: usize,
    budget_variable: &str,
    operation: &OperationCtx,
    executor: &Arc<GrepGlobExecutor>,
    #[cfg(test)] collection_probe: Option<&GlobCollectionProbe>,
    scope: &ReadScope,
) -> ToolResponse {
    if operation.check().is_err() {
        return ToolResponse::error("Request cancelled.");
    }
    let root = match resolve_search_root_with_scope(
        request.path.as_deref(),
        RootRequirement::Directory,
        scope,
    ) {
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
    scope: ReadScope,
    request: GlobRequest,
) -> Result<ToolResponse, OpError> {
    let work = operation.inline_work();
    work.check_inline()?;
    let response = glob_files_with_execution_scoped(request, operation.clone(), executor, &scope);
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
    if root.scope.is_restricted() {
        let candidates = crate::traversal::collect_capability_candidates_filtered(
            root,
            Some(matcher),
            None,
            Some(operation),
            crate::traversal::CapabilityCandidateOptions {
                honor_ignore: filter_mode == FilterMode::Project,
                detail: if sort == SortMode::Path {
                    crate::traversal::CandidateDetail::Path
                } else {
                    crate::traversal::CandidateDetail::Metadata
                },
                limit: Some(MAX_RESULTS),
                limit_message: TOO_MANY_MATCHES_ERROR,
            },
        )?;
        return Ok(candidates
            .into_iter()
            .map(|path| MatchEntry { path })
            .collect());
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
        glob_files_with_execution_unadapted, glob_files_with_scope,
    };
    use crate::file_executor::{GrepGlobExecutor, LedgerSnapshot};
    use crate::operation::RequestWorkGuard;
    use crate::path_codec::display_path as search_display_path;
    use crate::path_codec::{PathRecord, RootRequirement, resolve_search_root};
    use crate::paths::ReadScope;
    use crate::render_plan::RenderPlanMetrics;
    use crate::{ToolContent, ToolResponse};
    use rmcp::model::RequestId;
    use std::env;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
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
            &ReadScope::unrestricted(),
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

    fn restricted_project_glob(root: &Path) -> ToolResponse {
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root.to_path_buf())).unwrap();
        glob_files_with_scope(
            GlobRequest {
                pattern: "*.txt".to_string(),
                path: Some(root.to_string_lossy().into_owned()),
                filter_mode: Some(FilterMode::Project),
                sort: Some(SortMode::Path),
                offset: None,
                limit: None,
            },
            CancellationToken::new(),
            &scope,
        )
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

    #[test]
    fn restricted_project_walk_reads_worktree_excludes_from_commondir_or_gitdir() {
        for (name, gitdir_relative, commondir, exclude_relative) in [
            (
                "relative commondir",
                "../common/.git/worktrees/checked-out",
                Some("../.."),
                "../common/.git/info/exclude",
            ),
            (
                "gitdir fallback",
                "../gitdir",
                None,
                "../gitdir/info/exclude",
            ),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("worktree");
            let gitdir = root.join(gitdir_relative);
            let exclude = root.join(exclude_relative);
            fs::create_dir(&root).unwrap();
            fs::create_dir_all(&gitdir).unwrap();
            fs::create_dir_all(exclude.parent().unwrap()).unwrap();
            fs::write(root.join(".git"), format!("gitdir: {gitdir_relative}\n")).unwrap();
            if let Some(commondir) = commondir {
                fs::write(gitdir.join("commondir"), format!("{commondir}\n")).unwrap();
            }
            fs::write(exclude, "excluded.txt\n").unwrap();
            fs::write(root.join("excluded.txt"), b"excluded").unwrap();
            fs::write(root.join("visible.txt"), b"visible").unwrap();

            let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
            let restricted = glob_files_with_scope(
                GlobRequest {
                    pattern: "*.txt".to_string(),
                    path: Some(root.to_string_lossy().into_owned()),
                    filter_mode: Some(FilterMode::Project),
                    sort: Some(SortMode::Path),
                    offset: None,
                    limit: None,
                },
                CancellationToken::new(),
                &scope,
            );
            let text = response_path_lines(&restricted).join("\n");
            assert!(text.contains("visible.txt"), "{name}: {text}");
            assert!(!text.contains("excluded.txt"), "{name}: {text}");
        }
    }

    #[test]
    fn malformed_parent_ignore_is_not_disclosed_and_later_rule_still_filters() {
        let fixture = tempfile::tempdir().unwrap();
        let parent = fixture.path().join("parent");
        let root = parent.join("root");
        fs::create_dir_all(&root).unwrap();
        let secret = "synthetic-secret-ignore-pattern";
        fs::write(parent.join(".ignore"), format!("[{secret}\nhidden.txt\n")).unwrap();
        fs::write(root.join("hidden.txt"), b"hidden").unwrap();
        fs::write(root.join("visible.txt"), b"visible").unwrap();

        let response = restricted_project_glob(&root);
        let ToolContent::Text(text) = &response.content[0] else {
            panic!("expected text response: {response:?}");
        };
        assert!(!text.contains(secret), "{text}");
        let paths = response_path_lines(&response);
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(paths[0].ends_with("visible.txt"), "{paths:?}");
    }

    #[test]
    fn restricted_glob_accepts_ignore_config_at_the_size_limit() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("root");
        fs::create_dir(&root).unwrap();
        let rule = "hidden.txt\n";
        let contents = format!(
            "{rule}#{}",
            "x".repeat(crate::traversal::MAX_IGNORE_CONFIG_BYTES - rule.len() - 1)
        );
        assert_eq!(contents.len(), crate::traversal::MAX_IGNORE_CONFIG_BYTES);
        fs::write(root.join(".ignore"), contents).unwrap();
        fs::write(root.join("hidden.txt"), b"hidden").unwrap();
        fs::write(root.join("visible.txt"), b"visible").unwrap();

        let paths = response_path_lines(&restricted_project_glob(&root));
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(paths[0].ends_with("visible.txt"), "{paths:?}");
    }

    #[test]
    fn restricted_glob_rejects_oversized_ignore_without_partial_matching_or_disclosure() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("root");
        fs::create_dir(&root).unwrap();
        let secret = "synthetic-secret-ignore-pattern";
        let prefix = "hidden.txt\n#";
        let contents = format!(
            "{prefix}{}{secret}",
            "x".repeat(crate::traversal::MAX_IGNORE_CONFIG_BYTES + 1 - prefix.len() - secret.len())
        );
        assert_eq!(
            contents.len(),
            crate::traversal::MAX_IGNORE_CONFIG_BYTES + 1
        );
        fs::write(root.join(".ignore"), contents).unwrap();
        fs::write(root.join("hidden.txt"), b"hidden").unwrap();
        fs::write(root.join("visible.txt"), b"visible").unwrap();

        let response = restricted_project_glob(&root);
        assert!(response.is_error, "{response:?}");
        let ToolContent::Text(text) = &response.content[0] else {
            panic!("expected text error: {response:?}");
        };
        assert_eq!(text, "Ignore configuration exceeds maximum size.");
        for forbidden in [secret, "hidden.txt", "visible.txt"] {
            assert!(!text.contains(forbidden), "{text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn restricted_glob_lists_unreadable_regular_files_without_opening_them() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("unreadable.txt");
        fs::write(&path, b"content").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let scope = ReadScope::from_allow_roots(&[fixture.path().to_path_buf()]).unwrap();
        let request = GlobRequest {
            pattern: "*.txt".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            filter_mode: Some(FilterMode::All),
            sort: Some(SortMode::Modified),
            offset: None,
            limit: None,
        };
        let restricted = glob_files_with_scope(request.clone(), CancellationToken::new(), &scope);
        let unrestricted = glob_files_with_scope(
            request,
            CancellationToken::new(),
            &ReadScope::unrestricted(),
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(restricted, unrestricted);
        assert!(response_path_lines(&restricted)[0].ends_with("unreadable.txt"));
    }

    #[test]
    fn restricted_project_walk_matches_walkbuilder_matrix() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        let project = workspace.join("project");
        let nested = project.join("nested");
        let nested_repo = project.join("nested-repo");
        fs::create_dir_all(workspace.join(".git/info")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(nested_repo.join(".git")).unwrap();

        fs::write(
            workspace.join(".gitignore"),
            b"\xEF\xBB\xBF/project/parent-anchored.txt\n/project/nested-repo/outer-must-not-apply.txt\n",
        )
        .unwrap();
        fs::write(workspace.join(".ignore"), "/project/parent-ignore.txt\n").unwrap();
        fs::write(
            workspace.join(".git/info/exclude"),
            "/project/project-exclude.txt\n",
        )
        .unwrap();
        fs::write(
            project.join(".gitignore"),
            "/child-anchored.txt\ntier.txt\nblocked/\n!blocked/keep.txt\n",
        )
        .unwrap();
        fs::write(project.join(".ignore"), "!tier.txt\n").unwrap();
        fs::write(nested.join(".gitignore"), "/nested-only.txt\n").unwrap();
        fs::write(nested_repo.join(".gitignore"), "nested-repo-hidden.txt\n").unwrap();

        for path in [
            project.join("parent-anchored.txt"),
            project.join("child-anchored.txt"),
            project.join("project-exclude.txt"),
            project.join("parent-ignore.txt"),
            project.join("tier.txt"),
            project.join(".hidden.txt"),
            project.join("blocked/keep.txt"),
            nested.join("nested-only.txt"),
            nested_repo.join("outer-must-not-apply.txt"),
            nested_repo.join("nested-repo-hidden.txt"),
            nested_repo.join("visible.txt"),
        ] {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, b"fixture").unwrap();
        }

        let request = |filter_mode| GlobRequest {
            pattern: "**/*".to_string(),
            path: Some(project.to_string_lossy().into_owned()),
            filter_mode: Some(filter_mode),
            sort: Some(SortMode::Path),
            offset: None,
            limit: None,
        };
        for allow_roots in [
            vec![workspace.clone()],
            vec![workspace.clone(), project.clone()],
        ] {
            let scope = ReadScope::from_allow_roots(&allow_roots).unwrap();
            for filter_mode in [FilterMode::Project, FilterMode::All] {
                let restricted =
                    glob_files_with_scope(request(filter_mode), CancellationToken::new(), &scope);
                let unrestricted = glob_files_with_scope(
                    request(filter_mode),
                    CancellationToken::new(),
                    &ReadScope::unrestricted(),
                );
                assert_eq!(restricted, unrestricted, "{filter_mode:?}, {allow_roots:?}");
            }
        }

        let scope = ReadScope::from_allow_roots(&[workspace, project.clone()]).unwrap();
        let project_result = glob_files_with_scope(
            request(FilterMode::Project),
            CancellationToken::new(),
            &scope,
        );
        let project_text = response_path_lines(&project_result).join("\n");
        for visible in [
            "tier.txt",
            "outer-must-not-apply.txt",
            "visible.txt",
            ".hidden.txt",
        ] {
            assert!(project_text.contains(visible), "{project_text}");
        }
        for hidden in [
            "parent-anchored.txt",
            "parent-ignore.txt",
            "child-anchored.txt",
            "project-exclude.txt",
            "blocked/keep.txt",
            "nested-only.txt",
            "nested-repo-hidden.txt",
        ] {
            assert!(!project_text.contains(hidden), "{project_text}");
        }
    }

    #[test]
    fn restricted_modified_sort_matches_unrestricted() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("root");
        fs::create_dir(&root).unwrap();
        let older = root.join("older.txt");
        let newer = root.join("newer.txt");
        fs::write(&older, b"old").unwrap();
        fs::write(&newer, b"new").unwrap();
        filetime::set_file_mtime(&older, filetime::FileTime::from_unix_time(1, 0)).unwrap();
        filetime::set_file_mtime(&newer, filetime::FileTime::from_unix_time(2, 0)).unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let request = GlobRequest {
            pattern: "*.txt".to_string(),
            path: Some(root.to_string_lossy().into_owned()),
            filter_mode: Some(FilterMode::All),
            sort: Some(SortMode::Modified),
            offset: None,
            limit: None,
        };
        let restricted = glob_files_with_scope(request.clone(), CancellationToken::new(), &scope);
        let unrestricted = glob_files_with_scope(
            request,
            CancellationToken::new(),
            &ReadScope::unrestricted(),
        );
        assert_eq!(restricted, unrestricted);
        let lines = response_path_lines(&restricted);
        assert!(lines[0].ends_with("newer.txt"), "{lines:?}");
    }

    #[test]
    fn restricted_walk_is_iterative_and_stops_at_the_candidate_limit() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("root");
        fs::create_dir(&root).unwrap();
        let mut deep = root.clone();
        for _ in 0..256 {
            deep.push("nested");
            fs::create_dir(&deep).unwrap();
        }
        fs::write(deep.join("leaf.txt"), b"leaf").unwrap();
        fs::write(root.join("first.txt"), b"first").unwrap();
        fs::write(root.join("second.txt"), b"second").unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let request = GlobRequest {
            pattern: "**/*.txt".to_string(),
            path: Some(root.to_string_lossy().into_owned()),
            filter_mode: Some(FilterMode::All),
            sort: Some(SortMode::Path),
            offset: None,
            limit: None,
        };
        let restricted = glob_files_with_scope(request.clone(), CancellationToken::new(), &scope);
        let unrestricted = glob_files_with_scope(
            request,
            CancellationToken::new(),
            &ReadScope::unrestricted(),
        );
        assert_eq!(restricted, unrestricted);

        let resolved = crate::path_codec::resolve_search_root_with_scope(
            Some(&root.to_string_lossy()),
            RootRequirement::Directory,
            &scope,
        )
        .unwrap();
        let error = crate::traversal::collect_capability_candidates_filtered(
            &resolved,
            None,
            None,
            None,
            crate::traversal::CapabilityCandidateOptions {
                honor_ignore: false,
                detail: crate::traversal::CandidateDetail::Path,
                limit: Some(1),
                limit_message: "candidate cap reached",
            },
        )
        .unwrap_err();
        assert_eq!(error, "candidate cap reached");
    }

    #[test]
    fn restricted_global_ignore_matches_unrestricted_in_isolated_subprocess() {
        if env::var_os("FASTCTX_GLOBAL_IGNORE_CHILD").is_some() {
            let global =
                std::path::PathBuf::from(env::var_os("FASTCTX_GLOBAL_IGNORE_PATH").unwrap());
            fs::write(&global, "globally-hidden.txt\n").unwrap();
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("project");
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join(".git")).unwrap();
            fs::write(root.join("globally-hidden.txt"), b"hidden").unwrap();
            fs::write(root.join("visible.txt"), b"visible").unwrap();
            let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
            let request = |filter_mode| GlobRequest {
                pattern: "**/*".to_string(),
                path: Some(root.to_string_lossy().into_owned()),
                filter_mode: Some(filter_mode),
                sort: Some(SortMode::Path),
                offset: None,
                limit: None,
            };
            let project = glob_files_with_scope(
                request(FilterMode::Project),
                CancellationToken::new(),
                &scope,
            );
            let unrestricted = glob_files_with_scope(
                request(FilterMode::Project),
                CancellationToken::new(),
                &ReadScope::unrestricted(),
            );
            let all =
                glob_files_with_scope(request(FilterMode::All), CancellationToken::new(), &scope);
            assert_eq!(project, unrestricted);
            let ToolContent::Text(project) = &project.content[0] else {
                panic!("{project:?}")
            };
            let ToolContent::Text(all) = &all.content[0] else {
                panic!("{all:?}")
            };
            assert!(!project.contains("globally-hidden.txt"), "{project}");
            assert!(all.contains("globally-hidden.txt"), "{all}");
            return;
        }
        let fixture = tempfile::tempdir().unwrap();
        let config = fixture.path().join("gitconfig");
        let global = fixture.path().join("global-ignore");
        fs::write(
            &config,
            format!("[core]\n\texcludesFile = {}\n", global.display()),
        )
        .unwrap();
        let status = Command::new(env::current_exe().unwrap())
            .arg("--exact")
            .arg("glob_tool::tests::restricted_global_ignore_matches_unrestricted_in_isolated_subprocess")
            .env("FASTCTX_GLOBAL_IGNORE_CHILD", "1")
            .env("FASTCTX_GLOBAL_IGNORE_PATH", &global)
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("HOME", fixture.path())
            .env("XDG_CONFIG_HOME", fixture.path().join("xdg"))
            .status()
            .unwrap();
        assert!(status.success(), "global-ignore child failed: {status}");
    }

    #[cfg(unix)]
    #[test]
    fn restricted_glob_keeps_stable_file_symlinks_in_configured_roots() {
        let fixture = tempfile::tempdir().unwrap();
        let left = fixture.path().join("left");
        let right = fixture.path().join("right");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        fs::write(left.join("same-target.txt"), b"same").unwrap();
        fs::write(right.join("cross-target.txt"), b"cross").unwrap();
        std::os::unix::fs::symlink("same-target.txt", left.join("same-link.txt")).unwrap();
        std::os::unix::fs::symlink(right.join("cross-target.txt"), left.join("cross-link.txt"))
            .unwrap();
        let scope = ReadScope::from_allow_roots(&[left.clone(), right]).unwrap();
        let request = GlobRequest {
            pattern: "*.txt".to_string(),
            path: Some(left.to_string_lossy().into_owned()),
            filter_mode: Some(FilterMode::All),
            sort: Some(SortMode::Path),
            offset: None,
            limit: None,
        };
        let restricted = glob_files_with_scope(request.clone(), CancellationToken::new(), &scope);
        let unrestricted = glob_files_with_scope(
            request,
            CancellationToken::new(),
            &ReadScope::unrestricted(),
        );
        assert_eq!(restricted, unrestricted);
        let paths = response_path_lines(&restricted);
        assert!(
            paths.iter().any(|path| path.ends_with("same-link.txt")),
            "{paths:?}"
        );
        assert!(
            paths.iter().any(|path| path.ends_with("cross-link.txt")),
            "{paths:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn scoped_glob_denies_recursive_file_symlink_without_returning_target_path() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        let outside = fixture.path().join("outside.txt");
        fs::create_dir(&root).unwrap();
        fs::File::create(&outside).unwrap();
        let link = root.join("outside-link.txt");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let response = glob_files_with_scope(
            GlobRequest {
                pattern: "**/*".to_string(),
                path: Some(root.to_string_lossy().into_owned()),
                filter_mode: Some(FilterMode::All),
                sort: Some(SortMode::Path),
                offset: None,
                limit: None,
            },
            CancellationToken::new(),
            &scope,
        );
        assert!(response.is_error, "{response:?}");
        let ToolContent::Text(text) = &response.content[0] else {
            panic!("expected text error");
        };
        assert!(text.starts_with("Permission denied: "), "{text}");
        assert!(
            !text.contains(&outside.to_string_lossy().to_string()),
            "{text}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn restricted_glob_fails_closed_when_candidate_symlink_retargets_after_routing() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        let outside = fixture.path().join("outside.txt");
        fs::create_dir(&root).unwrap();
        fs::File::create(root.join("inside.txt")).unwrap();
        fs::File::create(&outside).unwrap();
        let link = root.join("candidate-link.txt");
        let old_link = root.join("candidate-link.old");
        std::os::unix::fs::symlink("inside.txt", &link).unwrap();
        let link_for_hook = link.clone();
        let old_for_hook = old_link.clone();
        let outside_for_hook = outside.clone();
        let _hook = crate::file_snapshot::tests::OriginalOpenObserverGuard::install(Arc::new(
            move |path| {
                if path == link_for_hook && link_for_hook.exists() {
                    fs::rename(&link_for_hook, &old_for_hook).unwrap();
                    std::os::unix::fs::symlink(&outside_for_hook, &link_for_hook).unwrap();
                }
            },
        ));
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let response = glob_files_with_scope(
            GlobRequest {
                pattern: "**/*".to_string(),
                path: Some(root.to_string_lossy().into_owned()),
                filter_mode: Some(FilterMode::All),
                sort: Some(SortMode::Path),
                offset: None,
                limit: None,
            },
            CancellationToken::new(),
            &scope,
        );
        let ToolContent::Text(text) = &response.content[0] else {
            panic!("expected text response: {response:?}");
        };
        assert!(response.is_error || text.contains("inside.txt"), "{text}");
        assert!(!text.contains("candidate-link.txt"), "{text}");
        assert!(
            !text.contains(&outside.to_string_lossy().to_string()),
            "{text}"
        );
    }

    #[test]
    fn scoped_glob_path_denial_is_line_safe_and_does_not_leak_raw_path() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        let outside = fixture.path().join("outside\nsecret.txt");
        fs::create_dir(&root).unwrap();
        fs::File::create(&outside).unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let response = glob_files_with_scope(
            GlobRequest {
                pattern: "*".to_string(),
                path: Some(search_display_path(&outside)),
                filter_mode: Some(FilterMode::All),
                sort: Some(SortMode::Path),
                offset: None,
                limit: None,
            },
            CancellationToken::new(),
            &scope,
        );
        let ToolContent::Text(text) = &response.content[0] else {
            panic!("expected text error");
        };
        assert!(response.is_error, "{response:?}");
        assert!(text.starts_with("Permission denied: "), "{text}");
        assert!(text.contains("~fastctx~b"), "{text}");
        assert!(!text.contains('\n'), "{text:?}");
        assert!(!text.contains("outside\nsecret"), "{text:?}");
    }
}
