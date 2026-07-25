//! grep tool backed by ripgrep engines, ignore traversal, deterministic paging, and content formatting.

use crate::budget::{
    ErrorBudgetAdapter, ErrorClass, GREP_TOKEN_BUDGET_ENV, TokenCheckpoint, error_budget_hint,
    tool_token_budget,
};
use crate::encoding::{
    ByteSource, EncodingDecision, EncodingPipelineFailure, EncodingRejection,
    canonical_encoding_label, validate_snapshot_encoding,
};
use crate::file_executor::GrepGlobExecutor;
use crate::file_snapshot::{CaptureDisposition, CaptureFailure, capture_classify};
use crate::grep_sink::{
    CapturedLine, ContentEntry, ContentSpec, FileResult, GrepSearchPlan, GrepSinkError,
    LineMatchSpan, PlanSink,
};
use crate::model::ToolResponse;
use crate::operation::{
    OpError, OperationCtx, RequestWorkGuard, WorkCheckpoint, WorkCtx, WorkStop,
};
use crate::ordered_window::{OrderedError, for_each_ordered};
use crate::path_codec::{
    PathRecord as Candidate, RootRequirement, io_error_message, resolve_search_root,
};
use crate::render_plan::{
    DetailRenderGraph, LineRenderGraph, LineRenderView, RenderPlanError, SharedLineRenderGraph,
};
use crate::search_text::{SearchText, SearchTextFailure};
use crate::traversal::collect_search_candidates;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use grep_matcher::LineTerminator;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::SearcherBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::ops::ControlFlow;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

const DEFAULT_HEAD_LIMIT: usize = 250;
const LONG_LINE_BYTES: usize = 500;
const MATCH_WINDOW_SIDE_CHARS: usize = 100;
const MAX_MATCH_CHARS: usize = 2_000;
const SEARCH_HEAP_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const CAPTURE_HEAP_LIMIT_BYTES: usize = 64 * 1024 * 1024;

/// The four grep output modes.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// Return matching lines with optional context.
    Content,
    /// Return only paths of files containing at least one match.
    #[default]
    FilesWithMatches,
    /// Return per-file occurrence counts and their aggregate.
    Count,
    /// Scan the full scope and return only global occurrence and file totals.
    Summary,
}

/// Parameters for the grep tool.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct GrepRequest {
    /// The regular expression to search for (Rust regex syntax; escape literal braces like `interface\{\}`).
    pub pattern: String,
    /// Absolute path of the file or directory to search in. Omit for the session working directory.
    pub path: Option<String>,
    /// Glob pattern to filter files, e.g. "*.rs", "**/*.{ts,tsx}" (equivalent to rg --glob).
    pub glob: Option<String>,
    /// File type filter, e.g. "js", "py", "rust" (equivalent to rg --type; more efficient than glob for standard types).
    #[serde(rename = "type")]
    #[schemars(rename = "type")]
    pub file_type: Option<String>,
    /// "content", "files_with_matches", "count", or "summary" (global totals from a full scan; ignores head_limit/offset).
    pub output_mode: Option<OutputMode>,
    /// Case-insensitive search (rg -i).
    pub case_insensitive: Option<bool>,
    /// Show line numbers in content mode (rg -n). Ignored in other modes.
    pub line_numbers: Option<bool>,
    /// Print only the matched parts, one per line (rg -o). Content mode only.
    pub only_matching: Option<bool>,
    /// Lines to show before each match (rg -B). Content mode only.
    pub before_context: Option<usize>,
    /// Lines to show after each match (rg -A). Content mode only.
    pub after_context: Option<usize>,
    /// Lines before and after each match (rg -C); overrides before/after_context. Content mode only.
    pub context: Option<usize>,
    /// Patterns may span lines; `.` matches newlines. `\n` also matches `\r\n`.
    pub multiline: Option<bool>,
    /// Max output entries. 0 removes the entry limit but not the token limit.
    pub head_limit: Option<usize>,
    /// Skip the first N entries before applying head_limit.
    pub offset: Option<usize>,
    /// Single-file target only: decode that file with this WHATWG encoding label (e.g. "gbk"), same semantics as read's encoding. On a directory target use fallback_encoding instead.
    pub encoding: Option<String>,
    /// Directory target: WHATWG encoding to assume only for files auto-detection can't determine — never overrides BOM, valid UTF-8, or already-resolved files. Strict-decoded; files that also fail under it stay in the skip report.
    pub fallback_encoding: Option<String>,
}

struct SearchOutcome {
    result: Option<FileResult>,
    entries_seen: usize,
    skip: Option<CandidateSkip>,
    transcoding_note: Option<String>,
    used_fallback: bool,
}

enum CandidateSkip {
    Encoding(EncodingRejection),
    ChangedWhileSearched,
}

impl CandidateSkip {
    fn reason(&self) -> String {
        match self {
            Self::Encoding(rejection) => rejection.skip_reason(),
            Self::ChangedWhileSearched => "changed while being searched".to_string(),
        }
    }

    fn single_file_message(&self, candidate: &Candidate) -> String {
        match self {
            Self::Encoding(rejection) => rejection.message(candidate.display.as_ref()),
            Self::ChangedWhileSearched => format!(
                "File changed while it was being searched: {}. Retry the grep request.",
                candidate.display
            ),
        }
    }
}

/// Why one candidate's search failed; the ordered reduce decides whether the
/// failure is even reachable before formatting it.
enum SearchFailure {
    /// Captured matches and context crossed the 64 MiB safety valve. Kept as a
    /// distinct variant so the paged reduce can retry with the exact live
    /// pagination window before giving up.
    CaptureOverflow,
    Cancelled,
    EpochRetired,
    Message(String),
}

impl From<String> for SearchFailure {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

fn failure_message(candidate: &Candidate, failure: SearchFailure) -> String {
    match failure {
        SearchFailure::CaptureOverflow => capture_limit_error(candidate),
        SearchFailure::Cancelled => "Request cancelled.".to_string(),
        SearchFailure::EpochRetired => {
            unreachable!("retired speculative work is never delivered to the reducer")
        }
        SearchFailure::Message(message) => message,
    }
}

struct PageFormat<'a> {
    offset: usize,
    head_limit: usize,
    budget: usize,
    budget_variable: &'a str,
    scan_complete: bool,
    total_entries_seen: usize,
    skipped_files: &'a SkippedFiles,
    transcoding_notes: &'a BTreeSet<String>,
    fallback_usage: &'a FallbackUsage,
    single_file_target: bool,
    operation: Option<&'a OperationCtx>,
}

#[derive(Default)]
struct SkippedFiles {
    entries: Vec<SkippedFile>,
}

impl SkippedFiles {
    fn record(&mut self, path: &str, skip: &CandidateSkip) {
        self.entries.push(SkippedFile {
            path: path.to_string(),
            reason: skip.reason(),
        });
    }
}

struct SkippedFile {
    path: String,
    reason: String,
}

#[derive(Default)]
struct FallbackUsage {
    count: usize,
    encoding: Option<&'static str>,
}

impl FallbackUsage {
    fn record(&mut self, encoding: &'static str) {
        self.count = self.count.saturating_add(1);
        self.encoding = Some(encoding);
    }

    fn note(&self) -> Option<String> {
        let encoding = self.encoding?;
        Some(format!(
            "(Note: {} decoded using fallback encoding {encoding}.)",
            counted(self.count, "file", "files")
        ))
    }
}

#[cfg(test)]
#[derive(Default)]
struct GrepExecutionProbe {
    exact_retries: AtomicUsize,
}

#[derive(Clone, Debug)]
struct SearchEncoding {
    explicit: Option<String>,
    fallback: Option<String>,
}

/// Executes a grep query within a caller-owned cancellation scope.
///
/// Cancellation is checked throughout admission, traversal, capture, decoding,
/// matching, sorting, rendering, and token verification. A cancelled operation
/// returns an error response and never exposes a partial success body.
pub fn grep_files(request: GrepRequest, cancellation: CancellationToken) -> ToolResponse {
    let budget = match tool_token_budget(GREP_TOKEN_BUDGET_ENV) {
        Ok(budget) => budget,
        Err(message) => {
            return ErrorBudgetAdapter::new(
                error_budget_hint(GREP_TOKEN_BUDGET_ENV),
                GREP_TOKEN_BUDGET_ENV,
            )
            .error(ErrorClass::Budget, message);
        }
    };
    let (mut guard, operation) = RequestWorkGuard::new(
        rmcp::model::RequestId::String(Arc::from("direct-grep")),
        cancellation,
    );
    let response = grep_files_with_budget_source_and_execution(
        request,
        budget.value,
        budget.variable,
        CAPTURE_HEAP_LIMIT_BYTES,
        #[cfg(test)]
        None,
        operation,
        GrepGlobExecutor::shared(),
    );
    guard.disarm();
    response
}

/// Runs grep on the server's request cancellation scope and shared executor.
pub(crate) fn grep_files_cancellable(
    operation: OperationCtx,
    executor: Arc<GrepGlobExecutor>,
    request: GrepRequest,
) -> Result<ToolResponse, OpError> {
    let work = operation.inline_work();
    work.check_inline()?;
    let budget = match tool_token_budget(GREP_TOKEN_BUDGET_ENV) {
        Ok(budget) => budget,
        Err(message) => {
            return Ok(ErrorBudgetAdapter::new(
                error_budget_hint(GREP_TOKEN_BUDGET_ENV),
                GREP_TOKEN_BUDGET_ENV,
            )
            .error(ErrorClass::Budget, message));
        }
    };
    let response = grep_files_with_budget_source_and_execution(
        request,
        budget.value,
        budget.variable,
        CAPTURE_HEAP_LIMIT_BYTES,
        #[cfg(test)]
        None,
        operation.clone(),
        executor,
    );
    work.check_inline()?;
    Ok(response)
}

#[cfg(test)]
fn grep_files_with_budget(request: GrepRequest, budget: usize) -> ToolResponse {
    let (mut guard, operation) = RequestWorkGuard::new(
        rmcp::model::RequestId::String(Arc::from("test-grep")),
        CancellationToken::new(),
    );
    let response = grep_files_with_budget_source_and_execution(
        request,
        budget,
        "FASTCTX_TOKEN_BUDGET",
        CAPTURE_HEAP_LIMIT_BYTES,
        None,
        operation,
        Arc::new(GrepGlobExecutor::with_test_parallelism(1)),
    );
    guard.disarm();
    response
}

#[cfg(test)]
fn grep_files_with_budget_source_and_operation(
    request: GrepRequest,
    budget: usize,
    budget_variable: &str,
    operation: Option<&OperationCtx>,
) -> ToolResponse {
    if let Some(operation) = operation {
        return grep_files_with_budget_source_and_execution(
            request,
            budget,
            budget_variable,
            CAPTURE_HEAP_LIMIT_BYTES,
            None,
            operation.clone(),
            Arc::new(GrepGlobExecutor::with_test_parallelism(1)),
        );
    }
    let (mut guard, operation) = RequestWorkGuard::new(
        rmcp::model::RequestId::String(Arc::from("test-grep-operation")),
        CancellationToken::new(),
    );
    let response = grep_files_with_budget_source_and_execution(
        request,
        budget,
        budget_variable,
        CAPTURE_HEAP_LIMIT_BYTES,
        None,
        operation,
        Arc::new(GrepGlobExecutor::with_test_parallelism(1)),
    );
    guard.disarm();
    response
}

#[cfg(test)]
fn grep_files_with_budget_and_parallelism(
    request: GrepRequest,
    budget: usize,
    parallelism: usize,
) -> ToolResponse {
    let (mut guard, operation) = RequestWorkGuard::new(
        rmcp::model::RequestId::String(Arc::from(format!("test-grep-p{parallelism}"))),
        CancellationToken::new(),
    );
    let response = grep_files_with_budget_source_and_execution(
        request,
        budget,
        "FASTCTX_TOKEN_BUDGET",
        CAPTURE_HEAP_LIMIT_BYTES,
        None,
        operation,
        Arc::new(GrepGlobExecutor::with_test_parallelism(parallelism)),
    );
    guard.disarm();
    response
}

#[cfg(test)]
fn grep_files_with_parallelism_and_capture_limit(
    request: GrepRequest,
    budget: usize,
    parallelism: usize,
    capture_heap_limit_bytes: usize,
) -> (
    ToolResponse,
    usize,
    crate::file_executor::LedgerSnapshot,
    crate::file_executor::LedgerSnapshot,
) {
    let (mut guard, operation) = RequestWorkGuard::new(
        rmcp::model::RequestId::String(Arc::from(format!(
            "test-grep-p{parallelism}-capture-limit"
        ))),
        CancellationToken::new(),
    );
    let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(parallelism));
    let probe = Arc::new(GrepExecutionProbe::default());
    let response = grep_files_with_budget_source_and_execution(
        request,
        budget,
        "FASTCTX_TOKEN_BUDGET",
        capture_heap_limit_bytes,
        Some(Arc::clone(&probe)),
        operation,
        Arc::clone(&executor),
    );
    guard.disarm();
    executor.wait_for_test_quiescence();
    (
        response,
        probe.exact_retries.load(Ordering::Acquire),
        executor.test_burst_ledger(),
        executor.test_ticket_ledger(),
    )
}

fn grep_files_with_budget_source_and_execution(
    request: GrepRequest,
    budget: usize,
    budget_variable: &str,
    capture_heap_limit_bytes: usize,
    #[cfg(test)] execution_probe: Option<Arc<GrepExecutionProbe>>,
    operation: OperationCtx,
    executor: Arc<GrepGlobExecutor>,
) -> ToolResponse {
    let adapter = ErrorBudgetAdapter::new(budget, budget_variable);
    adapter.adapt(grep_files_with_budget_source_and_execution_unadapted(
        request,
        budget,
        budget_variable,
        capture_heap_limit_bytes,
        #[cfg(test)]
        execution_probe,
        operation,
        executor,
    ))
}

fn grep_files_with_budget_source_and_execution_unadapted(
    request: GrepRequest,
    budget: usize,
    budget_variable: &str,
    capture_heap_limit_bytes: usize,
    #[cfg(test)] execution_probe: Option<Arc<GrepExecutionProbe>>,
    operation: OperationCtx,
    executor: Arc<GrepGlobExecutor>,
) -> ToolResponse {
    let root_input = request.path.clone();
    let root = match resolve_search_root(root_input.as_deref(), RootRequirement::FileOrDirectory) {
        Ok(root) => root,
        Err(message) => return ToolResponse::error(message),
    };
    let single_file_target = root.is_file();
    if single_file_target && request.fallback_encoding.is_some() {
        return ToolResponse::error(
            "The fallback_encoding parameter only applies to directory targets; use encoding for a single file.",
        );
    }
    if !single_file_target && request.encoding.is_some() {
        return ToolResponse::error(
            "The encoding parameter only applies to single-file targets; use fallback_encoding for a directory.",
        );
    }
    if let Some(encoding) = request.encoding.as_deref()
        && let Err(rejection) = canonical_encoding_label(encoding)
    {
        return ToolResponse::error(rejection.message(root.display.as_ref()));
    }
    let fallback_encoding_label = match request.fallback_encoding.as_deref() {
        Some(encoding) => match canonical_encoding_label(encoding) {
            Ok(label) => Some(label),
            Err(rejection) => {
                return ToolResponse::error(rejection.message(root.display.as_ref()));
            }
        },
        None => None,
    };
    let search_encoding = SearchEncoding {
        explicit: request.encoding.clone(),
        fallback: request.fallback_encoding.clone(),
    };
    let multiline = request.multiline.unwrap_or(false);
    let pattern = if multiline {
        normalize_multiline_pattern(&request.pattern)
    } else {
        request.pattern.clone()
    };
    let matcher = match build_matcher(
        &pattern,
        request.case_insensitive.unwrap_or(false),
        multiline,
    ) {
        Ok(matcher) => Arc::new(matcher),
        Err(error) => {
            return ToolResponse::error(format!(
                "Invalid regex pattern: {error}\nNote: Rust regex syntax — no lookaround or backreferences; escape literal braces."
            ));
        }
    };
    let glob = match build_glob(request.glob.as_deref()) {
        Ok(glob) => glob,
        Err(message) => return ToolResponse::error(message),
    };
    let candidates = match collect_search_candidates(
        &root,
        glob.as_ref(),
        request.file_type.as_deref(),
        Some(&operation),
        Some(&executor),
    ) {
        Ok(candidates) => Arc::<[Candidate]>::from(candidates),
        Err(message) => return ToolResponse::error(message),
    };
    let offset = request.offset.unwrap_or(0);
    let head_limit = request.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
    let mode = request.output_mode.unwrap_or_default();
    let only_matching = request.only_matching.unwrap_or(false);
    let (before_context, after_context) = if mode == OutputMode::Content {
        if let Some(context) = request.context {
            (context, context)
        } else {
            (
                request.before_context.unwrap_or(0),
                request.after_context.unwrap_or(0),
            )
        }
    } else {
        (0, 0)
    };
    if mode == OutputMode::Summary {
        let mut occurrence_total = 0_usize;
        let mut file_total = 0_usize;
        let mut skipped_files = SkippedFiles::default();
        let mut transcoding_notes = BTreeSet::new();
        let mut fallback_usage = FallbackUsage::default();
        let plan = GrepSearchPlan::Count;
        let mut failure: Option<ToolResponse> = None;
        let worker_matcher = Arc::clone(&matcher);
        let worker_encoding = search_encoding.clone();
        let panic_candidates = Arc::clone(&candidates);
        let ordered = for_each_ordered(
            Arc::clone(&candidates),
            operation.clone(),
            Arc::clone(&executor),
            move |_, candidate, work| {
                search_candidate_for_work(
                    candidate,
                    &worker_matcher,
                    plan,
                    multiline,
                    &worker_encoding,
                    work,
                )
            },
            move |index, _| {
                Err(SearchFailure::Message(format!(
                    "Search worker panicked while processing {}.",
                    panic_candidates[index].display
                )))
            },
            |index, outcome, _| {
                let candidate = &candidates[index];
                let outcome = match outcome {
                    Ok(outcome) => outcome,
                    Err(kind) => {
                        failure = Some(ToolResponse::error(failure_message(candidate, kind)));
                        return ControlFlow::Break(());
                    }
                };
                if let Some(skip) = outcome.skip {
                    if single_file_target {
                        failure = Some(ToolResponse::error(skip.single_file_message(candidate)));
                        return ControlFlow::Break(());
                    }
                    skipped_files.record(candidate.display.as_ref(), &skip);
                    return ControlFlow::Continue(());
                }
                if let Some(note) = outcome.transcoding_note {
                    transcoding_notes.insert(note);
                }
                if outcome.used_fallback
                    && let Some(encoding) = fallback_encoding_label
                {
                    fallback_usage.record(encoding);
                }
                if let Some(result) = outcome.result {
                    file_total = file_total.saturating_add(1);
                    occurrence_total = occurrence_total.saturating_add(result.occurrence_count());
                }
                ControlFlow::Continue(())
            },
        );
        if let Err(error) = ordered {
            return ToolResponse::error(ordered_error_message(error));
        }
        if let Some(response) = failure {
            return response;
        }
        let page = PageFormat {
            offset: 0,
            head_limit: 0,
            budget,
            budget_variable,
            scan_complete: true,
            total_entries_seen: 0,
            skipped_files: &skipped_files,
            transcoding_notes: &transcoding_notes,
            fallback_usage: &fallback_usage,
            single_file_target: false,
            operation: Some(&operation),
        };
        return format_summary(occurrence_total, file_total, &page);
    }

    let budget_entry_limit = budget.saturating_mul(4).saturating_add(1).max(1);
    let effective_head_limit = if head_limit == 0 {
        budget_entry_limit
    } else {
        head_limit.min(budget_entry_limit)
    };
    let probe_entry_limit = effective_head_limit.saturating_add(1);

    let mut results = Vec::new();
    let mut collected_entries = 0_usize;
    let mut skip_remaining = offset;
    let mut total_entries_seen = 0_usize;
    let mut scan_complete = true;
    let mut skipped_files = SkippedFiles::default();
    let mut transcoding_notes = BTreeSet::new();
    let mut fallback_usage = FallbackUsage::default();
    // Every candidate is searched with identical options so files can run in
    // parallel. Content mode over-captures (no skip, worst-case cap of
    // offset + probe); the ordered reduce below trims each file back to
    // exactly the entries the sequential pagination would have selected, so
    // the observable output stays byte-identical to a serial scan.
    let worker_plan = match mode {
        OutputMode::FilesWithMatches => GrepSearchPlan::Exists,
        OutputMode::Count => GrepSearchPlan::Count,
        OutputMode::Content => {
            let spec = ContentSpec {
                multiline,
                skip_entries: 0,
                max_selected_entries: Some(offset.saturating_add(probe_entry_limit)),
                capture_match_text: only_matching,
                before_context,
                after_context,
                capture_heap_limit_bytes,
            };
            if only_matching || multiline {
                GrepSearchPlan::ContentOccurrence(spec)
            } else {
                GrepSearchPlan::ContentLine(spec)
            }
        }
        OutputMode::Summary => unreachable!("summary is handled before paging"),
    };
    let mut failure: Option<String> = None;
    let worker_matcher = Arc::clone(&matcher);
    let worker_encoding = search_encoding.clone();
    let panic_candidates = Arc::clone(&candidates);
    let ordered = for_each_ordered(
        Arc::clone(&candidates),
        operation.clone(),
        Arc::clone(&executor),
        move |_, candidate, work| {
            search_candidate_for_work(
                candidate,
                &worker_matcher,
                worker_plan,
                multiline,
                &worker_encoding,
                work,
            )
        },
        move |index, _| {
            Err(SearchFailure::Message(format!(
                "Search worker panicked while processing {}.",
                panic_candidates[index].display
            )))
        },
        |index, outcome, reducer| {
            let candidate = &candidates[index];
            let (outcome, exact_form) = match outcome {
                Ok(outcome) => (outcome, false),
                Err(SearchFailure::CaptureOverflow) => {
                    #[cfg(test)]
                    if let Some(probe) = &execution_probe {
                        probe.exact_retries.fetch_add(1, Ordering::AcqRel);
                    }
                    // The over-capture cap can cross the 64 MiB capture valve
                    // where the live window would not; retry with the exact
                    // sequential options before surfacing the error.
                    if let Err(error) = reducer.retire_generation() {
                        failure = Some(ordered_error_message(error));
                        return ControlFlow::Break(());
                    }
                    let exact = if mode == OutputMode::Content {
                        worker_plan.with_content_window(
                            skip_remaining,
                            Some(probe_entry_limit.saturating_sub(collected_entries)),
                        )
                    } else {
                        worker_plan
                    };
                    let exact_work = operation.inline_work();
                    match search_candidate(
                        candidate,
                        &matcher,
                        exact,
                        multiline,
                        &search_encoding,
                        Some(&exact_work),
                    ) {
                        Ok(outcome) => (outcome, true),
                        Err(kind) => {
                            failure = Some(failure_message(candidate, kind));
                            return ControlFlow::Break(());
                        }
                    }
                }
                Err(kind) => {
                    failure = Some(failure_message(candidate, kind));
                    return ControlFlow::Break(());
                }
            };
            if let Some(skip) = outcome.skip {
                if single_file_target {
                    failure = Some(skip.single_file_message(candidate));
                    return ControlFlow::Break(());
                }
                skipped_files.record(candidate.display.as_ref(), &skip);
                return ControlFlow::Continue(());
            }
            if let Some(note) = outcome.transcoding_note {
                transcoding_notes.insert(note);
            }
            if outcome.used_fallback
                && let Some(encoding) = fallback_encoding_label
            {
                fallback_usage.record(encoding);
            }
            if mode == OutputMode::Content {
                if exact_form {
                    // The retried search already applied the live window, so
                    // account for it exactly like the sequential loop did.
                    total_entries_seen = total_entries_seen.saturating_add(outcome.entries_seen);
                    skip_remaining = skip_remaining.saturating_sub(outcome.entries_seen);
                    if let Some(result) = outcome.result {
                        collected_entries = collected_entries.saturating_add(result.entry_count());
                        results.push(result);
                    }
                } else if let Some(mut result) = outcome.result {
                    // Under the over-capture options every seen entry was
                    // selected, so entries[..start] is this file's share of
                    // the remaining offset and entries[start..end] is what a
                    // sequential scan would have delivered; `end` is also the
                    // number of entries that scan would have seen here.
                    let available = result.entry_count();
                    let need = probe_entry_limit.saturating_sub(collected_entries);
                    let start = skip_remaining.min(available);
                    let end = available.min(start.saturating_add(need));
                    total_entries_seen = total_entries_seen.saturating_add(end);
                    skip_remaining = skip_remaining.saturating_sub(start);
                    if end > start {
                        result.trim_entries(start, end);
                        collected_entries = collected_entries.saturating_add(end - start);
                        results.push(result);
                    }
                }
            } else {
                let Some(result) = outcome.result else {
                    return ControlFlow::Continue(());
                };
                total_entries_seen = total_entries_seen.saturating_add(1);
                if skip_remaining > 0 {
                    skip_remaining -= 1;
                    return ControlFlow::Continue(());
                }
                collected_entries = collected_entries.saturating_add(1);
                results.push(result);
            }
            if collected_entries >= probe_entry_limit {
                scan_complete = false;
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        },
    );
    if let Err(error) = ordered {
        return ToolResponse::error(ordered_error_message(error));
    }
    if let Some(message) = failure {
        return ToolResponse::error(message);
    }
    let page = PageFormat {
        offset,
        head_limit: effective_head_limit,
        budget,
        budget_variable,
        scan_complete,
        total_entries_seen,
        skipped_files: &skipped_files,
        transcoding_notes: &transcoding_notes,
        fallback_usage: &fallback_usage,
        single_file_target,
        operation: Some(&operation),
    };
    if results.is_empty() {
        return if total_entries_seen == 0 {
            zero_result(mode, &page)
        } else {
            offset_exhausted(mode, &page)
        };
    }

    match mode {
        OutputMode::FilesWithMatches => format_files_mode(&results, &page),
        OutputMode::Count => format_count_mode(&results, &page),
        OutputMode::Content => format_content_mode(
            &results,
            &request,
            &page,
            #[cfg(test)]
            None,
        ),
        OutputMode::Summary => unreachable!("summary is handled before paging"),
    }
}

fn build_matcher(
    pattern: &str,
    case_insensitive: bool,
    multiline: bool,
) -> Result<RegexMatcher, grep_regex::Error> {
    let mut builder = RegexMatcherBuilder::new();
    builder
        .case_insensitive(case_insensitive)
        .multi_line(true)
        .crlf(true)
        .dot_matches_new_line(multiline);
    if multiline {
        builder.line_terminator(None);
    }
    builder.build(pattern)
}

fn build_glob(pattern: Option<&str>) -> Result<Option<GlobSet>, String> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    let glob = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| {
            format!(
                "Invalid glob pattern: {error}. Use forms like \"*.rs\" or \"**/*.{{ts,tsx}}\"."
            )
        })?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    builder.build().map(Some).map_err(|error| {
        format!("Invalid glob pattern: {error}. Use forms like \"*.rs\" or \"**/*.{{ts,tsx}}\".")
    })
}

fn search_candidate(
    candidate: &Candidate,
    matcher: &RegexMatcher,
    plan: GrepSearchPlan,
    multiline: bool,
    encoding: &SearchEncoding,
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<SearchOutcome, SearchFailure> {
    if let Some(content_multiline) = plan.content_multiline() {
        debug_assert_eq!(content_multiline, multiline);
    }
    let snapshot = match capture_classify(
        candidate,
        encoding.explicit.as_deref(),
        encoding.fallback.as_deref(),
        operation,
    ) {
        Ok(CaptureDisposition::Searchable(snapshot)) => snapshot,
        Ok(CaptureDisposition::BinarySkipped(proof)) => {
            debug_assert!(matches!(
                proof,
                crate::file_snapshot::TerminalProof::NulWithinFrozenProbe
                    | crate::file_snapshot::TerminalProof::BinaryMagicAfterUtf8Failure
            ));
            return Ok(SearchOutcome {
                result: None,
                entries_seen: 0,
                skip: None,
                transcoding_note: None,
                used_fallback: false,
            });
        }
        Ok(CaptureDisposition::EncodingRejected { rejection, proof }) => {
            debug_assert!(proof.rejection().is_some());
            return Ok(SearchOutcome {
                result: None,
                entries_seen: 0,
                skip: Some(CandidateSkip::Encoding(rejection)),
                transcoding_note: None,
                used_fallback: false,
            });
        }
        Ok(CaptureDisposition::FileChanged) => {
            return Ok(SearchOutcome {
                result: None,
                entries_seen: 0,
                skip: Some(CandidateSkip::ChangedWhileSearched),
                transcoding_note: None,
                used_fallback: false,
            });
        }
        Err(CaptureFailure::Cancelled) => return Err(SearchFailure::Cancelled),
        Err(CaptureFailure::EpochRetired) => return Err(SearchFailure::EpochRetired),
        Err(CaptureFailure::InvalidEncoding(rejection)) => {
            return Err(rejection.message(candidate.display.as_ref()).into());
        }
        Err(CaptureFailure::Io(error)) => {
            return Err(io_error_message(&candidate.native, &error).into());
        }
        Err(CaptureFailure::Snapshot(error)) => {
            return Err(snapshot_error_message(candidate, &error).into());
        }
    };
    debug_assert_eq!(snapshot.path().native, candidate.native);
    if let Some(bytes) = snapshot.memory_bytes() {
        debug_assert_eq!(snapshot.len(), bytes.len() as u64);
    }
    let source = ByteSource::Snapshot(&snapshot);
    check_search_operation(operation)?;
    let initial = validate_search_encoding(
        &snapshot,
        candidate,
        encoding.explicit.as_deref(),
        operation,
    )?;
    check_search_operation(operation)?;
    let (validated, used_fallback) = match initial {
        EncodingDecision::Text(validated) => (validated, false),
        EncodingDecision::Binary => {
            return Ok(SearchOutcome {
                result: None,
                entries_seen: 0,
                skip: None,
                transcoding_note: None,
                used_fallback: false,
            });
        }
        EncodingDecision::Rejected(rejection) => match encoding.fallback.as_deref() {
            Some(fallback)
                if encoding.explicit.is_none()
                    && !matches!(rejection, EncodingRejection::BomMismatch { .. }) =>
            {
                match validate_search_encoding(&snapshot, candidate, Some(fallback), operation)? {
                    EncodingDecision::Text(validated) => (validated, true),
                    EncodingDecision::Binary | EncodingDecision::Rejected(_) => {
                        return Ok(SearchOutcome {
                            result: None,
                            entries_seen: 0,
                            skip: Some(CandidateSkip::Encoding(rejection)),
                            transcoding_note: None,
                            used_fallback: false,
                        });
                    }
                }
            }
            _ => {
                return Ok(SearchOutcome {
                    result: None,
                    entries_seen: 0,
                    skip: Some(CandidateSkip::Encoding(rejection)),
                    transcoding_note: None,
                    used_fallback: false,
                });
            }
        },
    };
    check_search_operation(operation)?;
    let transcoding_note = (!used_fallback)
        .then(|| validated.transcoding_note())
        .flatten();
    let mut searcher = SearcherBuilder::new();
    searcher
        .line_number(true)
        .line_terminator(LineTerminator::crlf())
        .multi_line(multiline)
        .before_context(plan.before_context())
        .after_context(plan.after_context())
        .heap_limit(Some(SEARCH_HEAP_LIMIT_BYTES));
    let mut searcher = searcher.build();
    let content_backing = if plan.content_multiline().is_some() {
        if let Some(start) = validated.utf8_snapshot_start() {
            let range = snapshot
                .shared_range(start)
                .map_err(|error| snapshot_error_message(candidate, &error))?;
            Some(SearchText::from_snapshot(range))
        } else {
            let reader = validated
                .open_source_reader(source)
                .map_err(|error| snapshot_error_message(candidate, &error))?;
            Some(
                SearchText::capture(reader, operation).map_err(|failure| match failure {
                    SearchTextFailure::Io(error) => {
                        SearchFailure::Message(snapshot_error_message(candidate, &error))
                    }
                    SearchTextFailure::Stopped(WorkStop::RequestCancelled) => {
                        SearchFailure::Cancelled
                    }
                    SearchTextFailure::Stopped(WorkStop::EpochRetired) => {
                        SearchFailure::EpochRetired
                    }
                })?,
            )
        }
    } else {
        None
    };
    let mut sink = PlanSink::new(matcher, plan, operation, content_backing.clone());
    check_search_operation(operation)?;
    #[cfg(test)]
    if let Some(operation) = operation {
        operation.stage(crate::operation::TestStage::BeforeRegexSearch);
    }
    check_search_operation(operation)?;
    let search_result = if let Some(backing) = content_backing {
        match backing.memory_bytes() {
            Some(bytes) => searcher.search_slice(matcher, bytes, &mut sink),
            None => {
                let reader = backing
                    .open_reader()
                    .map_err(|error| snapshot_error_message(candidate, &error))?;
                searcher.search_reader(matcher, reader, &mut sink)
            }
        }
    } else {
        match snapshot.memory_bytes() {
            Some(bytes) => {
                let Some(decoded) = validated.decode_for_search(bytes) else {
                    return Err(validated
                        .malformed_rejection()
                        .message(candidate.display.as_ref())
                        .into());
                };
                searcher.search_slice(matcher, &decoded, &mut sink)
            }
            None => {
                let reader = validated
                    .open_source_reader(source)
                    .map_err(|error| snapshot_error_message(candidate, &error))?;
                searcher.search_reader(matcher, reader, &mut sink)
            }
        }
    };
    check_search_operation(operation)?;
    if let Err(error) = search_result {
        return Err(match error {
            GrepSinkError::Stopped(WorkStop::RequestCancelled) => SearchFailure::Cancelled,
            GrepSinkError::Stopped(WorkStop::EpochRetired) => SearchFailure::EpochRetired,
            GrepSinkError::CaptureOverflow => SearchFailure::CaptureOverflow,
            GrepSinkError::CountOverflow => SearchFailure::Message(format!(
                "Cannot search file {}: the occurrence count overflowed.",
                candidate.display
            )),
            GrepSinkError::Search(message) => SearchFailure::Message(format!(
                "Cannot search file {}: {message}",
                candidate.display
            )),
            GrepSinkError::Io(error) if error.kind() == io::ErrorKind::InvalidData => {
                SearchFailure::Message(
                    validated
                        .malformed_rejection()
                        .message(candidate.display.as_ref()),
                )
            }
            GrepSinkError::Io(error) => {
                let message = error.to_string().to_ascii_lowercase();
                SearchFailure::Message(
                    if message.contains("heap limit") || message.contains("allocation limit") {
                        search_error_message(candidate, &error)
                    } else {
                        snapshot_error_message(candidate, &error)
                    },
                )
            }
        });
    }
    let sink_output = sink.into_output(
        candidate.display.to_string(),
        validated.total_lines,
        validated.has_trailing_newline,
    );
    Ok(SearchOutcome {
        result: sink_output.result,
        entries_seen: sink_output.entries_seen,
        skip: None,
        transcoding_note,
        used_fallback,
    })
}

fn search_candidate_for_work(
    candidate: &Candidate,
    matcher: &RegexMatcher,
    plan: GrepSearchPlan,
    multiline: bool,
    encoding: &SearchEncoding,
    work: &WorkCtx,
) -> Result<Result<SearchOutcome, SearchFailure>, WorkStop> {
    match search_candidate(candidate, matcher, plan, multiline, encoding, Some(work)) {
        Err(SearchFailure::Cancelled) => Err(WorkStop::RequestCancelled),
        Err(SearchFailure::EpochRetired) => Err(WorkStop::EpochRetired),
        outcome => Ok(outcome),
    }
}

fn check_search_operation(operation: Option<&dyn WorkCheckpoint>) -> Result<(), SearchFailure> {
    match operation.map(WorkCheckpoint::check_work) {
        Some(Err(WorkStop::RequestCancelled)) => Err(SearchFailure::Cancelled),
        Some(Err(WorkStop::EpochRetired)) => Err(SearchFailure::EpochRetired),
        Some(Ok(())) | None => Ok(()),
    }
}

fn validate_search_encoding(
    snapshot: &crate::file_snapshot::SealedSnapshot,
    candidate: &Candidate,
    explicit_encoding: Option<&str>,
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<EncodingDecision, SearchFailure> {
    validate_snapshot_encoding(snapshot, explicit_encoding, operation).map_err(|failure| {
        match failure {
            EncodingPipelineFailure::Io(error) => snapshot_error_message(candidate, &error).into(),
            EncodingPipelineFailure::Stopped(WorkStop::RequestCancelled) => {
                SearchFailure::Cancelled
            }
            EncodingPipelineFailure::Stopped(WorkStop::EpochRetired) => SearchFailure::EpochRetired,
        }
    })
}

fn ordered_error_message(error: OrderedError) -> String {
    match error {
        OrderedError::Cancelled => "Request cancelled.".to_string(),
        OrderedError::GenerationOverflow => {
            "Cannot continue ordered search because its generation counter overflowed.".to_string()
        }
    }
}

fn snapshot_error_message(candidate: &Candidate, error: &io::Error) -> String {
    stable_snapshot_error(candidate.display.as_ref(), error)
}

fn stable_snapshot_error(path: &str, error: &io::Error) -> String {
    format!(
        "Cannot create a stable search snapshot for {path}: {error}. Free temporary-disk space or retry after the file stops changing."
    )
}

fn search_error_message(candidate: &Candidate, error: &io::Error) -> String {
    let message = error.to_string();
    let lower = message.to_ascii_lowercase();
    if lower.contains("heap limit") || lower.contains("allocation limit") {
        format!(
            "Cannot search file {}: a line or multiline buffer exceeds the 64 MiB safety limit. Narrow the path or search without multiline.",
            candidate.display
        )
    } else {
        format!("Cannot search file {}: {error}", candidate.display)
    }
}

fn capture_limit_error(candidate: &Candidate) -> String {
    format!(
        "Cannot search file {}: matching content and context exceed the 64 MiB safety limit. Narrow the pattern or reduce context.",
        candidate.display
    )
}

impl PageFormat<'_> {
    fn work(&self) -> Option<&dyn WorkCheckpoint> {
        self.operation
            .map(|operation| operation as &dyn WorkCheckpoint)
    }
}

struct GrepNoteUnits {
    fixed: Vec<Arc<str>>,
    details: Vec<Arc<str>>,
    fallback: Option<Arc<str>>,
}

impl GrepNoteUnits {
    fn new(page: &PageFormat<'_>) -> Self {
        let fixed = page
            .transcoding_notes
            .iter()
            .map(|line| Arc::<str>::from(line.as_str()))
            .collect();
        let details = page
            .skipped_files
            .entries
            .iter()
            .map(|entry| Arc::<str>::from(format!("{} — {}", entry.path, entry.reason)))
            .collect();
        let fallback = page.fallback_usage.note().map(Arc::<str>::from);
        Self {
            fixed,
            details,
            fallback,
        }
    }

    fn tail(&self, terminal: Arc<str>) -> Vec<Arc<str>> {
        let mut tail = Vec::with_capacity(2);
        if let Some(fallback) = &self.fallback {
            tail.push(Arc::clone(fallback));
        }
        tail.push(terminal);
        tail
    }

    fn final_notes(&self, shown_skips: usize, terminal: Arc<str>) -> Vec<Arc<str>> {
        let mut notes = Vec::with_capacity(
            self.fixed
                .len()
                .saturating_add(shown_skips)
                .saturating_add(2),
        );
        notes.extend(self.fixed.iter().cloned());
        notes.extend(self.details[..shown_skips].iter().cloned());
        if let Some(fallback) = &self.fallback {
            notes.push(Arc::clone(fallback));
        }
        notes.push(terminal);
        notes
    }

    fn baseline_notes(&self, terminal: &str) -> Result<Vec<Arc<str>>, RenderPlanError> {
        let terminal = Arc::<str>::from(terminal_with_skips(terminal, self.details.len(), 0)?);
        Ok(self.final_notes(0, terminal))
    }
}

/// Replays v0.1.1's inclusive binary-probe order without unsigned underflow.
fn replay_compat_binary_probes<T, E>(
    mut low: usize,
    mut high: usize,
    mut best: Option<T>,
    mut probe: impl FnMut(usize) -> Result<Option<T>, E>,
) -> Result<Option<T>, E> {
    while low <= high {
        let middle = low + (high - low) / 2;
        if let Some(candidate) = probe(middle)? {
            best = Some(candidate);
            let Some(next) = middle.checked_add(1) else {
                break;
            };
            low = next;
        } else {
            if middle == 0 {
                break;
            }
            high = middle - 1;
        }
    }
    Ok(best)
}

fn select_body_prefix(
    graph: &mut LineRenderGraph,
    maximum: usize,
    page: &PageFormat<'_>,
    notes: &GrepNoteUnits,
    mut terminal: impl FnMut(usize) -> String,
) -> Result<Option<(usize, String)>, RenderPlanError> {
    if maximum == 0 {
        return Ok(None);
    }
    let maximum_terminal = terminal(maximum);
    let maximum_notes = notes.baseline_notes(&maximum_terminal)?;
    if graph.probe_notes(maximum, &maximum_notes, page.work())? <= page.budget {
        return Ok(Some((maximum, maximum_terminal)));
    }

    replay_compat_binary_probes(1, maximum - 1, None, |middle| {
        let candidate_terminal = terminal(middle);
        let candidate_notes = notes.baseline_notes(&candidate_terminal)?;
        if graph.probe_notes(middle, &candidate_notes, page.work())? <= page.budget {
            Ok(Some((middle, candidate_terminal)))
        } else {
            Ok(None)
        }
    })
}

fn finish_selected_grep_body(
    graph: &mut LineRenderGraph,
    selected: Result<Option<(usize, String)>, RenderPlanError>,
    page: &PageFormat<'_>,
    notes: &GrepNoteUnits,
) -> ToolResponse {
    let Some((shown, terminal)) = (match selected {
        Ok(selected) => selected,
        Err(error) => return grep_render_failure(error),
    }) else {
        return budget_too_small(page.budget, page.budget_variable);
    };
    match finish_grep_graph(graph, shown, page, notes, &terminal) {
        Ok(Some(text)) => ToolResponse::text(text),
        Ok(None) => budget_too_small(page.budget, page.budget_variable),
        Err(error) => grep_render_failure(error),
    }
}

fn finish_grep_graph(
    graph: &mut LineRenderGraph,
    shown: usize,
    page: &PageFormat<'_>,
    notes: &GrepNoteUnits,
    terminal: &str,
) -> Result<Option<String>, RenderPlanError> {
    let body_checkpoint = graph.checkpoint(shown)?;
    let Some(selected) = select_grep_notes(&body_checkpoint, shown > 0, page, notes, terminal)?
    else {
        return Ok(None);
    };
    let rendered = graph.finish(
        shown,
        &selected.notes,
        selected.tokens,
        page.budget,
        page.work(),
    )?;
    Ok(Some(rendered.text))
}

fn finish_content_grep_view(
    graph: &mut SharedLineRenderGraph,
    view: &LineRenderView,
    page: &PageFormat<'_>,
    notes: &GrepNoteUnits,
    terminal: &str,
) -> Result<Option<String>, RenderPlanError> {
    let Some(selected) =
        select_grep_notes(view.checkpoint(), view.len() > 0, page, notes, terminal)?
    else {
        return Ok(None);
    };
    let rendered = graph.finish(
        view,
        &selected.notes,
        selected.tokens,
        page.budget,
        page.work(),
    )?;
    Ok(Some(rendered.text))
}

struct SelectedGrepNotes {
    notes: Vec<Arc<str>>,
    tokens: usize,
}

fn select_grep_notes(
    body_checkpoint: &TokenCheckpoint,
    prefix_has_body: bool,
    page: &PageFormat<'_>,
    notes: &GrepNoteUnits,
    terminal: &str,
) -> Result<Option<SelectedGrepNotes>, RenderPlanError> {
    let mut details = DetailRenderGraph::new(
        body_checkpoint,
        prefix_has_body,
        &notes.fixed,
        &notes.details,
        page.work(),
    )?;
    let total_skipped = notes.details.len();

    let full_terminal =
        Arc::<str>::from(terminal_with_skips(terminal, total_skipped, total_skipped)?);
    let full_tail = notes.tail(Arc::clone(&full_terminal));
    let full_tokens = details.probe_tail(total_skipped, &full_tail, page.work())?;
    let (shown_skips, selected_terminal, selected_tokens) = if full_tokens <= page.budget {
        (total_skipped, full_terminal, full_tokens)
    } else {
        let baseline_terminal = Arc::<str>::from(terminal_with_skips(terminal, total_skipped, 0)?);
        let baseline_tail = notes.tail(Arc::clone(&baseline_terminal));
        let baseline_tokens = details.probe_tail(0, &baseline_tail, page.work())?;
        if baseline_tokens > page.budget {
            return Ok(None);
        }

        let baseline = (0_usize, baseline_terminal, baseline_tokens);
        if total_skipped <= 1 {
            baseline
        } else {
            replay_compat_binary_probes(
                1,
                total_skipped - 1,
                Some(baseline.clone()),
                |middle| -> Result<Option<(usize, Arc<str>, usize)>, RenderPlanError> {
                    let candidate_terminal =
                        Arc::<str>::from(terminal_with_skips(terminal, total_skipped, middle)?);
                    let candidate_tail = notes.tail(Arc::clone(&candidate_terminal));
                    let candidate_tokens =
                        details.probe_tail(middle, &candidate_tail, page.work())?;
                    Ok((candidate_tokens <= page.budget).then_some((
                        middle,
                        candidate_terminal,
                        candidate_tokens,
                    )))
                },
            )?
            .unwrap_or(baseline)
        }
    };

    Ok(Some(SelectedGrepNotes {
        notes: notes.final_notes(shown_skips, selected_terminal),
        tokens: selected_tokens,
    }))
}

fn grep_render_failure(error: RenderPlanError) -> ToolResponse {
    if error.is_cancelled() {
        ToolResponse::error("Request cancelled.")
    } else {
        ToolResponse::error(format!("Internal grep rendering failure: {error}"))
    }
}

fn format_files_mode(results: &[FileResult], page: &PageFormat<'_>) -> ToolResponse {
    let initial = if page.head_limit == 0 {
        results.len()
    } else {
        page.head_limit.min(results.len())
    };
    let lines = results[..initial]
        .iter()
        .map(|result| Arc::<str>::from(result.path()))
        .collect::<Vec<_>>();
    let mut graph = match LineRenderGraph::new(lines, page.work()) {
        Ok(graph) => graph,
        Err(error) => return grep_render_failure(error),
    };
    let notes = GrepNoteUnits::new(page);
    let selected = select_body_prefix(&mut graph, initial, page, &notes, |shown| {
        let has_more = shown < results.len() || !page.scan_complete;
        paged_terminal(
            "file",
            "files",
            page.offset,
            shown,
            has_more,
            page.total_entries_seen,
        )
    });
    finish_selected_grep_body(&mut graph, selected, page, &notes)
}

fn format_count_mode(results: &[FileResult], page: &PageFormat<'_>) -> ToolResponse {
    let initial = if page.head_limit == 0 {
        results.len()
    } else {
        page.head_limit.min(results.len())
    };
    let mut occurrence_prefix = Vec::with_capacity(initial.saturating_add(1));
    occurrence_prefix.push(0_usize);
    let mut lines = Vec::with_capacity(initial);
    for result in &results[..initial] {
        let next = occurrence_prefix
            .last()
            .copied()
            .unwrap_or(0)
            .saturating_add(result.occurrence_count());
        occurrence_prefix.push(next);
        lines.push(Arc::<str>::from(format!(
            "{}:{}",
            result.path(),
            result.occurrence_count()
        )));
    }
    let mut graph = match LineRenderGraph::new(lines, page.work()) {
        Ok(graph) => graph,
        Err(error) => return grep_render_failure(error),
    };
    let notes = GrepNoteUnits::new(page);
    let selected = select_body_prefix(&mut graph, initial, page, &notes, |shown| {
        let has_more = shown < results.len() || !page.scan_complete;
        count_terminal(
            page.offset,
            shown,
            occurrence_prefix[shown],
            has_more,
            page.total_entries_seen,
        )
    });
    finish_selected_grep_body(&mut graph, selected, page, &notes)
}

fn format_content_mode(
    results: &[FileResult],
    request: &GrepRequest,
    page: &PageFormat<'_>,
    #[cfg(test)] metrics_out: Option<&mut ContentFormatMetrics>,
) -> ToolResponse {
    let entries = results
        .iter()
        .enumerate()
        .flat_map(|(file_index, result)| {
            result
                .content()
                .entries
                .iter()
                .copied()
                .map(move |entry| (file_index, entry))
        })
        .collect::<Vec<_>>();
    let initial = if page.head_limit == 0 {
        entries.len()
    } else {
        page.head_limit.min(entries.len())
    };
    let notes = GrepNoteUnits::new(page);
    let mut render_cache = ContentRenderCache::new();
    let render_page = |shown: usize| {
        let has_more = shown < entries.len() || !page.scan_complete;
        let terminal = paged_terminal(
            "result",
            "results",
            page.offset,
            shown,
            has_more,
            page.total_entries_seen,
        );
        render_content_page_with_degradation(
            results,
            &entries[..shown],
            request,
            page,
            &notes,
            &mut render_cache,
            terminal,
        )
    };
    let response = match fit_largest_content_output(initial, render_page) {
        Ok(Some(candidate)) => {
            match finish_content_grep_view(
                &mut render_cache.token_graph,
                &candidate.view,
                page,
                &notes,
                &candidate.terminal,
            ) {
                Ok(Some(text)) => ToolResponse::text(text),
                Ok(None) => budget_too_small(page.budget, page.budget_variable),
                Err(error) => grep_render_failure(error),
            }
        }
        Ok(None) => budget_too_small(page.budget, page.budget_variable),
        Err(ContentFormatError::Source(message)) => ToolResponse::error(message),
        Err(ContentFormatError::Render(error)) => grep_render_failure(error),
    };
    #[cfg(test)]
    if let Some(metrics_out) = metrics_out {
        let (render_units_built, render_bytes_built, plan_builds) = render_cache.metrics();
        *metrics_out = ContentFormatMetrics {
            render_units_built,
            render_bytes_built,
            plan_builds,
            token: render_cache.token_graph.metrics(),
        };
    }
    response
}

struct ContentBodyCandidate {
    view: LineRenderView,
    terminal: String,
}

enum ContentFormatError {
    Source(String),
    Render(RenderPlanError),
}

impl From<RenderPlanError> for ContentFormatError {
    fn from(error: RenderPlanError) -> Self {
        Self::Render(error)
    }
}

fn fit_largest_content_output(
    maximum: usize,
    mut render: impl FnMut(usize) -> Result<Option<ContentBodyCandidate>, ContentFormatError>,
) -> Result<Option<ContentBodyCandidate>, ContentFormatError> {
    if maximum == 0 {
        return Ok(None);
    }
    if let Some(output) = render(maximum)? {
        return Ok(Some(output));
    }
    replay_compat_binary_probes(1, maximum - 1, None, render)
}

fn render_content_page_with_degradation(
    results: &[FileResult],
    selected: &[(usize, ContentEntry)],
    request: &GrepRequest,
    page: &PageFormat<'_>,
    notes: &GrepNoteUnits,
    render_cache: &mut ContentRenderCache,
    terminal: String,
) -> Result<Option<ContentBodyCandidate>, ContentFormatError> {
    let (requested_before, requested_after) = requested_context(request);
    let maximum_context = requested_before.max(requested_after);
    let mut render = |context_depth: usize,
                      match_window: usize|
     -> Result<Option<ContentBodyCandidate>, ContentFormatError> {
        let lines = render_content_lines(
            results,
            selected,
            request,
            context_depth,
            match_window,
            page.single_file_target,
            render_cache,
        )
        .map_err(ContentFormatError::Source)?;
        let view = render_cache.token_graph.prepare_view(lines, page.work())?;
        let baseline_notes = notes.baseline_notes(&terminal)?;
        let tokens = render_cache
            .token_graph
            .probe_notes(&view, &baseline_notes, page.work())?;
        Ok((tokens <= page.budget).then_some(ContentBodyCandidate {
            view,
            terminal: terminal.clone(),
        }))
    };

    let full = render(maximum_context, MAX_MATCH_CHARS)?;
    if full.is_some() {
        return Ok(full);
    }

    let no_context = render(0, MAX_MATCH_CHARS)?;
    if let Some(no_context) = no_context {
        return replay_compat_binary_probes(0, maximum_context, Some(no_context), |middle| {
            render(middle, MAX_MATCH_CHARS)
        });
    }

    replay_compat_binary_probes(1, MAX_MATCH_CHARS - 1, None, |middle| render(0, middle))
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ContentPlanKey {
    shown: usize,
    before: usize,
    after: usize,
    only_matching: bool,
    single_file_target: bool,
}

#[derive(Clone)]
enum PlannedContentLine {
    Empty,
    Header(usize),
    Separator,
    Context {
        file_index: usize,
        line_number: usize,
    },
    Match {
        file_index: usize,
        line_number: usize,
        spans: Arc<[LineMatchSpan]>,
    },
    OnlyMatch {
        file_index: usize,
        start_line: usize,
        occurrence_index: usize,
        output_line: usize,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct MatchRenderKey {
    file_index: usize,
    line_number: usize,
    line_numbers: bool,
    match_window: usize,
    spans: Vec<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct OnlyMatchRenderKey {
    file_index: usize,
    start_line: usize,
    occurrence_index: usize,
    output_line: usize,
    line_numbers: bool,
    match_window: usize,
}

struct ContentRenderCache {
    token_graph: SharedLineRenderGraph,
    plans: HashMap<ContentPlanKey, Arc<[PlannedContentLine]>>,
    headers: HashMap<usize, Arc<str>>,
    contexts: HashMap<(usize, usize, bool), Arc<str>>,
    matches: HashMap<MatchRenderKey, Arc<str>>,
    only_matches: HashMap<OnlyMatchRenderKey, Arc<str>>,
    literals: HashMap<&'static str, Arc<str>>,
    #[cfg(test)]
    render_units_built: usize,
    #[cfg(test)]
    render_bytes_built: usize,
    #[cfg(test)]
    plan_builds: usize,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ContentFormatMetrics {
    render_units_built: usize,
    render_bytes_built: usize,
    plan_builds: usize,
    token: crate::render_plan::RenderPlanMetrics,
}

impl ContentRenderCache {
    fn new() -> Self {
        Self {
            token_graph: SharedLineRenderGraph::new(),
            plans: HashMap::new(),
            headers: HashMap::new(),
            contexts: HashMap::new(),
            matches: HashMap::new(),
            only_matches: HashMap::new(),
            literals: HashMap::new(),
            #[cfg(test)]
            render_units_built: 0,
            #[cfg(test)]
            render_bytes_built: 0,
            #[cfg(test)]
            plan_builds: 0,
        }
    }

    fn record_unit(&mut self, line: &str) {
        #[cfg(test)]
        {
            self.render_units_built = self.render_units_built.saturating_add(1);
            self.render_bytes_built = self.render_bytes_built.saturating_add(line.len());
        }
        #[cfg(not(test))]
        let _ = line;
    }

    fn literal(&mut self, value: &'static str) -> Arc<str> {
        if let Some(line) = self.literals.get(value) {
            return Arc::clone(line);
        }
        let line = Arc::<str>::from(value);
        self.record_unit(&line);
        self.literals.insert(value, Arc::clone(&line));
        line
    }

    fn header(&mut self, file_index: usize, result: &FileResult) -> Arc<str> {
        if let Some(line) = self.headers.get(&file_index) {
            return Arc::clone(line);
        }
        let line = Arc::<str>::from(result.path());
        self.record_unit(&line);
        self.headers.insert(file_index, Arc::clone(&line));
        line
    }

    fn context(
        &mut self,
        file_index: usize,
        line_number: usize,
        line_numbers: bool,
        result: &FileResult,
    ) -> io::Result<Arc<str>> {
        let key = (file_index, line_number, line_numbers);
        if let Some(line) = self.contexts.get(&key) {
            return Ok(Arc::clone(line));
        }
        let source = result_line(result, line_number).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "captured context line is missing",
            )
        })?;
        let rendered = format_context_line(context_prefix(line_number, line_numbers), source)?;
        let line = Arc::<str>::from(rendered);
        self.record_unit(&line);
        self.contexts.insert(key, Arc::clone(&line));
        Ok(line)
    }

    fn matching(
        &mut self,
        file_index: usize,
        line_number: usize,
        line_numbers: bool,
        match_window: usize,
        spans: &[LineMatchSpan],
        result: &FileResult,
    ) -> io::Result<Arc<str>> {
        let mut span_key = spans
            .iter()
            .map(|span| (span.match_char_start, span.match_char_len))
            .collect::<Vec<_>>();
        span_key.sort_unstable();
        let key = MatchRenderKey {
            file_index,
            line_number,
            line_numbers,
            match_window,
            spans: span_key,
        };
        if let Some(line) = self.matches.get(&key) {
            return Ok(Arc::clone(line));
        }
        let source = result_line(result, line_number).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "captured matching line is missing",
            )
        })?;
        let rendered = format_match_line(
            match_prefix(line_number, line_numbers),
            source,
            spans,
            match_window,
        )?;
        let line = Arc::<str>::from(rendered);
        self.record_unit(&line);
        self.matches.insert(key, Arc::clone(&line));
        Ok(line)
    }

    fn only_match(&mut self, key: OnlyMatchRenderKey, result: &FileResult) -> io::Result<Arc<str>> {
        if let Some(line) = self.only_matches.get(&key) {
            return Ok(Arc::clone(line));
        }
        let occurrence = result
            .content()
            .occurrences
            .get(&key.start_line)
            .and_then(|occurrences| occurrences.get(key.occurrence_index))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "captured occurrence is missing")
            })?;
        let rendered = format_only_match(
            match_prefix(key.output_line, key.line_numbers),
            occurrence.matched_text()?,
            key.match_window,
        );
        let line = Arc::<str>::from(rendered);
        self.record_unit(&line);
        self.only_matches.insert(key, Arc::clone(&line));
        Ok(line)
    }

    #[cfg(test)]
    fn metrics(&self) -> (usize, usize, usize) {
        (
            self.render_units_built,
            self.render_bytes_built,
            self.plan_builds,
        )
    }
}

fn render_content_lines(
    results: &[FileResult],
    selected: &[(usize, ContentEntry)],
    request: &GrepRequest,
    context_depth: usize,
    match_window: usize,
    single_file_target: bool,
    cache: &mut ContentRenderCache,
) -> Result<Vec<Arc<str>>, String> {
    let line_numbers = request.line_numbers.unwrap_or(true);
    let only_matching = request.only_matching.unwrap_or(false);
    let (requested_before, requested_after) = requested_context(request);
    let before = requested_before.min(context_depth);
    let after = requested_after.min(context_depth);
    let key = ContentPlanKey {
        shown: selected.len(),
        before,
        after,
        only_matching,
        single_file_target,
    };
    let plan = if let Some(plan) = cache.plans.get(&key) {
        Arc::clone(plan)
    } else {
        let plan = Arc::<[PlannedContentLine]>::from(build_content_plan(
            results,
            selected,
            before,
            after,
            only_matching,
            single_file_target,
        ));
        #[cfg(test)]
        {
            cache.plan_builds = cache.plan_builds.saturating_add(1);
        }
        cache.plans.insert(key, Arc::clone(&plan));
        plan
    };

    let mut lines = Vec::with_capacity(plan.len());
    for planned in plan.iter() {
        let rendered = match planned {
            PlannedContentLine::Empty => cache.literal(""),
            PlannedContentLine::Header(file_index) => {
                cache.header(*file_index, &results[*file_index])
            }
            PlannedContentLine::Separator => cache.literal("--"),
            PlannedContentLine::Context {
                file_index,
                line_number,
            } => cache
                .context(
                    *file_index,
                    *line_number,
                    line_numbers,
                    &results[*file_index],
                )
                .map_err(|error| stable_snapshot_error(results[*file_index].path(), &error))?,
            PlannedContentLine::Match {
                file_index,
                line_number,
                spans,
            } => cache
                .matching(
                    *file_index,
                    *line_number,
                    line_numbers,
                    match_window,
                    spans,
                    &results[*file_index],
                )
                .map_err(|error| stable_snapshot_error(results[*file_index].path(), &error))?,
            PlannedContentLine::OnlyMatch {
                file_index,
                start_line,
                occurrence_index,
                output_line,
            } => cache
                .only_match(
                    OnlyMatchRenderKey {
                        file_index: *file_index,
                        start_line: *start_line,
                        occurrence_index: *occurrence_index,
                        output_line: *output_line,
                        line_numbers,
                        match_window,
                    },
                    &results[*file_index],
                )
                .map_err(|error| stable_snapshot_error(results[*file_index].path(), &error))?,
        };
        lines.push(rendered);
    }
    Ok(lines)
}

fn build_content_plan(
    results: &[FileResult],
    selected: &[(usize, ContentEntry)],
    before: usize,
    after: usize,
    only_matching: bool,
    single_file_target: bool,
) -> Vec<PlannedContentLine> {
    let mut by_file = Vec::<(usize, Vec<ContentEntry>)>::new();
    for (file_index, entry) in selected {
        if let Some((last_file, entries)) = by_file.last_mut()
            && *last_file == *file_index
        {
            entries.push(*entry);
        } else {
            by_file.push((*file_index, vec![*entry]));
        }
    }
    let mut lines = Vec::new();
    for (group_index, (file_index, entries)) in by_file.into_iter().enumerate() {
        let result = &results[file_index];
        if !single_file_target {
            if group_index > 0 {
                lines.push(PlannedContentLine::Empty);
            }
            lines.push(PlannedContentLine::Header(file_index));
        }
        if only_matching {
            plan_only_matching_group(file_index, result, &entries, before, after, &mut lines);
        } else {
            plan_matching_line_group(file_index, result, &entries, before, after, &mut lines);
        }
    }
    lines
}

fn requested_context(request: &GrepRequest) -> (usize, usize) {
    if let Some(context) = request.context {
        (context, context)
    } else {
        (
            request.before_context.unwrap_or(0),
            request.after_context.unwrap_or(0),
        )
    }
}

fn plan_only_matching_group(
    file_index: usize,
    result: &FileResult,
    entries: &[ContentEntry],
    before: usize,
    after: usize,
    lines: &mut Vec<PlannedContentLine>,
) {
    let content = result.content();
    let mut occurrence_starts = BTreeMap::<usize, Vec<(usize, usize)>>::new();
    let mut match_ranges = Vec::new();
    let mut ranges = Vec::new();
    for entry in entries {
        for (start_line, occurrence_index) in occurrence_keys(result, *entry) {
            let occurrence = &content.occurrences[&start_line][occurrence_index];
            occurrence_starts
                .entry(occurrence.start_line)
                .or_default()
                .push((start_line, occurrence_index));
            match_ranges.push((occurrence.start_line, occurrence.end_line));
            ranges.push(context_range(
                occurrence.start_line,
                occurrence.end_line,
                before,
                after,
                content.total_lines,
            ));
        }
    }
    let ranges = merge_ranges(ranges);
    let match_ranges = merge_ranges(match_ranges);
    for (block_index, (start, end)) in ranges.into_iter().enumerate() {
        if block_index > 0 {
            lines.push(PlannedContentLine::Separator);
        }
        for line_number in start..=end {
            if let Some(keys) = occurrence_starts.get(&line_number) {
                for (start_line, occurrence_index) in keys {
                    lines.push(PlannedContentLine::OnlyMatch {
                        file_index,
                        start_line: *start_line,
                        occurrence_index: *occurrence_index,
                        output_line: line_number,
                    });
                }
                continue;
            }
            if ranges_contain(&match_ranges, line_number) {
                continue;
            }
            if result_line(result, line_number).is_some() {
                lines.push(PlannedContentLine::Context {
                    file_index,
                    line_number,
                });
            }
        }
    }
}

fn plan_matching_line_group(
    file_index: usize,
    result: &FileResult,
    entries: &[ContentEntry],
    before: usize,
    after: usize,
    lines: &mut Vec<PlannedContentLine>,
) {
    let content = result.content();
    let mut match_ranges = Vec::new();
    let mut spans = BTreeMap::<usize, Vec<LineMatchSpan>>::new();
    let mut ranges = Vec::new();
    for entry in entries {
        match *entry {
            ContentEntry::MatchingLine(line_number) => {
                match_ranges.push((line_number, line_number));
                if let Some(occurrences) = content.occurrences.get(&line_number) {
                    for occurrence in occurrences {
                        for span in &occurrence.line_spans {
                            spans.entry(span.line_number).or_default().push(*span);
                        }
                    }
                }
                ranges.push(context_range(
                    line_number,
                    line_number,
                    before,
                    after,
                    content.total_lines,
                ));
            }
            ContentEntry::Occurrence {
                start_line,
                occurrence_index,
            } => {
                let occurrence = &content.occurrences[&start_line][occurrence_index];
                match_ranges.push((occurrence.start_line, occurrence.end_line));
                for span in &occurrence.line_spans {
                    spans.entry(span.line_number).or_default().push(*span);
                }
                ranges.push(context_range(
                    occurrence.start_line,
                    occurrence.end_line,
                    before,
                    after,
                    content.total_lines,
                ));
            }
        }
    }
    let ranges = merge_ranges(ranges);
    let match_ranges = merge_ranges(match_ranges);
    for (block_index, (start, end)) in ranges.into_iter().enumerate() {
        if block_index > 0 {
            lines.push(PlannedContentLine::Separator);
        }
        for line_number in start..=end {
            if result_line(result, line_number).is_none() {
                continue;
            }
            if ranges_contain(&match_ranges, line_number) {
                lines.push(PlannedContentLine::Match {
                    file_index,
                    line_number,
                    spans: Arc::from(
                        spans
                            .get(&line_number)
                            .map(Vec::as_slice)
                            .unwrap_or(&[])
                            .to_vec(),
                    ),
                });
            } else {
                lines.push(PlannedContentLine::Context {
                    file_index,
                    line_number,
                });
            }
        }
    }
}

fn occurrence_keys(result: &FileResult, entry: ContentEntry) -> Vec<(usize, usize)> {
    match entry {
        ContentEntry::MatchingLine(line_number) => result
            .content()
            .occurrences
            .get(&line_number)
            .map(|occurrences| {
                (0..occurrences.len())
                    .map(|index| (line_number, index))
                    .collect()
            })
            .unwrap_or_default(),
        ContentEntry::Occurrence {
            start_line,
            occurrence_index,
        } => vec![(start_line, occurrence_index)],
    }
}

fn context_range(
    start_line: usize,
    end_line: usize,
    before: usize,
    after: usize,
    total_lines: usize,
) -> (usize, usize) {
    (
        start_line.saturating_sub(before).max(1),
        end_line.saturating_add(after).min(total_lines),
    )
}

fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut merged = Vec::<(usize, usize)>::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut()
            && start <= last.1.saturating_add(1)
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

fn ranges_contain(ranges: &[(usize, usize)], line_number: usize) -> bool {
    let index = ranges.partition_point(|(_, end)| *end < line_number);
    ranges
        .get(index)
        .is_some_and(|(start, end)| *start <= line_number && line_number <= *end)
}

#[derive(Clone, Copy)]
enum ResultLine<'a> {
    Captured(&'a CapturedLine),
    Empty,
}

impl<'a> ResultLine<'a> {
    fn as_str(self) -> io::Result<&'a str> {
        match self {
            Self::Captured(line) => line.as_str(),
            Self::Empty => Ok(""),
        }
    }

    fn byte_len(self) -> usize {
        match self {
            Self::Captured(line) => line.byte_len(),
            Self::Empty => 0,
        }
    }

    fn chars(self) -> io::Result<&'a [char]> {
        match self {
            Self::Captured(line) => line.chars(),
            Self::Empty => Ok(&[]),
        }
    }

    fn char_count(self) -> io::Result<usize> {
        match self {
            Self::Captured(line) => line.char_count(),
            Self::Empty => Ok(0),
        }
    }
}

fn result_line(result: &FileResult, line_number: usize) -> Option<ResultLine<'_>> {
    let content = result.content();
    content
        .lines
        .get(&line_number)
        .map(ResultLine::Captured)
        .or_else(|| {
            (line_number == content.total_lines && content.has_trailing_newline)
                .then_some(ResultLine::Empty)
        })
}

fn match_prefix(line_number: usize, line_numbers: bool) -> String {
    if line_numbers {
        format!("{line_number}:")
    } else {
        String::new()
    }
}

fn context_prefix(line_number: usize, line_numbers: bool) -> String {
    if line_numbers {
        format!("{line_number}-")
    } else {
        String::new()
    }
}

fn format_only_match(prefix: String, matched_text: &str, match_window: usize) -> String {
    let match_chars = matched_text.chars().count();
    let match_window = match_window.max(1);
    let shown = matched_text.chars().take(match_window).collect::<String>();
    let shown = shown
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    if match_chars <= match_window {
        format!("{prefix}{shown}")
    } else {
        format!("{prefix}{shown}... [match truncated: {match_chars} chars total]")
    }
}

fn format_match_line(
    prefix: String,
    line: ResultLine<'_>,
    match_spans: &[LineMatchSpan],
    match_window: usize,
) -> io::Result<String> {
    if line.byte_len() <= LONG_LINE_BYTES {
        return Ok(format!("{prefix}{}", line.as_str()?));
    }
    let chars = line.chars()?;
    let mut spans = match_spans
        .iter()
        .map(|span| {
            let start = span.match_char_start.min(chars.len());
            let end = start.saturating_add(span.match_char_len).min(chars.len());
            (start, end)
        })
        .collect::<Vec<_>>();
    if spans.is_empty() {
        spans.push((0, 0));
    }
    spans.sort_unstable();
    let first_start = spans[0].0;
    let first_end = spans[0].1;
    let last_end = spans.iter().map(|(_, end)| *end).max().unwrap_or(first_end);
    let desired_start = first_start.saturating_sub(MATCH_WINDOW_SIDE_CHARS);
    let desired_end = last_end
        .saturating_add(MATCH_WINDOW_SIDE_CHARS)
        .min(chars.len());
    let match_window = match_window.max(1);
    let (window_start, window_end) = if desired_end.saturating_sub(desired_start) <= match_window {
        (desired_start, desired_end)
    } else {
        let before = MATCH_WINDOW_SIDE_CHARS.min(match_window / 4);
        let mut start = first_start.saturating_sub(before);
        let mut end = start.saturating_add(match_window).min(chars.len());
        if end == chars.len() {
            start = end.saturating_sub(match_window).min(first_start);
            end = start.saturating_add(match_window).min(chars.len());
        }
        (start, end)
    };
    let first_match_truncated = first_end > window_end || first_start < window_start;
    let matches_outside = spans
        .iter()
        .any(|(start, end)| *start < window_start || *end > window_end);
    let mut output = prefix;
    if window_start > 0 {
        output.push('…');
    }
    output.extend(chars[window_start..window_end].iter());
    if first_match_truncated {
        output.push_str(&format!(
            "... [match truncated: {} chars total]",
            first_end.saturating_sub(first_start)
        ));
    }
    if window_end < chars.len() {
        output.push('…');
    }
    let outside_note = if spans.len() > 1 && matches_outside {
        "; additional matches fall outside this window"
    } else {
        ""
    };
    output.push_str(&format!(
        " [line is {} chars; showing window around match(es){outside_note}]",
        chars.len(),
    ));
    Ok(output)
}

fn format_context_line(prefix: String, line: ResultLine<'_>) -> io::Result<String> {
    if line.byte_len() <= LONG_LINE_BYTES {
        Ok(format!("{prefix}{}", line.as_str()?))
    } else {
        Ok(format!(
            "{prefix}[long line omitted: {} chars]",
            line.char_count()?
        ))
    }
}

fn terminal_with_skips(
    terminal: &str,
    skipped: usize,
    shown: usize,
) -> Result<String, RenderPlanError> {
    if skipped == 0 {
        return Ok(terminal.to_string());
    }
    let stem = terminal
        .strip_suffix(".)")
        .ok_or(RenderPlanError::InvalidTerminal)?;
    if shown == skipped {
        Ok(format!(
            "{stem}; {} skipped.)",
            counted(skipped, "file", "files")
        ))
    } else {
        Ok(format!(
            "{stem}; {} skipped, showing {shown} — narrow path/glob to inspect the rest.)",
            counted(skipped, "file", "files")
        ))
    }
}

fn format_summary(occurrences: usize, files: usize, page: &PageFormat<'_>) -> ToolResponse {
    let terminal = format!(
        "(Complete: {} across {}.)",
        counted(occurrences, "occurrence", "occurrences"),
        counted(files, "file", "files")
    );
    terminal_only_response(terminal, page)
}

fn zero_result(mode: OutputMode, page: &PageFormat<'_>) -> ToolResponse {
    let terminal = match mode {
        OutputMode::FilesWithMatches => "(Complete: no files matched.)",
        OutputMode::Content | OutputMode::Count => "(Complete: no matches found.)",
        OutputMode::Summary => unreachable!("summary has its own zero-count response"),
    };
    terminal_only_response(terminal.to_string(), page)
}

fn offset_exhausted(mode: OutputMode, page: &PageFormat<'_>) -> ToolResponse {
    let (singular, plural) = match mode {
        OutputMode::Content => ("result", "results"),
        OutputMode::FilesWithMatches | OutputMode::Count => ("file", "files"),
        OutputMode::Summary => unreachable!("summary ignores offset"),
    };
    let offset = page.offset;
    let total = page.total_entries_seen;
    let verb = if total == 1 { "exists" } else { "exist" };
    terminal_only_response(
        format!(
            "(Complete: no {plural} at offset={offset}; only {} {verb}.)",
            counted(total, singular, plural)
        ),
        page,
    )
}

fn terminal_only_response(terminal: String, page: &PageFormat<'_>) -> ToolResponse {
    let mut graph = match LineRenderGraph::new(Vec::new(), page.work()) {
        Ok(graph) => graph,
        Err(error) => return grep_render_failure(error),
    };
    let notes = GrepNoteUnits::new(page);
    match finish_grep_graph(&mut graph, 0, page, &notes, &terminal) {
        Ok(Some(text)) => ToolResponse::text(text),
        Ok(None) => budget_too_small(page.budget, page.budget_variable),
        Err(error) => grep_render_failure(error),
    }
}

fn paged_terminal(
    singular: &str,
    plural: &str,
    offset: usize,
    shown: usize,
    has_more: bool,
    total: usize,
) -> String {
    let range = entry_range(singular, plural, offset + 1, shown);
    if has_more {
        format!(
            "(Partial: {range} shown; more exist. Continue with offset={}.)",
            offset + shown
        )
    } else if offset == 0 {
        format!(
            "(Complete: all {} shown.)",
            counted(total, singular, plural)
        )
    } else {
        format!("(Complete: {range} shown; end of results.)")
    }
}

fn count_terminal(
    offset: usize,
    shown_files: usize,
    occurrences: usize,
    has_more: bool,
    total_files: usize,
) -> String {
    if has_more {
        format!(
            "(Partial: {} shown, page subtotal {}; more exist. Continue with offset={}.)",
            counted(shown_files, "file", "files"),
            counted(occurrences, "occurrence", "occurrences"),
            offset + shown_files
        )
    } else if offset == 0 {
        format!(
            "(Complete: {} across {}.)",
            counted(occurrences, "occurrence", "occurrences"),
            counted(total_files, "file", "files")
        )
    } else {
        format!(
            "(Complete: {} shown, page subtotal {}; end of results.)",
            entry_range("file", "files", offset + 1, shown_files),
            counted(occurrences, "occurrence", "occurrences")
        )
    }
}

fn entry_range(singular: &str, plural: &str, first: usize, shown: usize) -> String {
    if shown == 1 {
        format!("{singular} {first}")
    } else {
        format!("{plural} {first}-{}", first + shown - 1)
    }
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{count} {noun}")
}

fn budget_too_small(budget: usize, budget_variable: &str) -> ToolResponse {
    ErrorBudgetAdapter::new(budget, budget_variable).error(
        ErrorClass::Budget,
        format!(
            "{budget_variable}={budget} is too small to return the required grep continuation note. Increase it and retry."
        ),
    )
}

fn normalize_multiline_pattern(pattern: &str) -> String {
    let mut output = String::with_capacity(pattern.len());
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\n' {
            output.push_str("\\r?\\n");
            index += 1;
            continue;
        }
        if chars[index] != '\\' {
            output.push(chars[index]);
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len() && chars[index] == '\\' {
            index += 1;
        }
        let slash_count = index - start;
        if index < chars.len() && chars[index] == 'n' && slash_count % 2 == 1 {
            output.extend(std::iter::repeat_n('\\', slash_count - 1));
            output.push_str("\\r?\\n");
            index += 1;
        } else {
            output.extend(std::iter::repeat_n('\\', slash_count));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        CAPTURE_HEAP_LIMIT_BYTES, Candidate, ContentFormatMetrics, ContentRenderCache, ContentSpec,
        FallbackUsage, FileResult, GrepNoteUnits, GrepRequest, GrepSearchPlan, LineRenderGraph,
        OutputMode, PageFormat, SearchEncoding, SkippedFile, SkippedFiles, budget_too_small,
        build_matcher, capture_limit_error, finish_grep_graph, format_content_mode,
        format_files_mode, grep_files_with_budget, grep_files_with_budget_and_parallelism,
        grep_files_with_budget_source_and_operation, grep_files_with_parallelism_and_capture_limit,
        normalize_multiline_pattern, replay_compat_binary_probes, search_candidate,
        search_error_message, snapshot_error_message,
    };
    use crate::operation::{RequestWorkGuard, TestStage};
    use crate::{ToolContent, ToolResponse};
    use filetime::{FileTime, set_file_mtime};
    use rmcp::model::RequestId;
    use std::collections::BTreeSet;
    use std::io;
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn normalizes_regex_newlines_but_not_literal_backslashes() {
        assert_eq!(normalize_multiline_pattern(r"one\ntwo"), r"one\r?\ntwo");
        assert_eq!(normalize_multiline_pattern(r"one\\ntwo"), r"one\\ntwo");
        assert_eq!(normalize_multiline_pattern("one\ntwo"), r"one\r?\ntwo");
    }

    #[test]
    fn compatibility_binary_search_replays_v011_inclusive_probe_order() {
        let mut all_fit_probes = Vec::new();
        let best = replay_compat_binary_probes(1, 4, None, |middle| {
            all_fit_probes.push(middle);
            Ok::<_, ()>(Some(middle))
        })
        .unwrap();
        assert_eq!(all_fit_probes, [2, 3, 4]);
        assert_eq!(best, Some(4));

        let mut none_fit_probes = Vec::new();
        let best = replay_compat_binary_probes(1, 4, None, |middle| {
            none_fit_probes.push(middle);
            Ok::<_, ()>(None::<usize>)
        })
        .unwrap();
        assert_eq!(none_fit_probes, [2, 1]);
        assert_eq!(best, None);

        let mut non_monotonic_probes = Vec::new();
        let best = replay_compat_binary_probes(1, 4, Some(0), |middle| {
            non_monotonic_probes.push(middle);
            Ok::<_, ()>((middle == 1 || middle == 4).then_some(middle))
        })
        .unwrap();
        assert_eq!(non_monotonic_probes, [2, 1]);
        assert_eq!(best, Some(1));
    }

    #[test]
    fn content_render_cache_builds_each_literal_unit_once() {
        let mut cache = ContentRenderCache::new();
        let first = cache.literal("--");
        let second = cache.literal("--");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.metrics(), (1, 2, 0));
    }

    #[test]
    fn content_render_work_is_stable_when_head_limit_grows_beyond_the_same_page() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("content.txt");
        std::fs::write(&path, "hit\n".repeat(64)).unwrap();
        let candidate = Candidate::without_metadata(&path, temp.path());
        let matcher = build_matcher("hit", false, false).unwrap();
        let outcome = match search_candidate(
            &candidate,
            &matcher,
            GrepSearchPlan::ContentLine(ContentSpec {
                multiline: false,
                skip_entries: 0,
                max_selected_entries: Some(64),
                capture_match_text: false,
                before_context: 0,
                after_context: 0,
                capture_heap_limit_bytes: CAPTURE_HEAP_LIMIT_BYTES,
            }),
            false,
            &SearchEncoding {
                explicit: None,
                fallback: None,
            },
            None,
        ) {
            Ok(outcome) => outcome,
            Err(_) => panic!("fixture content search failed"),
        };
        let results = vec![outcome.result.unwrap()];
        let skipped_files = SkippedFiles::default();
        let transcoding_notes = BTreeSet::new();
        let fallback_usage = FallbackUsage::default();
        let request = GrepRequest {
            pattern: "hit".to_string(),
            path: Some(crate::paths::display_path(&path)),
            glob: None,
            file_type: None,
            output_mode: Some(OutputMode::Content),
            case_insensitive: None,
            line_numbers: None,
            only_matching: None,
            before_context: None,
            after_context: None,
            context: None,
            multiline: None,
            head_limit: None,
            offset: None,
            encoding: None,
            fallback_encoding: None,
        };
        let render = |head_limit, metrics: &mut ContentFormatMetrics| {
            let page = PageFormat {
                offset: 0,
                head_limit,
                budget: usize::MAX,
                budget_variable: "FASTCTX_TOKEN_BUDGET",
                scan_complete: true,
                total_entries_seen: 64,
                skipped_files: &skipped_files,
                transcoding_notes: &transcoding_notes,
                fallback_usage: &fallback_usage,
                single_file_target: true,
                operation: None,
            };
            format_content_mode(&results, &request, &page, Some(metrics))
        };
        let mut at_250 = ContentFormatMetrics::default();
        let mut at_1000 = ContentFormatMetrics::default();
        let response_250 = render(250, &mut at_250);
        let response_1000 = render(1_000, &mut at_1000);
        assert_eq!(response_250, response_1000);
        assert_eq!(at_250, at_1000);
        assert_eq!(at_250.plan_builds, 1);
        assert_eq!(at_250.render_units_built, 64);
        assert_eq!(at_250.token.full_tokenizer_calls, 1);
        assert!(at_250.token.token_prefix_appends <= at_250.render_units_built * 2);
    }

    #[test]
    fn tiny_skip_budgets_terminate_for_every_mode_under_a_hard_deadline() {
        const CHILD_MARKER: &str = "FASTCTX_TEST_TINY_SKIP_BUDGET_CHILD";
        if std::env::var_os(CHILD_MARKER).is_some() {
            for skipped_count in [1_usize, 2, 100] {
                let skipped_files = SkippedFiles {
                    entries: (0..skipped_count)
                        .map(|index| SkippedFile {
                            path: format!("/skip/{index:03}.txt"),
                            reason: "undecodable".to_string(),
                        })
                        .collect(),
                };
                let transcoding_notes = BTreeSet::new();
                let fallback_usage = FallbackUsage::default();
                for budget in [1_usize, 2] {
                    let page = PageFormat {
                        offset: 0,
                        head_limit: 1,
                        budget,
                        budget_variable: "FASTCTX_TOKEN_BUDGET",
                        scan_complete: true,
                        total_entries_seen: 1,
                        skipped_files: &skipped_files,
                        transcoding_notes: &transcoding_notes,
                        fallback_usage: &fallback_usage,
                        single_file_target: false,
                        operation: None,
                    };
                    let result = FileResult::count("/match.txt".to_string(), 1);
                    let files = format_files_mode(std::slice::from_ref(&result), &page);
                    let count = super::format_count_mode(std::slice::from_ref(&result), &page);
                    let summary = super::terminal_only_response(
                        "(Complete: 1 occurrence across 1 file.)".to_string(),
                        &page,
                    );
                    let mut graph =
                        LineRenderGraph::new(vec![Arc::<str>::from("1:hit")], page.work()).unwrap();
                    let notes = GrepNoteUnits::new(&page);
                    let content = match finish_grep_graph(
                        &mut graph,
                        1,
                        &page,
                        &notes,
                        "(Complete: all 1 result shown.)",
                    )
                    .unwrap()
                    {
                        Some(text) => ToolResponse::text(text),
                        None => budget_too_small(page.budget, page.budget_variable),
                    };
                    for response in [files, count, summary, content] {
                        assert!(response.is_error, "budget={budget}, response={response:?}");
                        let [ToolContent::Text(text)] = response.content.as_slice() else {
                            panic!("expected one text error");
                        };
                        assert!(
                            tiktoken_rs::o200k_base_singleton()
                                .encode_ordinary(text)
                                .len()
                                <= budget,
                            "independent oracle exceeded budget={budget}, text={text:?}"
                        );
                    }
                }
            }
            return;
        }

        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .arg("--exact")
            .arg("grep_tool::tests::tiny_skip_budgets_terminate_for_every_mode_under_a_hard_deadline")
            .arg("--nocapture")
            .env(CHILD_MARKER, "1");
        let mut child = child.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "tiny-budget child failed: {status}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let status = child.wait().unwrap();
                panic!("tiny-budget child exceeded its hard deadline: {status}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn token_budget_returns_at_least_one_entry_and_an_exact_offset() {
        let results = (1..=3)
            .map(|index| FileResult::count(format!("{index}-{}", "x".repeat(100)), 1))
            .collect::<Vec<_>>();
        let skipped_files = SkippedFiles::default();
        let transcoding_notes = BTreeSet::new();
        let fallback_usage = FallbackUsage::default();
        let page = PageFormat {
            offset: 0,
            head_limit: 0,
            budget: 65,
            budget_variable: "FASTCTX_TOKEN_BUDGET",
            scan_complete: false,
            total_entries_seen: 0,
            skipped_files: &skipped_files,
            transcoding_notes: &transcoding_notes,
            fallback_usage: &fallback_usage,
            single_file_target: false,
            operation: None,
        };
        let response = format_files_mode(&results, &page);
        assert!(!response.is_error, "{response:?}");
        let ToolContent::Text(output) = &response.content[0] else {
            panic!("expected text");
        };
        assert!(output.starts_with("1-"));
        let shown = output.lines().take_while(|line| !line.is_empty()).count();
        assert!((1..=3).contains(&shown), "{output}");
        let range = if shown == 1 {
            "file 1".to_string()
        } else {
            format!("files 1-{shown}")
        };
        assert!(
            output.ends_with(&format!(
                "(Partial: {range} shown; more exist. Continue with offset={shown}.)"
            )),
            "{output}"
        );
    }

    #[test]
    fn tiny_budget_fails_instead_of_returning_an_empty_success() {
        let results = vec![FileResult::count("/a/very/long/path.txt".to_string(), 1)];
        let skipped_files = SkippedFiles::default();
        let transcoding_notes = BTreeSet::new();
        let fallback_usage = FallbackUsage::default();
        let page = PageFormat {
            offset: 0,
            head_limit: 1,
            budget: 1,
            budget_variable: "FASTCTX_TOKEN_BUDGET",
            scan_complete: true,
            total_entries_seen: 1,
            skipped_files: &skipped_files,
            transcoding_notes: &transcoding_notes,
            fallback_usage: &fallback_usage,
            single_file_target: false,
            operation: None,
        };
        let response = format_files_mode(&results, &page);
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
    fn tiny_budget_bounds_real_not_found_regex_encoding_and_cancel_errors() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("text.txt");
        std::fs::write(&file, b"hit\n").unwrap();
        let request = |path: String| GrepRequest {
            pattern: "hit".to_string(),
            path: Some(path),
            glob: None,
            file_type: None,
            output_mode: Some(OutputMode::FilesWithMatches),
            case_insensitive: None,
            line_numbers: None,
            only_matching: None,
            before_context: None,
            after_context: None,
            context: None,
            multiline: None,
            head_limit: None,
            offset: None,
            encoding: None,
            fallback_encoding: None,
        };

        let not_found = grep_files_with_budget(
            request(crate::paths::display_path(&temp.path().join("missing.txt"))),
            1,
        );
        let mut invalid_regex = request(crate::paths::display_path(&file));
        invalid_regex.pattern = "[".to_string();
        let invalid_regex = grep_files_with_budget(invalid_regex, 1);
        let mut invalid_encoding = request(crate::paths::display_path(&file));
        invalid_encoding.encoding = Some("not-a-real-encoding".to_string());
        let invalid_encoding = grep_files_with_budget(invalid_encoding, 1);

        let parent = CancellationToken::new();
        let (mut guard, operation) =
            RequestWorkGuard::new(RequestId::String(Arc::from("tiny-cancel")), parent.clone());
        parent.cancel();
        let cancelled = grep_files_with_budget_source_and_operation(
            request(crate::paths::display_path(&file)),
            1,
            "FASTCTX_TOKEN_BUDGET",
            Some(&operation),
        );
        guard.disarm();

        for response in [not_found, invalid_regex, invalid_encoding, cancelled] {
            assert!(response.is_error, "{response:?}");
            let [ToolContent::Text(text)] = response.content.as_slice() else {
                panic!("expected one text error");
            };
            assert!(
                tiktoken_rs::o200k_base_singleton()
                    .encode_ordinary(text)
                    .len()
                    <= 1,
                "{text:?}"
            );
        }
    }

    #[test]
    fn encoding_skip_report_uses_remaining_budget_and_keeps_the_terminal_truthful() {
        let paths = (0..3)
            .map(|index| format!("/{}-{index}.txt", "a".repeat(80)))
            .collect::<Vec<_>>();
        let skipped = SkippedFiles {
            entries: paths
                .iter()
                .map(|path| SkippedFile {
                    path: path.clone(),
                    reason: "ambiguous: windows-1252".to_string(),
                })
                .collect(),
        };
        let results = vec![FileResult::count("/match.txt".to_string(), 1)];
        let transcoding_notes = BTreeSet::new();
        let fallback_usage = FallbackUsage::default();
        let page = PageFormat {
            offset: 0,
            head_limit: 1,
            budget: 70,
            budget_variable: "FASTCTX_TOKEN_BUDGET",
            scan_complete: true,
            total_entries_seen: 1,
            skipped_files: &skipped,
            transcoding_notes: &transcoding_notes,
            fallback_usage: &fallback_usage,
            single_file_target: false,
            operation: None,
        };
        let response = format_files_mode(&results, &page);
        assert!(!response.is_error, "{response:?}");
        let [ToolContent::Text(output)] = response.content.as_slice() else {
            panic!("expected one text success");
        };
        let expected = format!(
            "/match.txt\n\n{} — ambiguous: windows-1252\n(Complete: all 1 file shown; 3 files skipped, showing 1 — narrow path/glob to inspect the rest.)",
            paths[0]
        );
        assert_eq!(output.as_str(), expected);
    }

    #[test]
    fn real_multi_file_skip_report_samples_in_deterministic_order_with_full_counts() {
        let temp = tempfile::tempdir().unwrap();
        let matches = temp.path().join("matches.txt");
        std::fs::write(&matches, b"hit\n").unwrap();
        set_file_mtime(&matches, FileTime::from_unix_time(1_700_000_001, 0)).unwrap();

        let mut ambiguous = Vec::new();
        for index in 0..3 {
            let path = temp.path().join(format!("ambiguous-{index}.txt"));
            std::fs::write(&path, b"valid\xFFtail").unwrap();
            set_file_mtime(
                &path,
                FileTime::from_unix_time(1_700_000_100 - index as i64, 0),
            )
            .unwrap();
            ambiguous.push(path);
        }

        let independent_display = |path: &std::path::Path| {
            dunce::canonicalize(path)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        };
        let match_path = independent_display(&matches);
        let skipped_paths = ambiguous
            .iter()
            .map(|path| independent_display(path))
            .collect::<Vec<_>>();
        let full_expected = format!(
            "{match_path}\n1:hit\n\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n(Complete: all 1 result shown; 3 files skipped.)",
            skipped_paths[0], skipped_paths[1], skipped_paths[2]
        );
        let expected = format!(
            "{match_path}\n1:hit\n\n{} — ambiguous: windows-1252\n(Complete: all 1 result shown; 3 files skipped, showing 1 — narrow path/glob to inspect the rest.)",
            skipped_paths[0]
        );
        let two_shown = format!(
            "{match_path}\n1:hit\n\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n(Complete: all 1 result shown; 3 files skipped, showing 2 — narrow path/glob to inspect the rest.)",
            skipped_paths[0], skipped_paths[1]
        );
        let budget = bpe_openai::o200k_base().count(&expected);
        assert!(bpe_openai::o200k_base().count(&two_shown) > budget);

        let request = GrepRequest {
            pattern: "hit".to_string(),
            path: Some(temp.path().to_string_lossy().replace('\\', "/")),
            glob: None,
            file_type: None,
            output_mode: Some(OutputMode::Content),
            case_insensitive: None,
            line_numbers: None,
            only_matching: None,
            before_context: None,
            after_context: None,
            context: None,
            multiline: None,
            head_limit: None,
            offset: None,
            encoding: None,
            fallback_encoding: None,
        };
        let full_response = grep_files_with_budget(request.clone(), 100_000);
        assert!(!full_response.is_error, "{full_response:?}");
        assert_eq!(
            full_response.content,
            vec![ToolContent::Text(full_expected)]
        );

        let response = grep_files_with_budget(request, budget);
        assert!(!response.is_error, "{response:?}");
        assert_eq!(response.content, vec![ToolContent::Text(expected)]);
    }

    #[test]
    fn unlimited_head_limit_still_stops_at_the_text_budget() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("many.txt");
        std::fs::write(&path, "hit\n".repeat(100)).unwrap();
        let response = grep_files_with_budget(
            GrepRequest {
                pattern: "hit".to_string(),
                path: Some(crate::paths::display_path(&path)),
                glob: None,
                file_type: None,
                output_mode: Some(OutputMode::Content),
                case_insensitive: None,
                line_numbers: None,
                only_matching: None,
                before_context: None,
                after_context: None,
                context: None,
                multiline: None,
                head_limit: Some(0),
                offset: None,
                encoding: None,
                fallback_encoding: None,
            },
            30,
        );
        assert_eq!(
            response.content,
            vec![ToolContent::Text(
                "1:hit\n2:hit\n\n(Partial: results 1-2 shown; more exist. Continue with offset=2.)"
                    .to_string()
            )]
        );
    }

    #[test]
    fn oversized_context_is_reduced_before_the_match_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("context.txt");
        let mut lines = (1..=500)
            .map(|index| format!("before-{index}"))
            .collect::<Vec<_>>();
        lines.push("NEEDLE".to_string());
        lines.extend((1..=500).map(|index| format!("after-{index}")));
        std::fs::write(&path, lines.join("\n")).unwrap();
        let response = grep_files_with_budget(
            GrepRequest {
                pattern: "NEEDLE".to_string(),
                path: Some(crate::paths::display_path(&path)),
                glob: None,
                file_type: None,
                output_mode: Some(OutputMode::Content),
                case_insensitive: None,
                line_numbers: None,
                only_matching: None,
                before_context: None,
                after_context: None,
                context: Some(10_000),
                multiline: None,
                head_limit: None,
                offset: None,
                encoding: None,
                fallback_encoding: None,
            },
            60,
        );
        assert!(!response.is_error, "{response:?}");
        let ToolContent::Text(output) = &response.content[0] else {
            panic!("expected text");
        };
        assert!(output.contains("501:NEEDLE"), "{output}");
        assert!(!output.contains("1-before-1"), "{output}");
        assert!(
            output.ends_with("(Complete: all 1 result shown.)"),
            "{output}"
        );
    }

    #[test]
    fn cancellation_before_regex_search_never_enters_a_sink_or_returns_success() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cancel-before-regex.txt");
        std::fs::write(&path, b"hit\nhit again\n").unwrap();
        let cancellation = CancellationToken::new();
        let cancellation_for_hook = cancellation.clone();
        let before_regex_hits = Arc::new(AtomicUsize::new(0));
        let before_regex_hits_for_hook = Arc::clone(&before_regex_hits);
        let sink_hits = Arc::new(AtomicUsize::new(0));
        let sink_hits_for_hook = Arc::clone(&sink_hits);
        let hook = Arc::new(move |stage| match stage {
            TestStage::BeforeRegexSearch
                if before_regex_hits_for_hook.fetch_add(1, Ordering::AcqRel) == 0 =>
            {
                cancellation_for_hook.cancel();
            }
            TestStage::SinkMatch => {
                sink_hits_for_hook.fetch_add(1, Ordering::AcqRel);
            }
            _ => {}
        });
        let (mut guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(40), cancellation, hook);

        let response = grep_files_with_budget_source_and_operation(
            grep_request(&path, OutputMode::Content),
            100_000,
            "FASTCTX_TOKEN_BUDGET",
            Some(&operation),
        );
        guard.disarm();

        assert!(response.is_error, "{response:?}");
        assert_eq!(
            response.content,
            vec![ToolContent::Text("Request cancelled.".to_string())]
        );
        assert_eq!(before_regex_hits.load(Ordering::Acquire), 1);
        assert_eq!(sink_hits.load(Ordering::Acquire), 0);
    }

    #[test]
    fn changing_single_file_returns_the_exact_retry_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("changing.txt");
        std::fs::write(&path, b"hit\n").unwrap();
        let path_for_hook = path.clone();
        let changed = Arc::new(AtomicBool::new(false));
        let changed_for_hook = Arc::clone(&changed);
        let hook = Arc::new(move |stage| {
            if stage == TestStage::BeforeIdentityPostCheck
                && !changed_for_hook.swap(true, Ordering::AcqRel)
            {
                std::fs::write(&path_for_hook, b"hit from a different version\nhit again\n")
                    .unwrap();
            }
        });
        let (mut guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(41), CancellationToken::new(), hook);
        let response = grep_files_with_budget_source_and_operation(
            grep_request(&path, OutputMode::Content),
            100_000,
            "FASTCTX_TOKEN_BUDGET",
            Some(&operation),
        );
        guard.disarm();
        assert!(response.is_error, "{response:?}");
        assert_eq!(
            response.content,
            vec![ToolContent::Text(format!(
                "File changed while it was being searched: {}. Retry the grep request.",
                crate::path_codec::display_path(&dunce::canonicalize(&path).unwrap())
            ))]
        );
        assert!(changed.load(Ordering::Acquire));
    }

    #[test]
    fn changing_directory_candidate_is_one_unified_skip_in_all_four_modes() {
        for (request_id, mode) in [
            (51, OutputMode::FilesWithMatches),
            (52, OutputMode::Count),
            (53, OutputMode::Content),
            (54, OutputMode::Summary),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let stable = temp.path().join("stable.txt");
            let changing = temp.path().join("changing.txt");
            std::fs::write(&stable, b"hit\n").unwrap();
            std::fs::write(&changing, b"hit\n").unwrap();
            set_file_mtime(&stable, FileTime::from_unix_time(1_700_000_200, 0)).unwrap();
            set_file_mtime(&changing, FileTime::from_unix_time(1_700_000_100, 0)).unwrap();

            let changing_for_hook = changing.clone();
            let changed = Arc::new(AtomicBool::new(false));
            let changed_for_hook = Arc::clone(&changed);
            let hook = Arc::new(move |stage| {
                if stage == TestStage::BeforeIdentityPostCheck
                    && !changed_for_hook.swap(true, Ordering::AcqRel)
                {
                    std::fs::write(
                        &changing_for_hook,
                        b"hit from a different version\nhit that must not leak\n",
                    )
                    .unwrap();
                }
            });
            let (mut guard, operation) = RequestWorkGuard::new_with_hook(
                RequestId::Number(request_id),
                CancellationToken::new(),
                hook,
            );
            let response = grep_files_with_budget_source_and_operation(
                grep_request(temp.path(), mode),
                100_000,
                "FASTCTX_TOKEN_BUDGET",
                Some(&operation),
            );
            guard.disarm();
            assert!(!response.is_error, "{mode:?}: {response:?}");
            assert!(changed.load(Ordering::Acquire), "{mode:?}");

            let stable_display =
                crate::path_codec::display_path(&dunce::canonicalize(&stable).unwrap());
            let changing_display =
                crate::path_codec::display_path(&dunce::canonicalize(&changing).unwrap());
            let body = match mode {
                OutputMode::FilesWithMatches => stable_display.clone(),
                OutputMode::Count => format!("{stable_display}:1"),
                OutputMode::Content => format!("{stable_display}\n1:hit"),
                OutputMode::Summary => String::new(),
            };
            let terminal = match mode {
                OutputMode::FilesWithMatches => "(Complete: all 1 file shown; 1 file skipped.)",
                OutputMode::Count | OutputMode::Summary => {
                    "(Complete: 1 occurrence across 1 file; 1 file skipped.)"
                }
                OutputMode::Content => "(Complete: all 1 result shown; 1 file skipped.)",
            };
            let expected = if body.is_empty() {
                format!("{changing_display} — changed while being searched\n{terminal}")
            } else {
                format!("{body}\n\n{changing_display} — changed while being searched\n{terminal}")
            };
            assert_eq!(
                response.content,
                vec![ToolContent::Text(expected)],
                "{mode:?}"
            );
        }
    }

    #[test]
    fn terminal_encoding_rejection_keeps_the_exact_single_file_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("malformed.txt");
        std::fs::write(&path, [b'a', b'b', b'c', 0xFF]).unwrap();
        let mut request = grep_request(&path, OutputMode::Content);
        request.encoding = Some("utf-8".to_string());
        let response = grep_files_with_budget(request, 100_000);
        assert!(response.is_error, "{response:?}");
        assert_eq!(
            response.content,
            vec![ToolContent::Text(format!(
                "Cannot decode {} as utf-8: the content is not valid utf-8. Try another encoding or view=\"hex\".",
                crate::path_codec::display_path(&dunce::canonicalize(&path).unwrap())
            ))]
        );
    }

    #[test]
    fn encoding_and_changed_candidates_share_one_ordered_skip_report_in_all_modes() {
        for (request_id, mode) in [
            (61, OutputMode::FilesWithMatches),
            (62, OutputMode::Count),
            (63, OutputMode::Content),
            (64, OutputMode::Summary),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let stable = temp.path().join("stable.txt");
            let mixed = temp.path().join("mixed.txt");
            let changing = temp.path().join("changing.txt");
            std::fs::write(&stable, b"hit\n").unwrap();
            let mut mixed_bytes = "界".repeat(11).into_bytes();
            mixed_bytes.push(0xFF);
            mixed_bytes.resize(8 * 1024, b'a');
            std::fs::write(&mixed, mixed_bytes).unwrap();
            std::fs::write(&changing, b"hit\n").unwrap();
            set_file_mtime(&stable, FileTime::from_unix_time(1_700_000_300, 0)).unwrap();
            set_file_mtime(&mixed, FileTime::from_unix_time(1_700_000_200, 0)).unwrap();
            set_file_mtime(&changing, FileTime::from_unix_time(1_700_000_100, 0)).unwrap();

            let changing_for_hook = changing.clone();
            let changed = Arc::new(AtomicBool::new(false));
            let changed_for_hook = Arc::clone(&changed);
            let hook = Arc::new(move |stage| {
                if stage == TestStage::BeforeIdentityPostCheck
                    && !changed_for_hook.swap(true, Ordering::AcqRel)
                {
                    std::fs::write(
                        &changing_for_hook,
                        b"hit from a different version\nhit that must not leak\n",
                    )
                    .unwrap();
                }
            });
            let (mut guard, operation) = RequestWorkGuard::new_with_hook(
                RequestId::Number(request_id),
                CancellationToken::new(),
                hook,
            );
            let response = grep_files_with_budget_source_and_operation(
                grep_request(temp.path(), mode),
                100_000,
                "FASTCTX_TOKEN_BUDGET",
                Some(&operation),
            );
            guard.disarm();
            assert!(!response.is_error, "{mode:?}: {response:?}");
            assert!(changed.load(Ordering::Acquire), "{mode:?}");

            let stable_display =
                crate::path_codec::display_path(&dunce::canonicalize(&stable).unwrap());
            let mixed_display =
                crate::path_codec::display_path(&dunce::canonicalize(&mixed).unwrap());
            let changing_display =
                crate::path_codec::display_path(&dunce::canonicalize(&changing).unwrap());
            let body = match mode {
                OutputMode::FilesWithMatches => stable_display.clone(),
                OutputMode::Count => format!("{stable_display}:1"),
                OutputMode::Content => format!("{stable_display}\n1:hit"),
                OutputMode::Summary => String::new(),
            };
            let terminal = match mode {
                OutputMode::FilesWithMatches => "(Complete: all 1 file shown; 2 files skipped.)",
                OutputMode::Count | OutputMode::Summary => {
                    "(Complete: 1 occurrence across 1 file; 2 files skipped.)"
                }
                OutputMode::Content => "(Complete: all 1 result shown; 2 files skipped.)",
            };
            let skips = format!(
                "{mixed_display} — mixed or inconsistent encodings\n{changing_display} — changed while being searched"
            );
            let expected = if body.is_empty() {
                format!("{skips}\n{terminal}")
            } else {
                format!("{body}\n\n{skips}\n{terminal}")
            };
            assert_eq!(
                response.content,
                vec![ToolContent::Text(expected)],
                "{mode:?}"
            );
        }
    }

    #[test]
    fn p1_and_p4_are_byte_identical_across_all_modes_and_paging() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..24 {
            let path = temp.path().join(format!("candidate-{index:02}.txt"));
            std::fs::write(
                &path,
                format!("before {index}\nhit {index}\nhit again {index}\nafter {index}\n"),
            )
            .unwrap();
            set_file_mtime(
                &path,
                FileTime::from_unix_time(1_700_100_000 + index as i64, 0),
            )
            .unwrap();
        }

        let mut requests = [
            OutputMode::FilesWithMatches,
            OutputMode::Count,
            OutputMode::Content,
            OutputMode::Summary,
        ]
        .into_iter()
        .map(|mode| grep_request(temp.path(), mode))
        .collect::<Vec<_>>();
        requests[0].offset = Some(3);
        requests[0].head_limit = Some(7);
        requests[1].offset = Some(4);
        requests[1].head_limit = Some(9);
        requests[2].offset = Some(5);
        requests[2].head_limit = Some(11);
        requests[2].context = Some(1);

        let mut occurrences = grep_request(temp.path(), OutputMode::Content);
        occurrences.only_matching = Some(true);
        occurrences.offset = Some(7);
        occurrences.head_limit = Some(13);
        requests.push(occurrences);

        for request in requests {
            let serial = grep_files_with_budget_and_parallelism(request.clone(), 100_000, 1);
            let parallel = grep_files_with_budget_and_parallelism(request, 100_000, 4);
            assert_eq!(parallel, serial);
        }
    }

    #[test]
    fn overcapture_overflow_retries_the_live_window_inline_and_matches_p1() {
        let temp = tempfile::tempdir().unwrap();
        let frontier = temp.path().join("frontier.txt");
        let mut frontier_text = String::new();
        for line in 1..=120 {
            frontier_text.push_str(&format!("hit {line:03} {}\n", "x".repeat(64)));
        }
        std::fs::write(&frontier, frontier_text).unwrap();
        set_file_mtime(&frontier, FileTime::from_unix_time(1_700_200_100, 0)).unwrap();
        for index in 0..3 {
            let path = temp.path().join(format!("future-{index}.txt"));
            std::fs::write(&path, format!("hit future {index}\n")).unwrap();
            set_file_mtime(
                &path,
                FileTime::from_unix_time(1_700_200_000 + index as i64, 0),
            )
            .unwrap();
        }

        let mut request = grep_request(temp.path(), OutputMode::Content);
        request.offset = Some(80);
        request.head_limit = Some(1);
        let (serial, serial_retries, serial_burst, serial_tickets) =
            grep_files_with_parallelism_and_capture_limit(request.clone(), 100_000, 1, 1_024);
        let (parallel, parallel_retries, parallel_burst, parallel_tickets) =
            grep_files_with_parallelism_and_capture_limit(request, 100_000, 4, 1_024);

        assert!(!serial.is_error, "{serial:?}");
        assert_eq!(parallel, serial);
        assert_eq!(serial_retries, 1);
        assert_eq!(parallel_retries, 1);
        for ledger in [
            serial_burst,
            serial_tickets,
            parallel_burst,
            parallel_tickets,
        ] {
            assert_eq!(ledger.released, ledger.allocated);
            assert_eq!(ledger.live, 0);
            assert_eq!(ledger.duplicate_releases, 0);
        }
        let ToolContent::Text(output) = &parallel.content[0] else {
            panic!("expected text")
        };
        assert!(output.contains("81:hit 081"), "{output}");
        assert!(output.ends_with("Continue with offset=81.)"), "{output}");

        let mut exact_overflow = grep_request(temp.path(), OutputMode::Content);
        exact_overflow.offset = Some(80);
        exact_overflow.head_limit = Some(1);
        let (error, retries, burst, tickets) =
            grep_files_with_parallelism_and_capture_limit(exact_overflow, 100_000, 4, 8);
        assert!(error.is_error, "{error:?}");
        assert_eq!(retries, 1, "an exact retry must never retry itself");
        for ledger in [burst, tickets] {
            assert_eq!(ledger.released, ledger.allocated);
            assert_eq!(ledger.live, 0);
            assert_eq!(ledger.duplicate_releases, 0);
        }
    }

    fn grep_request(path: &Path, mode: OutputMode) -> GrepRequest {
        GrepRequest {
            pattern: "hit".to_string(),
            path: Some(crate::paths::display_path(path)),
            glob: None,
            file_type: None,
            output_mode: Some(mode),
            case_insensitive: None,
            line_numbers: None,
            only_matching: None,
            before_context: None,
            after_context: None,
            context: None,
            multiline: None,
            head_limit: None,
            offset: None,
            encoding: None,
            fallback_encoding: None,
        }
    }

    #[test]
    fn search_memory_limits_have_exact_actionable_errors() {
        let candidate = Candidate::without_metadata(Path::new("/large.txt"), Path::new("/"));
        assert_eq!(
            search_error_message(&candidate, &io::Error::other("heap limit reached")),
            "Cannot search file /large.txt: a line or multiline buffer exceeds the 64 MiB safety limit. Narrow the path or search without multiline."
        );
        assert_eq!(
            capture_limit_error(&candidate),
            "Cannot search file /large.txt: matching content and context exceed the 64 MiB safety limit. Narrow the pattern or reduce context."
        );
        assert_eq!(
            snapshot_error_message(&candidate, &io::Error::other("disk full")),
            "Cannot create a stable search snapshot for /large.txt: disk full. Free temporary-disk space or retry after the file stops changing."
        );
    }
}
