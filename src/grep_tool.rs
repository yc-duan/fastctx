//! grep tool backed by ripgrep engines, ignore traversal, deterministic paging, and content formatting.

use crate::budget::{
    ErrorBudgetAdapter, ErrorClass, GREP_TOKEN_BUDGET_ENV, error_budget_hint, tool_token_budget,
};
use crate::encoding::{
    ByteSource, EncodingDecision, EncodingPipelineFailure, EncodingRejection,
    canonical_encoding_label, validate_snapshot_encoding,
};
use crate::file_executor::GrepGlobExecutor;
use crate::file_snapshot::{CaptureDisposition, CaptureFailure, capture_classify};
use crate::glob_filter::{GlobPatterns, PathGlobFilter};
use crate::grep_sink::{
    CapturedLine, ContentEntry, ContentSpec, FileResult, GrepSearchPlan, GrepSinkError,
    LineMatchSpan, PlanSink,
};
use crate::head_note::{CoverageTotal, CoveredRange, HeadMetric, HeadNote};
use crate::model::ToolResponse;
use crate::operation::{
    OpError, OperationCtx, RequestWorkGuard, WorkCheckpoint, WorkCtx, WorkStop,
};
use crate::ordered_window::{OrderedError, for_each_ordered};
use crate::path_codec::{
    PathRecord as Candidate, RootRequirement, io_error_message, resolve_search_root,
};
use crate::render_plan::{LineRenderGraph, LineRenderView, RenderPlanError, SharedLineRenderGraph};
use crate::search_text::{SearchText, SearchTextFailure};
use crate::skip_report::{SkipTally, detail_line};
use crate::traversal::{SkippedPaths, collect_search_candidates};
use grep_matcher::LineTerminator;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::SearcherBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::ops::ControlFlow;
use std::sync::Arc;
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
    /// File or directory to search; omit for the session working directory.
    #[schemars(description = crate::model_guidance::local_path_description(
        "File or directory to search. Omit for the session working directory."
    ))]
    pub path: Option<String>,
    /// Globs to filter files, e.g. ["**/*.rs", "!tests/**"]. A leading `!` excludes and always wins; negative-only lists include every other file.
    // Published as a plain string array; see the note on GlobRequest::pattern. (2026-08-17)
    #[schemars(with = "Option<Vec<String>>")]
    pub glob: Option<GlobPatterns>,
    /// File type filter, e.g. "js", "py", "rust" (equivalent to rg --type; more efficient than glob for standard types).
    #[serde(rename = "type")]
    #[schemars(rename = "type")]
    pub file_type: Option<String>,
    /// "content" = matching lines with optional context; "files_with_matches" (default) = matching paths only; "count" = per-file counts plus their total; "summary" = global totals from a full scan (ignores head_limit/offset).
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
    /// Single-file target only: decode that file with this WHATWG encoding label (e.g. "gbk"), same semantics as inspect_local_file's encoding. On a directory target use fallback_encoding instead.
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
    SearchFailure {
        reason: String,
        single_file_message: String,
    },
}

impl CandidateSkip {
    fn reason(&self) -> String {
        match self {
            Self::Encoding(rejection) => rejection.skip_reason(),
            Self::ChangedWhileSearched => "changed while being searched".to_string(),
            Self::SearchFailure { reason, .. } => reason.clone(),
        }
    }

    fn single_file_message(&self, candidate: &Candidate) -> String {
        match self {
            Self::Encoding(rejection) => rejection.message(candidate.display.as_ref()),
            Self::ChangedWhileSearched => format!(
                "File changed while it was being searched: {}. Retry the grep request.",
                candidate.display
            ),
            Self::SearchFailure {
                single_file_message,
                ..
            } => single_file_message.clone(),
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
    Candidate(CandidateSkip),
    Fatal(String),
}

fn failure_message(candidate: &Candidate, failure: SearchFailure) -> String {
    match failure {
        SearchFailure::CaptureOverflow => capture_limit_error(candidate),
        SearchFailure::Cancelled => "Request cancelled.".to_string(),
        SearchFailure::EpochRetired => {
            unreachable!("retired speculative work is never delivered to the reducer")
        }
        SearchFailure::Candidate(skip) => skip.single_file_message(candidate),
        SearchFailure::Fatal(message) => message,
    }
}

fn candidate_failure(candidate: &Candidate, message: String) -> SearchFailure {
    let prefixes = [
        format!("Cannot search file {}: ", candidate.display),
        format!(
            "Cannot create a stable search snapshot for {}: ",
            candidate.display
        ),
    ];
    let remainder = prefixes
        .iter()
        .find_map(|prefix| message.strip_prefix(prefix))
        .unwrap_or(&message);
    let reason = remainder
        .split_once(". ")
        .map_or(remainder, |(first, _)| first)
        .trim_end_matches('.')
        .to_string();
    SearchFailure::Candidate(CandidateSkip::SearchFailure {
        reason,
        single_file_message: message,
    })
}

fn capture_overflow_skip(candidate: &Candidate) -> CandidateSkip {
    CandidateSkip::SearchFailure {
        reason: "matching content and context exceed the 64 MiB safety limit".to_string(),
        single_file_message: capture_limit_error(candidate),
    }
}

struct PageFormat<'a> {
    pattern: &'a str,
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
    /// Leading entries contributed by traversal rather than by searching.
    unreachable_listed: usize,
    /// Unreachable paths the traversal detail cap counted but dropped.
    unreachable_unlisted: usize,
}

impl SkippedFiles {
    /// Seeds the report with paths the walk never entered. Traversal happens
    /// before any file is searched, so its entries lead the detail list and the
    /// split point stays a simple prefix length.
    fn from_traversal(skipped: &SkippedPaths) -> Self {
        let entries = skipped
            .listed()
            .map(|path| SkippedFile {
                path: path.display.to_string(),
                reason: path.reason.to_string(),
            })
            .collect::<Vec<_>>();
        Self {
            unreachable_listed: entries.len(),
            unreachable_unlisted: skipped.unlisted(),
            entries,
        }
    }

    fn record(&mut self, path: &str, skip: &CandidateSkip) {
        self.entries.push(SkippedFile {
            path: path.to_string(),
            reason: skip.reason(),
        });
    }

    fn tally(&self) -> SkipTally {
        SkipTally {
            files: self.entries.len().saturating_sub(self.unreachable_listed),
            unreachable: self
                .unreachable_listed
                .saturating_add(self.unreachable_unlisted),
            listed: self.entries.len(),
        }
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

    fn fact(&self) -> Option<String> {
        let encoding = self.encoding?;
        Some(format!(
            "{} decoded using fallback encoding {encoding}",
            counted(self.count, "file", "files")
        ))
    }
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
        operation.clone(),
        executor,
    );
    work.check_inline()?;
    Ok(response)
}

fn grep_files_with_budget_source_and_execution(
    request: GrepRequest,
    budget: usize,
    budget_variable: &str,
    capture_heap_limit_bytes: usize,
    operation: OperationCtx,
    executor: Arc<GrepGlobExecutor>,
) -> ToolResponse {
    let adapter = ErrorBudgetAdapter::new(budget, budget_variable);
    adapter.adapt(grep_files_with_budget_source_and_execution_unadapted(
        request,
        budget,
        budget_variable,
        capture_heap_limit_bytes,
        operation,
        executor,
    ))
}

fn grep_files_with_budget_source_and_execution_unadapted(
    request: GrepRequest,
    budget: usize,
    budget_variable: &str,
    capture_heap_limit_bytes: usize,
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
    let glob = match build_glob(request.glob.as_ref()) {
        Ok(glob) => glob,
        Err(message) => return ToolResponse::error(message),
    };
    let collected = match collect_search_candidates(
        &root,
        glob.as_ref(),
        request.file_type.as_deref(),
        Some(&operation),
        Some(&executor),
    ) {
        Ok(collected) => collected,
        Err(message) => return ToolResponse::error(message),
    };
    let traversal_skips = collected.skipped;
    let candidates = Arc::<[Candidate]>::from(collected.items);
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
        let mut skipped_files = SkippedFiles::from_traversal(&traversal_skips);
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
                Err(SearchFailure::Fatal(format!(
                    "Search worker panicked while processing {}.",
                    panic_candidates[index].display
                )))
            },
            |index, outcome, _| {
                let candidate = &candidates[index];
                let outcome = match outcome {
                    Ok(outcome) => outcome,
                    Err(SearchFailure::Candidate(skip)) if !single_file_target => {
                        skipped_files.record(candidate.display.as_ref(), &skip);
                        return ControlFlow::Continue(());
                    }
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
            pattern: &request.pattern,
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
    let mut skipped_files = SkippedFiles::from_traversal(&traversal_skips);
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
            Err(SearchFailure::Fatal(format!(
                "Search worker panicked while processing {}.",
                panic_candidates[index].display
            )))
        },
        |index, outcome, reducer| {
            let candidate = &candidates[index];
            let (outcome, exact_form) = match outcome {
                Ok(outcome) => (outcome, false),
                Err(SearchFailure::CaptureOverflow) => {
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
                        Err(SearchFailure::CaptureOverflow) if !single_file_target => {
                            let skip = capture_overflow_skip(candidate);
                            skipped_files.record(candidate.display.as_ref(), &skip);
                            return ControlFlow::Continue(());
                        }
                        Err(SearchFailure::Candidate(skip)) if !single_file_target => {
                            skipped_files.record(candidate.display.as_ref(), &skip);
                            return ControlFlow::Continue(());
                        }
                        Err(kind) => {
                            failure = Some(failure_message(candidate, kind));
                            return ControlFlow::Break(());
                        }
                    }
                }
                Err(SearchFailure::Candidate(skip)) if !single_file_target => {
                    skipped_files.record(candidate.display.as_ref(), &skip);
                    return ControlFlow::Continue(());
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
        pattern: &request.pattern,
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
        OutputMode::Content => format_content_mode(&results, &request, &page),
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

fn build_glob(patterns: Option<&GlobPatterns>) -> Result<Option<PathGlobFilter>, String> {
    let Some(patterns) = patterns else {
        return Ok(None);
    };
    PathGlobFilter::compile(patterns, true)
        .map(Some)
        .map_err(|error| {
            format!(
                "Invalid glob pattern: {error}. Use forms like \"*.rs\" or \"**/*.{{ts,tsx}}\"."
            )
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
            return Err(candidate_failure(
                candidate,
                rejection.message(candidate.display.as_ref()),
            ));
        }
        Err(CaptureFailure::Io(error)) => {
            return Err(candidate_failure(
                candidate,
                io_error_message(&candidate.native, &error),
            ));
        }
        Err(CaptureFailure::Snapshot(error)) => {
            return Err(candidate_failure(
                candidate,
                snapshot_error_message(candidate, &error),
            ));
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
        .then(|| validated.transcoding_fact())
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
            let range = snapshot.shared_range(start).map_err(|error| {
                candidate_failure(candidate, snapshot_error_message(candidate, &error))
            })?;
            Some(SearchText::from_snapshot(range))
        } else {
            let reader = validated.open_source_reader(source).map_err(|error| {
                candidate_failure(candidate, snapshot_error_message(candidate, &error))
            })?;
            Some(
                SearchText::capture(reader, operation).map_err(|failure| match failure {
                    SearchTextFailure::Io(error) => {
                        candidate_failure(candidate, snapshot_error_message(candidate, &error))
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
                let reader = backing.open_reader().map_err(|error| {
                    candidate_failure(candidate, snapshot_error_message(candidate, &error))
                })?;
                searcher.search_reader(matcher, reader, &mut sink)
            }
        }
    } else {
        match snapshot.memory_bytes() {
            Some(bytes) => {
                let Some(decoded) = validated.decode_for_search(bytes) else {
                    return Err(candidate_failure(
                        candidate,
                        validated
                            .malformed_rejection()
                            .message(candidate.display.as_ref()),
                    ));
                };
                searcher.search_slice(matcher, &decoded, &mut sink)
            }
            None => {
                let reader = validated.open_source_reader(source).map_err(|error| {
                    candidate_failure(candidate, snapshot_error_message(candidate, &error))
                })?;
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
            GrepSinkError::CountOverflow => candidate_failure(
                candidate,
                format!(
                    "Cannot search file {}: the occurrence count overflowed.",
                    candidate.display
                ),
            ),
            GrepSinkError::Search(message) => candidate_failure(
                candidate,
                format!("Cannot search file {}: {message}", candidate.display),
            ),
            GrepSinkError::Io(error) if error.kind() == io::ErrorKind::InvalidData => {
                candidate_failure(
                    candidate,
                    validated
                        .malformed_rejection()
                        .message(candidate.display.as_ref()),
                )
            }
            GrepSinkError::Io(error) => {
                let message = error.to_string().to_ascii_lowercase();
                candidate_failure(
                    candidate,
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
            EncodingPipelineFailure::Io(error) => {
                candidate_failure(candidate, snapshot_error_message(candidate, &error))
            }
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
    facts: Vec<Arc<str>>,
    details: Vec<Arc<str>>,
    tally: SkipTally,
}

impl GrepNoteUnits {
    fn new(page: &PageFormat<'_>) -> Self {
        let mut facts = page
            .transcoding_notes
            .iter()
            .map(|line| Arc::<str>::from(line.as_str()))
            .collect::<Vec<_>>();
        if let Some(fallback) = page.fallback_usage.fact() {
            facts.push(Arc::from(fallback));
        }
        let details = page
            .skipped_files
            .entries
            .iter()
            .map(|entry| Arc::<str>::from(detail_line(&entry.path, &entry.reason)))
            .collect();
        Self {
            facts,
            details,
            tally: page.skipped_files.tally(),
        }
    }

    fn head(&self, page: &PageFormat<'_>, metric: HeadMetric, shown_details: usize) -> String {
        self.render(
            HeadNote::new(format!("grep {:?}", page.pattern), metric),
            shown_details,
        )
    }

    fn render(&self, mut note: HeadNote, shown_details: usize) -> String {
        for fact in &self.facts {
            note = note.fact(fact.as_ref());
        }
        if let Some(fact) = self.tally.fact(shown_details) {
            note = note.fact(fact);
        }
        note.render()
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

fn render_line_grep(
    graph: &mut LineRenderGraph,
    maximum: usize,
    page: &PageFormat<'_>,
    notes: &GrepNoteUnits,
    mut metric: impl FnMut(usize) -> HeadMetric,
) -> ToolResponse {
    if maximum == 0 {
        return budget_too_small(page.budget, page.budget_variable);
    }
    let probe = |graph: &mut LineRenderGraph, shown: usize, metric: HeadMetric| {
        let head = notes.head(page, metric, 0);
        graph.probe_head(shown, &head, &[] as &[Arc<str>], page.work())
    };
    let selected = match probe(graph, maximum, metric(maximum)) {
        Ok(tokens) if tokens <= page.budget => Some(maximum),
        Ok(_) if maximum > 1 => match replay_compat_binary_probes(1, maximum - 1, None, |middle| {
            match probe(graph, middle, metric(middle))? {
                tokens if tokens <= page.budget => Ok(Some(middle)),
                _ => Ok(None),
            }
        }) {
            Ok(selected) => selected,
            Err(error) => return grep_render_failure(error),
        },
        Ok(_) => None,
        Err(error) => return grep_render_failure(error),
    };
    let Some(shown) = selected else {
        return budget_too_small(page.budget, page.budget_variable);
    };
    match finish_line_grep_head(graph, shown, page, notes, metric(shown)) {
        Ok(Some(text)) => ToolResponse::text(text),
        Ok(None) => budget_too_small(page.budget, page.budget_variable),
        Err(error) => grep_render_failure(error),
    }
}

fn finish_line_grep_head(
    graph: &mut LineRenderGraph,
    shown: usize,
    page: &PageFormat<'_>,
    notes: &GrepNoteUnits,
    metric: HeadMetric,
) -> Result<Option<String>, RenderPlanError> {
    let Some((shown_details, head, tokens)) = select_grep_details(
        notes.details.len(),
        |shown_details| {
            let head = notes.head(page, metric.clone(), shown_details);
            let tokens =
                graph.probe_head(shown, &head, &notes.details[..shown_details], page.work())?;
            Ok((head, tokens))
        },
        page.budget,
    )?
    else {
        return Ok(None);
    };
    let rendered = graph.finish_head(
        shown,
        &head,
        &notes.details[..shown_details],
        tokens,
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
    metric: HeadMetric,
) -> Result<Option<String>, RenderPlanError> {
    let Some((shown_details, head, tokens)) = select_grep_details(
        notes.details.len(),
        |shown_details| {
            let head = notes.head(page, metric.clone(), shown_details);
            let tokens =
                graph.probe_head(view, &head, &notes.details[..shown_details], page.work())?;
            Ok((head, tokens))
        },
        page.budget,
    )?
    else {
        return Ok(None);
    };
    let rendered = graph.finish_head(
        view,
        &head,
        &notes.details[..shown_details],
        tokens,
        page.budget,
        page.work(),
    )?;
    Ok(Some(rendered.text))
}

fn select_grep_details(
    maximum: usize,
    mut probe: impl FnMut(usize) -> Result<(String, usize), RenderPlanError>,
    budget: usize,
) -> Result<Option<(usize, String, usize)>, RenderPlanError> {
    let (full_head, full_tokens) = probe(maximum)?;
    if full_tokens <= budget {
        return Ok(Some((maximum, full_head, full_tokens)));
    }
    let (empty_head, empty_tokens) = probe(0)?;
    if empty_tokens > budget {
        return Ok(None);
    }
    if maximum <= 1 {
        return Ok(Some((0, empty_head, empty_tokens)));
    }
    replay_compat_binary_probes(
        1,
        maximum - 1,
        Some((0, empty_head, empty_tokens)),
        |middle| {
            let (head, tokens) = probe(middle)?;
            Ok((tokens <= budget).then_some((middle, head, tokens)))
        },
    )
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
    render_line_grep(&mut graph, initial, page, &notes, |shown| {
        paged_metric(page, shown, "files")
    })
}

fn format_count_mode(results: &[FileResult], page: &PageFormat<'_>) -> ToolResponse {
    let initial = if page.head_limit == 0 {
        results.len()
    } else {
        page.head_limit.min(results.len())
    };
    let mut lines = Vec::with_capacity(initial);
    for result in &results[..initial] {
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
    render_line_grep(&mut graph, initial, page, &notes, |shown| {
        paged_metric(page, shown, "files")
    })
}

fn format_content_mode(
    results: &[FileResult],
    request: &GrepRequest,
    page: &PageFormat<'_>,
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
        let metric = paged_metric(page, shown, "matches");
        render_content_page_with_degradation(
            results,
            &entries[..shown],
            request,
            page,
            &notes,
            &mut render_cache,
            metric,
        )
    };
    match fit_largest_content_output(initial, render_page) {
        Ok(Some(candidate)) => {
            match finish_content_grep_view(
                &mut render_cache.token_graph,
                &candidate.view,
                page,
                &notes,
                candidate.metric,
            ) {
                Ok(Some(text)) => ToolResponse::text(text),
                Ok(None) => budget_too_small(page.budget, page.budget_variable),
                Err(error) => grep_render_failure(error),
            }
        }
        Ok(None) => budget_too_small(page.budget, page.budget_variable),
        Err(ContentFormatError::Source(message)) => ToolResponse::error(message),
        Err(ContentFormatError::Render(error)) => grep_render_failure(error),
    }
}

struct ContentBodyCandidate {
    view: LineRenderView,
    metric: HeadMetric,
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
    metric: HeadMetric,
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
        let head = notes.head(page, metric.clone(), 0);
        let tokens =
            render_cache
                .token_graph
                .probe_head(&view, &head, &[] as &[Arc<str>], page.work())?;
        Ok((tokens <= page.budget).then_some(ContentBodyCandidate {
            view,
            metric: metric.clone(),
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
        }
    }

    fn literal(&mut self, value: &'static str) -> Arc<str> {
        if let Some(line) = self.literals.get(value) {
            return Arc::clone(line);
        }
        let line = Arc::<str>::from(value);
        self.literals.insert(value, Arc::clone(&line));
        line
    }

    fn header(&mut self, file_index: usize, result: &FileResult) -> Arc<str> {
        if let Some(line) = self.headers.get(&file_index) {
            return Arc::clone(line);
        }
        let line = Arc::<str>::from(result.path());
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
        self.only_matches.insert(key, Arc::clone(&line));
        Ok(line)
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

fn format_summary(occurrences: usize, files: usize, page: &PageFormat<'_>) -> ToolResponse {
    bodyless_grep_response(
        HeadNote::new(
            format!("grep {:?}", page.pattern),
            HeadMetric::count_in_files(occurrences, "occurrence", "occurrences", files),
        ),
        page,
    )
}

fn zero_result(mode: OutputMode, page: &PageFormat<'_>) -> ToolResponse {
    let metric = match mode {
        OutputMode::FilesWithMatches => HeadMetric::count(0, "file", "files"),
        OutputMode::Content | OutputMode::Count => HeadMetric::count(0, "match", "matches"),
        OutputMode::Summary => unreachable!("summary has its own zero-count response"),
    };
    bodyless_grep_response(
        HeadNote::new(format!("grep {:?}", page.pattern), metric),
        page,
    )
}

fn offset_exhausted(mode: OutputMode, page: &PageFormat<'_>) -> ToolResponse {
    let (singular, plural) = match mode {
        OutputMode::Content => ("match", "matches"),
        OutputMode::FilesWithMatches | OutputMode::Count => ("file", "files"),
        OutputMode::Summary => unreachable!("summary ignores offset"),
    };
    let extent = if page.scan_complete {
        format!(
            "{} exist",
            counted(page.total_entries_seen, singular, plural)
        )
    } else {
        format!(
            "at least {} exist",
            counted(page.total_entries_seen, singular, plural)
        )
    };
    bodyless_grep_response(
        HeadNote::new(
            format!("grep {:?}", page.pattern),
            HeadMetric::count(0, singular, plural),
        )
        .fact(extent),
        page,
    )
}

fn bodyless_grep_response(note: HeadNote, page: &PageFormat<'_>) -> ToolResponse {
    let mut graph = match LineRenderGraph::new(Vec::new(), page.work()) {
        Ok(graph) => graph,
        Err(error) => return grep_render_failure(error),
    };
    let notes = GrepNoteUnits::new(page);
    let selected = select_grep_details(
        notes.details.len(),
        |shown_details| {
            let head = notes.render(note.clone(), shown_details);
            let tokens =
                graph.probe_head(0, &head, &notes.details[..shown_details], page.work())?;
            Ok((head, tokens))
        },
        page.budget,
    );
    let Some((shown_details, head, tokens)) = (match selected {
        Ok(selected) => selected,
        Err(error) => return grep_render_failure(error),
    }) else {
        return budget_too_small(page.budget, page.budget_variable);
    };
    match graph.finish_head(
        0,
        &head,
        &notes.details[..shown_details],
        tokens,
        page.budget,
        page.work(),
    ) {
        Ok(rendered) => ToolResponse::text(rendered.text),
        Err(error) => grep_render_failure(error),
    }
}

fn paged_metric(page: &PageFormat<'_>, shown: usize, unit: &'static str) -> HeadMetric {
    let proven = page
        .total_entries_seen
        .max(page.offset.saturating_add(shown));
    let total = if page.scan_complete {
        CoverageTotal::Exact(proven)
    } else {
        CoverageTotal::AtLeast(proven.max(page.offset.saturating_add(shown).saturating_add(1)))
    };
    HeadMetric::Coverage {
        unit,
        ranges: vec![CoveredRange::new(
            page.offset.saturating_add(1),
            page.offset.saturating_add(shown),
        )],
        total,
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
            "{budget_variable}={budget} is too small to return the grep head note and one result. Increase it and retry."
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
