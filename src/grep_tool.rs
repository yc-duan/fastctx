//! grep tool backed by ripgrep engines, ignore traversal, deterministic paging, and content formatting.

use crate::budget::{GREP_TOKEN_BUDGET_ENV, assemble_text, estimate_tokens, tool_token_budget};
use crate::encoding::{
    ByteSource, EncodingDecision, EncodingRejection, canonical_encoding_label,
    validate_source_encoding,
};
use crate::model::ToolResponse;
use crate::parallel::for_each_ordered;
use crate::paths::{
    canonical_existing, display_path, io_error_message, missing_search_path_message,
    parse_input_path,
};
use crate::traversal::{ProjectCandidate as Candidate, collect_project_candidates};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use grep_matcher::{LineTerminator, Match, Matcher};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io;
use std::ops::ControlFlow;
use std::path::PathBuf;

const DEFAULT_HEAD_LIMIT: usize = 250;
const LONG_LINE_BYTES: usize = 500;
const MATCH_WINDOW_SIDE_CHARS: usize = 100;
const MAX_MATCH_CHARS: usize = 2_000;
const SEARCH_HEAP_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const CAPTURE_HEAP_LIMIT_BYTES: usize = 64 * 1024 * 1024;
// Files up to this size are read once and validated + searched in memory;
// larger files keep the bounded streaming path. 8 MiB covers virtually every
// repository text file while capping per-thread buffering.
const FAST_PATH_MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SEARCH_THREADS: usize = 16;

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
    /// Absolute path of the file or directory to search in.
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

#[derive(Clone, Copy, Debug)]
struct LineMatchSpan {
    line_number: usize,
    match_char_start: usize,
    match_char_len: usize,
}

#[derive(Clone, Debug)]
struct Occurrence {
    matched_text: String,
    start_line: usize,
    end_line: usize,
    line_spans: Vec<LineMatchSpan>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentEntry {
    MatchingLine(usize),
    Occurrence {
        start_line: usize,
        occurrence_index: usize,
    },
}

#[derive(Debug)]
struct FileResult {
    path: String,
    lines: BTreeMap<usize, String>,
    occurrences: BTreeMap<usize, Vec<Occurrence>>,
    entries: Vec<ContentEntry>,
    occurrence_total: usize,
    total_lines: usize,
    has_trailing_newline: bool,
}

struct SearchOutcome {
    result: Option<FileResult>,
    entries_seen: usize,
    encoding_rejection: Option<EncodingRejection>,
    transcoding_note: Option<String>,
    used_fallback: bool,
}

/// Why one candidate's search failed; the ordered reduce decides whether the
/// failure is even reachable before formatting it.
enum SearchFailure {
    /// Captured matches and context crossed the 64 MiB safety valve. Kept as a
    /// distinct variant so the paged reduce can retry with the exact live
    /// pagination window before giving up.
    CaptureOverflow,
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
        SearchFailure::Message(message) => message,
    }
}

fn search_thread_count(candidates: usize) -> usize {
    if candidates < 4 {
        return 1;
    }
    std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1)
        .min(MAX_SEARCH_THREADS)
}

struct PageFormat<'a> {
    offset: usize,
    head_limit: usize,
    budget: usize,
    budget_variable: &'a str,
    scan_complete: bool,
    total_entries_seen: usize,
    skipped_encodings: &'a SkippedEncodings,
    transcoding_notes: &'a BTreeSet<String>,
    fallback_usage: &'a FallbackUsage,
}

#[derive(Default)]
struct SkippedEncodings {
    entries: Vec<SkippedEncoding>,
}

impl SkippedEncodings {
    fn record(&mut self, path: &str, rejection: &EncodingRejection) {
        self.entries.push(SkippedEncoding {
            path: path.to_string(),
            reason: rejection.skip_reason(),
        });
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

struct SkippedEncoding {
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

#[derive(Clone, Copy)]
struct SearchOptions {
    multiline: bool,
    entry_mode: EntryMode,
    skip_entries: usize,
    max_selected_entries: Option<usize>,
    capture_content: bool,
    capture_match_text: bool,
    before_context: usize,
    after_context: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryMode {
    MatchingLine,
    Occurrence,
}

#[derive(Clone, Copy)]
struct SearchEncoding<'a> {
    explicit: Option<&'a str>,
    fallback: Option<&'a str>,
}

impl FileResult {
    fn occurrence_count(&self) -> usize {
        self.occurrence_total
    }
}

struct CollectSink<'a> {
    matcher: &'a RegexMatcher,
    occurrences: BTreeMap<usize, Vec<Occurrence>>,
    entries: Vec<ContentEntry>,
    lines: BTreeMap<usize, String>,
    occurrence_total: usize,
    entries_seen: usize,
    selected_entry_count: usize,
    last_matching_line: Option<usize>,
    current_line_selected: bool,
    entry_mode: EntryMode,
    skip_entries: usize,
    max_selected_entries: Option<usize>,
    capture_content: bool,
    capture_match_text: bool,
    before_context: usize,
    after_context: usize,
    recent_lines: VecDeque<(usize, String)>,
    recent_bytes: usize,
    stored_bytes: usize,
    after_context_until: usize,
    capture_overflow: bool,
}

impl<'a> CollectSink<'a> {
    fn new(matcher: &'a RegexMatcher, options: SearchOptions) -> Self {
        Self {
            matcher,
            occurrences: BTreeMap::new(),
            entries: Vec::new(),
            lines: BTreeMap::new(),
            occurrence_total: 0,
            entries_seen: 0,
            selected_entry_count: 0,
            last_matching_line: None,
            current_line_selected: false,
            entry_mode: options.entry_mode,
            skip_entries: options.skip_entries,
            max_selected_entries: options.max_selected_entries,
            capture_content: options.capture_content,
            capture_match_text: options.capture_match_text,
            before_context: options.before_context,
            after_context: options.after_context,
            recent_lines: VecDeque::with_capacity(options.before_context.min(1_024)),
            recent_bytes: 0,
            stored_bytes: 0,
            after_context_until: 0,
            capture_overflow: false,
        }
    }

    fn store_line(&mut self, line_number: usize, bytes: &[u8]) {
        if !self.capture_content || self.lines.contains_key(&line_number) {
            return;
        }
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        let line = String::from_utf8_lossy(bytes).into_owned();
        self.store_line_owned(line_number, line);
    }

    fn store_line_owned(&mut self, line_number: usize, line: String) {
        if self.lines.contains_key(&line_number) {
            return;
        }
        if !self.reserve(line.len()) {
            return;
        }
        self.stored_bytes = self.stored_bytes.saturating_add(line.len());
        self.lines.insert(line_number, line);
    }

    fn push_recent(&mut self, line_number: usize, bytes: &[u8]) {
        if !self.capture_content || self.before_context == 0 {
            return;
        }
        if self
            .recent_lines
            .back()
            .is_some_and(|(recent_line, _)| *recent_line == line_number)
        {
            return;
        }
        while self.recent_lines.len() >= self.before_context {
            if let Some((_, removed)) = self.recent_lines.pop_front() {
                self.recent_bytes = self.recent_bytes.saturating_sub(removed.len());
            }
        }
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        let line = String::from_utf8_lossy(bytes).into_owned();
        if !self.reserve(line.len()) {
            return;
        }
        self.recent_bytes = self.recent_bytes.saturating_add(line.len());
        self.recent_lines.push_back((line_number, line));
    }

    fn commit_recent(&mut self) {
        let recent = std::mem::take(&mut self.recent_lines);
        self.recent_bytes = 0;
        for (line_number, line) in recent {
            self.store_line_owned(line_number, line);
        }
    }

    fn reserve(&mut self, additional: usize) -> bool {
        if self
            .stored_bytes
            .saturating_add(self.recent_bytes)
            .saturating_add(additional)
            > CAPTURE_HEAP_LIMIT_BYTES
        {
            self.capture_overflow = true;
            false
        } else {
            true
        }
    }

    fn select_line(&mut self, line_number: usize) -> bool {
        if self.last_matching_line == Some(line_number) {
            return self.current_line_selected;
        }
        self.last_matching_line = Some(line_number);
        self.entries_seen = self.entries_seen.saturating_add(1);
        self.current_line_selected = self.entries_seen > self.skip_entries
            && self
                .max_selected_entries
                .is_none_or(|limit| self.selected_entry_count < limit);
        if self.current_line_selected {
            self.selected_entry_count = self.selected_entry_count.saturating_add(1);
            if self.capture_content {
                self.entries.push(ContentEntry::MatchingLine(line_number));
                self.commit_recent();
                self.after_context_until = self
                    .after_context_until
                    .max(line_number.saturating_add(self.after_context));
            }
        }
        self.current_line_selected
    }

    fn select_occurrence(&mut self, end_line: usize) -> bool {
        self.entries_seen = self.entries_seen.saturating_add(1);
        let selected = self.entries_seen > self.skip_entries
            && self
                .max_selected_entries
                .is_none_or(|limit| self.selected_entry_count < limit);
        if selected {
            self.selected_entry_count = self.selected_entry_count.saturating_add(1);
            if self.capture_content {
                self.commit_recent();
                self.after_context_until = self
                    .after_context_until
                    .max(end_line.saturating_add(self.after_context));
            }
        }
        selected
    }

    fn store_occurrence(
        &mut self,
        start_line: usize,
        end_line: usize,
        line_spans: Vec<LineMatchSpan>,
        matched_text: &[u8],
    ) {
        let metadata_bytes = std::mem::size_of::<Occurrence>().saturating_add(
            line_spans
                .len()
                .saturating_mul(std::mem::size_of::<LineMatchSpan>()),
        );
        let text_bytes = if self.capture_match_text {
            matched_text.len()
        } else {
            0
        };
        if !self.reserve(metadata_bytes.saturating_add(text_bytes)) {
            return;
        }
        let matched_text = if self.capture_match_text {
            String::from_utf8_lossy(matched_text).into_owned()
        } else {
            String::new()
        };
        self.stored_bytes = self
            .stored_bytes
            .saturating_add(metadata_bytes)
            .saturating_add(matched_text.len());
        let occurrences = self.occurrences.entry(start_line).or_default();
        let occurrence_index = occurrences.len();
        occurrences.push(Occurrence {
            matched_text,
            start_line,
            end_line,
            line_spans,
        });
        if self.entry_mode == EntryMode::Occurrence {
            self.entries.push(ContentEntry::Occurrence {
                start_line,
                occurrence_index,
            });
        }
    }

    fn limit_reached(&self) -> bool {
        self.max_selected_entries
            .is_some_and(|limit| self.selected_entry_count >= limit)
    }
}

impl Sink for CollectSink<'_> {
    type Error = io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        matched: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        let raw_bytes = matched.bytes();
        let raw_line_number = matched.line_number().unwrap_or(1) as usize;
        let mut selected_lines = BTreeSet::new();
        let matcher = self.matcher;
        matcher
            .find_iter(raw_bytes, |found: Match| {
                // 2026-07-15: CRLF multiline matching exposes a synthetic empty match after the terminator, outside the logical line set.
                if found.start() == found.end()
                    && found.end() == raw_bytes.len()
                    && raw_bytes.ends_with(b"\n")
                {
                    return true;
                }
                let (start_line, end_line, line_spans) =
                    occurrence_metadata(raw_bytes, raw_line_number, found);
                let is_selected = match self.entry_mode {
                    EntryMode::MatchingLine => self.select_line(start_line),
                    EntryMode::Occurrence => self.select_occurrence(end_line),
                };
                self.occurrence_total = self.occurrence_total.saturating_add(1);
                if is_selected && self.capture_content {
                    selected_lines.extend(start_line..=end_line);
                    self.store_occurrence(
                        start_line,
                        end_line,
                        line_spans,
                        &raw_bytes[found.start()..found.end()],
                    );
                }
                !self.capture_overflow
                    && (self.entry_mode == EntryMode::MatchingLine || !self.limit_reached())
            })
            .map_err(io::Error::from)?;
        for (line_delta, line) in matched.lines().enumerate() {
            let line_number = raw_line_number + line_delta;
            if selected_lines.contains(&line_number) || line_number <= self.after_context_until {
                self.store_line(line_number, line);
            }
            self.push_recent(line_number, line);
        }
        Ok(!self.capture_overflow && !self.limit_reached())
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        if let Some(line_number) = context.line_number() {
            let line_number = line_number as usize;
            if line_number <= self.after_context_until {
                self.store_line(line_number, context.bytes());
            }
            if matches!(
                context.kind(),
                SinkContextKind::Before | SinkContextKind::After
            ) {
                self.push_recent(line_number, context.bytes());
            }
        }
        Ok(!self.capture_overflow)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        self.recent_lines.clear();
        self.recent_bytes = 0;
        self.after_context_until = 0;
        Ok(!self.capture_overflow)
    }
}

fn occurrence_metadata(
    bytes: &[u8],
    first_line_number: usize,
    found: Match,
) -> (usize, usize, Vec<LineMatchSpan>) {
    let mut logical_lines = Vec::new();
    let mut line_start = 0_usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let content_end = if index > line_start && bytes[index - 1] == b'\r' {
            index - 1
        } else {
            index
        };
        logical_lines.push((line_start, content_end, index + 1));
        line_start = index + 1;
    }
    if line_start < bytes.len() || logical_lines.is_empty() {
        logical_lines.push((line_start, bytes.len(), bytes.len()));
    }

    let line_index_at = |offset: usize| {
        logical_lines
            .iter()
            .enumerate()
            .find_map(|(index, (_, _, full_end))| {
                (offset < *full_end || (index + 1 == logical_lines.len() && offset <= *full_end))
                    .then_some(index)
            })
            .unwrap_or_else(|| logical_lines.len().saturating_sub(1))
    };
    let start_index = line_index_at(found.start());
    let end_offset = if found.end() > found.start() {
        found.end() - 1
    } else {
        found.start()
    };
    let end_index = line_index_at(end_offset);
    let start_line = first_line_number + start_index;
    let end_line = first_line_number + end_index;
    let mut line_spans = Vec::with_capacity(end_index.saturating_sub(start_index) + 1);
    for (line_index, (content_start, content_end, _)) in logical_lines
        .iter()
        .enumerate()
        .take(end_index + 1)
        .skip(start_index)
    {
        let overlap_start = found.start().max(*content_start).min(*content_end);
        let overlap_end = found.end().min(*content_end).max(overlap_start);
        let anchor = if overlap_start < overlap_end {
            overlap_start
        } else if found.start() <= *content_start {
            *content_start
        } else {
            *content_end
        };
        let match_char_start = String::from_utf8_lossy(&bytes[*content_start..anchor])
            .chars()
            .count();
        let match_char_len = if overlap_start < overlap_end {
            String::from_utf8_lossy(&bytes[overlap_start..overlap_end])
                .chars()
                .count()
        } else {
            0
        };
        line_spans.push(LineMatchSpan {
            line_number: first_line_number + line_index,
            match_char_start,
            match_char_len,
        });
    }
    (start_line, end_line, line_spans)
}

/// Executes a grep query, returning traversal and matching failures as explicit actionable text errors.
pub fn grep_files(request: GrepRequest) -> ToolResponse {
    let budget = match tool_token_budget(GREP_TOKEN_BUDGET_ENV) {
        Ok(budget) => budget,
        Err(message) => return ToolResponse::error(message),
    };
    grep_files_with_budget_source(request, budget.value, budget.variable)
}

#[cfg(test)]
fn grep_files_with_budget(request: GrepRequest, budget: usize) -> ToolResponse {
    grep_files_with_budget_source(request, budget, "FASTCTX_TOKEN_BUDGET")
}

fn grep_files_with_budget_source(
    request: GrepRequest,
    budget: usize,
    budget_variable: &str,
) -> ToolResponse {
    let root_input = request.path.clone();
    let root = match resolve_root(root_input.as_deref()) {
        Ok(root) => root,
        Err(response) => return response,
    };
    let single_file_target = fs::metadata(&root).is_ok_and(|metadata| metadata.is_file());
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
        return ToolResponse::error(rejection.message(&display_path(&root)));
    }
    let fallback_encoding_label = match request.fallback_encoding.as_deref() {
        Some(encoding) => match canonical_encoding_label(encoding) {
            Ok(label) => Some(label),
            Err(rejection) => return ToolResponse::error(rejection.message(&display_path(&root))),
        },
        None => None,
    };
    let search_encoding = SearchEncoding {
        explicit: request.encoding.as_deref(),
        fallback: request.fallback_encoding.as_deref(),
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
        Ok(matcher) => matcher,
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
    let candidates =
        match collect_project_candidates(&root, glob.as_ref(), request.file_type.as_deref()) {
            Ok(candidates) => candidates,
            Err(message) => return ToolResponse::error(message),
        };
    let offset = request.offset.unwrap_or(0);
    let head_limit = request.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
    let mode = request.output_mode.unwrap_or_default();
    let only_matching = request.only_matching.unwrap_or(false);
    let content_entry_mode = if only_matching || multiline {
        EntryMode::Occurrence
    } else {
        EntryMode::MatchingLine
    };
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
        let mut skipped_encodings = SkippedEncodings::default();
        let mut transcoding_notes = BTreeSet::new();
        let mut fallback_usage = FallbackUsage::default();
        let options = SearchOptions {
            multiline,
            entry_mode: EntryMode::MatchingLine,
            skip_entries: 0,
            max_selected_entries: None,
            capture_content: false,
            capture_match_text: false,
            before_context: 0,
            after_context: 0,
        };
        let mut failure: Option<ToolResponse> = None;
        for_each_ordered(
            &candidates,
            search_thread_count(candidates.len()),
            |candidate| search_candidate(candidate, &matcher, options, search_encoding),
            |index, outcome| {
                let candidate = &candidates[index];
                let outcome = match outcome {
                    Ok(outcome) => outcome,
                    Err(kind) => {
                        failure = Some(ToolResponse::error(failure_message(candidate, kind)));
                        return ControlFlow::Break(());
                    }
                };
                if let Some(rejection) = outcome.encoding_rejection {
                    if single_file_target {
                        failure = Some(ToolResponse::error(rejection.message(&candidate.display)));
                        return ControlFlow::Break(());
                    }
                    skipped_encodings.record(&candidate.display, &rejection);
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
        if let Some(response) = failure {
            return response;
        }
        return format_summary(
            occurrence_total,
            file_total,
            &skipped_encodings,
            &transcoding_notes,
            &fallback_usage,
            budget,
            budget_variable,
        );
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
    let mut skipped_encodings = SkippedEncodings::default();
    let mut transcoding_notes = BTreeSet::new();
    let mut fallback_usage = FallbackUsage::default();
    // Every candidate is searched with identical options so files can run in
    // parallel. Content mode over-captures (no skip, worst-case cap of
    // offset + probe); the ordered reduce below trims each file back to
    // exactly the entries the sequential pagination would have selected, so
    // the observable output stays byte-identical to a serial scan.
    let worker_options = SearchOptions {
        multiline,
        entry_mode: if mode == OutputMode::Content {
            content_entry_mode
        } else {
            EntryMode::MatchingLine
        },
        skip_entries: 0,
        max_selected_entries: match mode {
            OutputMode::FilesWithMatches => Some(1),
            OutputMode::Count => None,
            OutputMode::Content => Some(offset.saturating_add(probe_entry_limit)),
            OutputMode::Summary => unreachable!("summary is handled before paging"),
        },
        capture_content: mode == OutputMode::Content,
        capture_match_text: mode == OutputMode::Content && only_matching,
        before_context,
        after_context,
    };
    let mut failure: Option<String> = None;
    for_each_ordered(
        &candidates,
        search_thread_count(candidates.len()),
        |candidate| search_candidate(candidate, &matcher, worker_options, search_encoding),
        |index, outcome| {
            let candidate = &candidates[index];
            let (outcome, exact_form) = match outcome {
                Ok(outcome) => (outcome, false),
                Err(SearchFailure::CaptureOverflow) => {
                    // The over-capture cap can cross the 64 MiB capture valve
                    // where the live window would not; retry with the exact
                    // sequential options before surfacing the error.
                    let exact = SearchOptions {
                        skip_entries: if mode == OutputMode::Content {
                            skip_remaining
                        } else {
                            0
                        },
                        max_selected_entries: match mode {
                            OutputMode::FilesWithMatches => Some(1),
                            OutputMode::Count => None,
                            OutputMode::Content => {
                                Some(probe_entry_limit.saturating_sub(collected_entries))
                            }
                            OutputMode::Summary => {
                                unreachable!("summary is handled before paging")
                            }
                        },
                        ..worker_options
                    };
                    match search_candidate(candidate, &matcher, exact, search_encoding) {
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
            if let Some(rejection) = outcome.encoding_rejection {
                if single_file_target {
                    failure = Some(rejection.message(&candidate.display));
                    return ControlFlow::Break(());
                }
                skipped_encodings.record(&candidate.display, &rejection);
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
                        collected_entries = collected_entries.saturating_add(result.entries.len());
                        results.push(result);
                    }
                } else if let Some(mut result) = outcome.result {
                    // Under the over-capture options every seen entry was
                    // selected, so entries[..start] is this file's share of
                    // the remaining offset and entries[start..end] is what a
                    // sequential scan would have delivered; `end` is also the
                    // number of entries that scan would have seen here.
                    let available = result.entries.len();
                    let need = probe_entry_limit.saturating_sub(collected_entries);
                    let start = skip_remaining.min(available);
                    let end = available.min(start.saturating_add(need));
                    total_entries_seen = total_entries_seen.saturating_add(end);
                    skip_remaining = skip_remaining.saturating_sub(start);
                    if end > start {
                        result.entries.drain(..start);
                        result.entries.truncate(end - start);
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
        skipped_encodings: &skipped_encodings,
        transcoding_notes: &transcoding_notes,
        fallback_usage: &fallback_usage,
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

fn resolve_root(input: Option<&str>) -> Result<PathBuf, ToolResponse> {
    let path = match input {
        Some(input) => {
            let parsed = parse_input_path(input);
            if !parsed.is_absolute() || !parsed.exists() {
                return Err(ToolResponse::error(missing_search_path_message(input)));
            }
            parsed
        }
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    match fs::metadata(&path) {
        Ok(_) => Ok(canonical_existing(&path).unwrap_or(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(ToolResponse::error(
            missing_search_path_message(input.unwrap_or(".")),
        )),
        Err(error) => Err(ToolResponse::error(io_error_message(&path, &error))),
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
    options: SearchOptions,
    encoding: SearchEncoding<'_>,
) -> Result<SearchOutcome, SearchFailure> {
    // Small files are read once so validation and search run over the same
    // snapshot instead of re-opening the file for every validation pass.
    let snapshot = if candidate.file_len <= FAST_PATH_MAX_BYTES {
        Some(fs::read(&candidate.path).map_err(|error| io_error_message(&candidate.path, &error))?)
    } else {
        None
    };
    let source = match snapshot.as_deref() {
        Some(bytes) => ByteSource::Bytes(bytes),
        None => ByteSource::File(&candidate.path),
    };
    let initial = validate_source_encoding(source, encoding.explicit)
        .map_err(|error| io_error_message(&candidate.path, &error))?;
    let (validated, used_fallback) = match initial {
        EncodingDecision::Text(validated) => (validated, false),
        EncodingDecision::Binary => {
            return Ok(SearchOutcome {
                result: None,
                entries_seen: 0,
                encoding_rejection: None,
                transcoding_note: None,
                used_fallback: false,
            });
        }
        EncodingDecision::Rejected(rejection) => match encoding.fallback {
            Some(fallback)
                if encoding.explicit.is_none()
                    && !matches!(rejection, EncodingRejection::BomMismatch { .. }) =>
            {
                match validate_source_encoding(source, Some(fallback))
                    .map_err(|error| io_error_message(&candidate.path, &error))?
                {
                    EncodingDecision::Text(validated) => (validated, true),
                    EncodingDecision::Binary | EncodingDecision::Rejected(_) => {
                        return Ok(SearchOutcome {
                            result: None,
                            entries_seen: 0,
                            encoding_rejection: Some(rejection),
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
                    encoding_rejection: Some(rejection),
                    transcoding_note: None,
                    used_fallback: false,
                });
            }
        },
    };
    let transcoding_note = (!used_fallback)
        .then(|| validated.transcoding_note())
        .flatten();
    let mut searcher = SearcherBuilder::new();
    searcher
        .line_number(true)
        .line_terminator(LineTerminator::crlf())
        .multi_line(options.multiline)
        .before_context(options.before_context)
        .after_context(options.after_context)
        .heap_limit(Some(SEARCH_HEAP_LIMIT_BYTES));
    let mut searcher = searcher.build();
    let mut sink = CollectSink::new(matcher, options);
    let search_result = match snapshot.as_deref() {
        Some(bytes) => {
            let Some(decoded) = validated.decode_for_search(bytes) else {
                return Err(validated
                    .malformed_rejection()
                    .message(&candidate.display)
                    .into());
            };
            searcher.search_slice(matcher, &decoded, &mut sink)
        }
        None => {
            let reader = validated
                .open_reader(&candidate.path)
                .map_err(|error| io_error_message(&candidate.path, &error))?;
            searcher.search_reader(matcher, reader, &mut sink)
        }
    };
    if let Err(error) = search_result {
        if error.kind() == io::ErrorKind::InvalidData {
            return Err(validated
                .malformed_rejection()
                .message(&candidate.display)
                .into());
        }
        return Err(search_error_message(candidate, &error).into());
    }
    if sink.capture_overflow {
        return Err(SearchFailure::CaptureOverflow);
    }
    let entries_seen = sink.entries_seen;
    let has_selected_result = if options.capture_content {
        !sink.entries.is_empty()
    } else {
        sink.occurrence_total > 0
    };
    let result = has_selected_result.then(|| FileResult {
        path: candidate.display.clone(),
        lines: sink.lines,
        occurrences: sink.occurrences,
        entries: sink.entries,
        occurrence_total: sink.occurrence_total,
        total_lines: validated.total_lines,
        has_trailing_newline: validated.has_trailing_newline,
    });
    Ok(SearchOutcome {
        result,
        entries_seen,
        encoding_rejection: None,
        transcoding_note,
        used_fallback,
    })
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

fn format_files_mode(results: &[FileResult], page: &PageFormat<'_>) -> ToolResponse {
    let available = results.iter().collect::<Vec<_>>();
    let initial = if page.head_limit == 0 {
        available.len()
    } else {
        page.head_limit.min(available.len())
    };
    let output = fit_largest_output(initial, page.budget, |shown| {
        let selected = &available[..shown];
        let lines = selected
            .iter()
            .map(|result| result.path.clone())
            .collect::<Vec<_>>();
        let has_more = shown < available.len() || !page.scan_complete;
        let terminal = paged_terminal(
            "file",
            "files",
            page.offset,
            shown,
            has_more,
            page.total_entries_seen,
        );
        render_grep_output(
            &lines,
            page.transcoding_notes,
            page.skipped_encodings,
            page.fallback_usage,
            &terminal,
            page.budget,
        )
    });
    match output {
        Some(output) => ToolResponse::text(output),
        None => budget_too_small(page.budget, page.budget_variable),
    }
}

fn format_count_mode(results: &[FileResult], page: &PageFormat<'_>) -> ToolResponse {
    let available = results.iter().collect::<Vec<_>>();
    let initial = if page.head_limit == 0 {
        available.len()
    } else {
        page.head_limit.min(available.len())
    };
    let output = fit_largest_output(initial, page.budget, |shown| {
        let selected = &available[..shown];
        let total = selected
            .iter()
            .map(|result| result.occurrence_count())
            .sum::<usize>();
        let lines = selected
            .iter()
            .map(|result| format!("{}:{}", result.path, result.occurrence_count()))
            .collect::<Vec<_>>();
        let has_more = shown < available.len() || !page.scan_complete;
        let terminal = count_terminal(page.offset, shown, total, has_more, page.total_entries_seen);
        render_grep_output(
            &lines,
            page.transcoding_notes,
            page.skipped_encodings,
            page.fallback_usage,
            &terminal,
            page.budget,
        )
    });
    match output {
        Some(output) => ToolResponse::text(output),
        None => budget_too_small(page.budget, page.budget_variable),
    }
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
        render_content_page_with_degradation(results, &entries[..shown], request, page, &terminal)
    };
    let output = fit_largest_content_output(initial, render_page);
    match output {
        Some(output) => ToolResponse::text(output),
        None => budget_too_small(page.budget, page.budget_variable),
    }
}

fn fit_largest_content_output(
    maximum: usize,
    mut render: impl FnMut(usize) -> Option<String>,
) -> Option<String> {
    if maximum == 0 {
        return None;
    }
    if let Some(output) = render(maximum) {
        return Some(output);
    }
    let mut low = 1_usize;
    let mut high = maximum - 1;
    let mut best = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        if let Some(output) = render(middle) {
            best = Some(output);
            low = middle.saturating_add(1);
        } else {
            high = middle.saturating_sub(1);
        }
    }
    best
}

fn render_content_page_with_degradation(
    results: &[FileResult],
    selected: &[(usize, ContentEntry)],
    request: &GrepRequest,
    page: &PageFormat<'_>,
    terminal: &str,
) -> Option<String> {
    let (requested_before, requested_after) = requested_context(request);
    let maximum_context = requested_before.max(requested_after);
    let render = |context_depth: usize, match_window: usize| {
        let lines = render_content_lines(results, selected, request, context_depth, match_window);
        render_grep_output(
            &lines,
            page.transcoding_notes,
            page.skipped_encodings,
            page.fallback_usage,
            terminal,
            page.budget,
        )
    };

    let full = render(maximum_context, MAX_MATCH_CHARS);
    if estimate_tokens(&full) <= page.budget {
        return Some(full);
    }

    let no_context = render(0, MAX_MATCH_CHARS);
    if estimate_tokens(&no_context) <= page.budget {
        let mut low = 0_usize;
        let mut high = maximum_context;
        let mut best = no_context;
        while low <= high {
            let middle = low + (high - low) / 2;
            let output = render(middle, MAX_MATCH_CHARS);
            if estimate_tokens(&output) <= page.budget {
                best = output;
                low = middle.saturating_add(1);
            } else {
                high = middle.saturating_sub(1);
            }
        }
        return Some(best);
    }

    let mut low = 1_usize;
    let mut high = MAX_MATCH_CHARS - 1;
    let mut best = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        let output = render(0, middle);
        if estimate_tokens(&output) <= page.budget {
            best = Some(output);
            low = middle.saturating_add(1);
        } else {
            high = middle.saturating_sub(1);
        }
    }
    best
}

fn render_content_lines(
    results: &[FileResult],
    selected: &[(usize, ContentEntry)],
    request: &GrepRequest,
    context_depth: usize,
    match_window: usize,
) -> Vec<String> {
    let single_file_target = request
        .path
        .as_deref()
        .map(parse_input_path)
        .and_then(|path| fs::metadata(path).ok())
        .is_some_and(|metadata| metadata.is_file());
    let line_numbers = request.line_numbers.unwrap_or(true);
    let only_matching = request.only_matching.unwrap_or(false);
    let (requested_before, requested_after) = requested_context(request);
    let before = requested_before.min(context_depth);
    let after = requested_after.min(context_depth);

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
                lines.push(String::new());
            }
            lines.push(result.path.clone());
        }
        let mut rendered = if only_matching {
            render_only_matching_group(result, &entries, before, after, line_numbers, match_window)
        } else {
            render_matching_line_group(result, &entries, before, after, line_numbers, match_window)
        };
        lines.append(&mut rendered);
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

fn render_only_matching_group(
    result: &FileResult,
    entries: &[ContentEntry],
    before: usize,
    after: usize,
    line_numbers: bool,
    match_window: usize,
) -> Vec<String> {
    let mut occurrence_starts = BTreeMap::<usize, Vec<(usize, usize)>>::new();
    let mut match_lines = BTreeSet::new();
    let mut ranges = Vec::new();
    for entry in entries {
        for (start_line, occurrence_index) in occurrence_keys(result, *entry) {
            let occurrence = &result.occurrences[&start_line][occurrence_index];
            occurrence_starts
                .entry(occurrence.start_line)
                .or_default()
                .push((start_line, occurrence_index));
            match_lines.extend(occurrence.start_line..=occurrence.end_line);
            ranges.push(context_range(
                occurrence.start_line,
                occurrence.end_line,
                before,
                after,
                result.total_lines,
            ));
        }
    }
    let ranges = merge_ranges(ranges);
    let mut lines = Vec::new();
    for (block_index, (start, end)) in ranges.into_iter().enumerate() {
        if block_index > 0 {
            lines.push("--".to_string());
        }
        for line_number in start..=end {
            if let Some(keys) = occurrence_starts.get(&line_number) {
                for (start_line, occurrence_index) in keys {
                    let occurrence = &result.occurrences[start_line][*occurrence_index];
                    lines.push(format_only_match(
                        match_prefix(line_number, line_numbers),
                        &occurrence.matched_text,
                        match_window,
                    ));
                }
                continue;
            }
            if match_lines.contains(&line_number) {
                continue;
            }
            if let Some(line) = result_line(result, line_number) {
                lines.push(format_context_line(
                    context_prefix(line_number, line_numbers),
                    line,
                ));
            }
        }
    }
    lines
}

fn render_matching_line_group(
    result: &FileResult,
    entries: &[ContentEntry],
    before: usize,
    after: usize,
    line_numbers: bool,
    match_window: usize,
) -> Vec<String> {
    let mut match_lines = BTreeSet::new();
    let mut spans = BTreeMap::<usize, Vec<LineMatchSpan>>::new();
    let mut ranges = Vec::new();
    for entry in entries {
        match *entry {
            ContentEntry::MatchingLine(line_number) => {
                match_lines.insert(line_number);
                if let Some(occurrences) = result.occurrences.get(&line_number) {
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
                    result.total_lines,
                ));
            }
            ContentEntry::Occurrence {
                start_line,
                occurrence_index,
            } => {
                let occurrence = &result.occurrences[&start_line][occurrence_index];
                match_lines.extend(occurrence.start_line..=occurrence.end_line);
                for span in &occurrence.line_spans {
                    spans.entry(span.line_number).or_default().push(*span);
                }
                ranges.push(context_range(
                    occurrence.start_line,
                    occurrence.end_line,
                    before,
                    after,
                    result.total_lines,
                ));
            }
        }
    }
    let ranges = merge_ranges(ranges);
    let mut lines = Vec::new();
    for (block_index, (start, end)) in ranges.into_iter().enumerate() {
        if block_index > 0 {
            lines.push("--".to_string());
        }
        for line_number in start..=end {
            let Some(line) = result_line(result, line_number) else {
                continue;
            };
            if match_lines.contains(&line_number) {
                lines.push(format_match_line(
                    match_prefix(line_number, line_numbers),
                    line,
                    spans.get(&line_number).map(Vec::as_slice).unwrap_or(&[]),
                    match_window,
                ));
            } else {
                lines.push(format_context_line(
                    context_prefix(line_number, line_numbers),
                    line,
                ));
            }
        }
    }
    lines
}

fn occurrence_keys(result: &FileResult, entry: ContentEntry) -> Vec<(usize, usize)> {
    match entry {
        ContentEntry::MatchingLine(line_number) => result
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

fn result_line(result: &FileResult, line_number: usize) -> Option<&str> {
    result
        .lines
        .get(&line_number)
        .map(String::as_str)
        .or_else(|| {
            (line_number == result.total_lines && result.has_trailing_newline).then_some("")
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
    line: &str,
    match_spans: &[LineMatchSpan],
    match_window: usize,
) -> String {
    if line.len() <= LONG_LINE_BYTES {
        return format!("{prefix}{line}");
    }
    let chars = line.chars().collect::<Vec<_>>();
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
    output
}

fn format_context_line(prefix: String, line: &str) -> String {
    if line.len() <= LONG_LINE_BYTES {
        format!("{prefix}{line}")
    } else {
        format!(
            "{prefix}[long line omitted: {} chars]",
            line.chars().count()
        )
    }
}

fn render_grep_output(
    lines: &[String],
    transcoding_notes: &BTreeSet<String>,
    skipped_encodings: &SkippedEncodings,
    fallback_usage: &FallbackUsage,
    terminal: &str,
    budget: usize,
) -> String {
    let total_skipped = skipped_encodings.len();
    let full = assemble_grep_output(
        lines,
        transcoding_notes,
        skipped_encodings,
        fallback_usage,
        terminal,
        total_skipped,
    );
    if estimate_tokens(&full) <= budget || total_skipped == 0 {
        return full;
    }
    let mut low = 0_usize;
    let mut high = total_skipped - 1;
    let mut best = assemble_grep_output(
        lines,
        transcoding_notes,
        skipped_encodings,
        fallback_usage,
        terminal,
        0,
    );
    while low <= high {
        let middle = low + (high - low) / 2;
        let output = assemble_grep_output(
            lines,
            transcoding_notes,
            skipped_encodings,
            fallback_usage,
            terminal,
            middle,
        );
        if estimate_tokens(&output) <= budget {
            best = output;
            low = middle.saturating_add(1);
        } else {
            high = middle.saturating_sub(1);
        }
    }
    best
}

fn assemble_grep_output(
    lines: &[String],
    transcoding_notes: &BTreeSet<String>,
    skipped_encodings: &SkippedEncodings,
    fallback_usage: &FallbackUsage,
    terminal: &str,
    shown_skips: usize,
) -> String {
    let mut notes = transcoding_notes.iter().cloned().collect::<Vec<_>>();
    notes.extend(
        skipped_encodings
            .entries
            .iter()
            .take(shown_skips)
            .map(|entry| format!("{} — {}", entry.path, entry.reason)),
    );
    if let Some(note) = fallback_usage.note() {
        notes.push(note);
    }
    notes.push(terminal_with_skips(
        terminal,
        skipped_encodings.len(),
        shown_skips,
    ));
    assemble_text(lines, &notes)
}

fn terminal_with_skips(terminal: &str, skipped: usize, shown: usize) -> String {
    if skipped == 0 {
        return terminal.to_string();
    }
    let stem = terminal
        .strip_suffix(".)")
        .expect("grep terminal notes always end with .)");
    if shown == skipped {
        format!("{stem}; {} skipped.)", counted(skipped, "file", "files"))
    } else {
        format!(
            "{stem}; {} skipped, showing {shown} — narrow path/glob to inspect the rest.)",
            counted(skipped, "file", "files")
        )
    }
}

fn format_summary(
    occurrences: usize,
    files: usize,
    skipped_encodings: &SkippedEncodings,
    transcoding_notes: &BTreeSet<String>,
    fallback_usage: &FallbackUsage,
    budget: usize,
    budget_variable: &str,
) -> ToolResponse {
    let terminal = format!(
        "(Complete: {} across {}.)",
        counted(occurrences, "occurrence", "occurrences"),
        counted(files, "file", "files")
    );
    let output = render_grep_output(
        &[],
        transcoding_notes,
        skipped_encodings,
        fallback_usage,
        &terminal,
        budget,
    );
    if estimate_tokens(&output) <= budget {
        ToolResponse::text(output)
    } else {
        budget_too_small(budget, budget_variable)
    }
}

fn zero_result(mode: OutputMode, page: &PageFormat<'_>) -> ToolResponse {
    let terminal = match mode {
        OutputMode::FilesWithMatches => "(Complete: no files matched.)",
        OutputMode::Content | OutputMode::Count => "(Complete: no matches found.)",
        OutputMode::Summary => unreachable!("summary has its own zero-count response"),
    };
    terminal_only_response(
        terminal.to_string(),
        page.skipped_encodings,
        page.transcoding_notes,
        page.fallback_usage,
        page.budget,
        page.budget_variable,
    )
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
        page.skipped_encodings,
        page.transcoding_notes,
        page.fallback_usage,
        page.budget,
        page.budget_variable,
    )
}

fn terminal_only_response(
    terminal: String,
    skipped_encodings: &SkippedEncodings,
    transcoding_notes: &BTreeSet<String>,
    fallback_usage: &FallbackUsage,
    budget: usize,
    budget_variable: &str,
) -> ToolResponse {
    let output = render_grep_output(
        &[],
        transcoding_notes,
        skipped_encodings,
        fallback_usage,
        &terminal,
        budget,
    );
    if estimate_tokens(&output) <= budget {
        ToolResponse::text(output)
    } else {
        budget_too_small(budget, budget_variable)
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
    ToolResponse::error(format!(
        "{budget_variable}={budget} is too small to return the required grep continuation note. Increase it and retry."
    ))
}

fn fit_largest_output(
    maximum: usize,
    budget: usize,
    mut render: impl FnMut(usize) -> String,
) -> Option<String> {
    if maximum == 0 {
        return None;
    }
    let maximum_output = render(maximum);
    if estimate_tokens(&maximum_output) <= budget {
        return Some(maximum_output);
    }
    let mut low = 1_usize;
    let mut high = maximum - 1;
    let mut best = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        let output = render(middle);
        if estimate_tokens(&output) <= budget {
            best = Some(output);
            low = middle.saturating_add(1);
        } else {
            high = middle.saturating_sub(1);
        }
    }
    best
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
        Candidate, CollectSink, EntryMode, FallbackUsage, FileResult, GrepRequest, OutputMode,
        PageFormat, SearchOptions, SkippedEncoding, SkippedEncodings, build_matcher,
        capture_limit_error, format_files_mode, grep_files_with_budget,
        normalize_multiline_pattern, render_grep_output, search_error_message,
    };
    use crate::ToolContent;
    use filetime::{FileTime, set_file_mtime};
    use std::collections::{BTreeMap, BTreeSet};
    use std::io;
    use std::path::PathBuf;
    use std::time::SystemTime;

    #[test]
    fn normalizes_regex_newlines_but_not_literal_backslashes() {
        assert_eq!(normalize_multiline_pattern(r"one\ntwo"), r"one\r?\ntwo");
        assert_eq!(normalize_multiline_pattern(r"one\\ntwo"), r"one\\ntwo");
        assert_eq!(normalize_multiline_pattern("one\ntwo"), r"one\r?\ntwo");
    }

    #[test]
    fn committing_before_context_transfers_its_memory_charge() {
        let matcher = build_matcher("hit", false, false).unwrap();
        let mut sink = CollectSink::new(
            &matcher,
            SearchOptions {
                multiline: false,
                entry_mode: EntryMode::MatchingLine,
                skip_entries: 0,
                max_selected_entries: None,
                capture_content: true,
                capture_match_text: false,
                before_context: 2,
                after_context: 0,
            },
        );
        sink.push_recent(1, b"before\n");
        assert_eq!(sink.recent_bytes, "before".len());
        sink.commit_recent();
        assert_eq!(sink.lines.get(&1).map(String::as_str), Some("before"));
        assert!(sink.recent_lines.is_empty());
        assert_eq!(sink.recent_bytes, 0);
        assert_eq!(sink.stored_bytes, "before".len());
    }

    #[test]
    fn token_budget_returns_at_least_one_entry_and_an_exact_offset() {
        let results = (1..=3)
            .map(|index| FileResult {
                path: format!("{index}-{}", "x".repeat(100)),
                lines: BTreeMap::new(),
                occurrences: BTreeMap::new(),
                entries: Vec::new(),
                occurrence_total: 1,
                total_lines: 1,
                has_trailing_newline: false,
            })
            .collect::<Vec<_>>();
        let skipped_encodings = SkippedEncodings::default();
        let transcoding_notes = BTreeSet::new();
        let fallback_usage = FallbackUsage::default();
        let page = PageFormat {
            offset: 0,
            head_limit: 0,
            budget: 65,
            budget_variable: "FASTCTX_TOKEN_BUDGET",
            scan_complete: false,
            total_entries_seen: 0,
            skipped_encodings: &skipped_encodings,
            transcoding_notes: &transcoding_notes,
            fallback_usage: &fallback_usage,
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
        let results = vec![FileResult {
            path: "/a/very/long/path.txt".to_string(),
            lines: BTreeMap::new(),
            occurrences: BTreeMap::new(),
            entries: Vec::new(),
            occurrence_total: 1,
            total_lines: 1,
            has_trailing_newline: false,
        }];
        let skipped_encodings = SkippedEncodings::default();
        let transcoding_notes = BTreeSet::new();
        let fallback_usage = FallbackUsage::default();
        let page = PageFormat {
            offset: 0,
            head_limit: 1,
            budget: 1,
            budget_variable: "FASTCTX_TOKEN_BUDGET",
            scan_complete: true,
            total_entries_seen: 1,
            skipped_encodings: &skipped_encodings,
            transcoding_notes: &transcoding_notes,
            fallback_usage: &fallback_usage,
        };
        let response = format_files_mode(&results, &page);
        assert!(response.is_error);
        assert_eq!(
            response.content,
            vec![ToolContent::Text(
                "FASTCTX_TOKEN_BUDGET=1 is too small to return the required grep continuation note. Increase it and retry."
                    .to_string()
            )]
        );
    }

    #[test]
    fn encoding_skip_report_uses_remaining_budget_and_keeps_the_terminal_truthful() {
        let paths = (0..3)
            .map(|index| format!("/{}-{index}.txt", "a".repeat(80)))
            .collect::<Vec<_>>();
        let skipped = SkippedEncodings {
            entries: paths
                .iter()
                .map(|path| SkippedEncoding {
                    path: path.clone(),
                    reason: "ambiguous: windows-1252".to_string(),
                })
                .collect(),
        };
        let output = render_grep_output(
            &["/match.txt".to_string()],
            &BTreeSet::new(),
            &skipped,
            &FallbackUsage::default(),
            "(Complete: all 1 file shown.)",
            70,
        );
        assert_eq!(
            output,
            format!(
                "/match.txt\n\n{} — ambiguous: windows-1252\n(Complete: all 1 file shown; 3 files skipped, showing 1 — narrow path/glob to inspect the rest.)",
                paths[0]
            )
        );
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
    fn search_memory_limits_have_exact_actionable_errors() {
        let candidate = Candidate {
            path: PathBuf::from("large.txt"),
            display: "/large.txt".to_string(),
            modified: SystemTime::UNIX_EPOCH,
            file_len: 0,
        };
        assert_eq!(
            search_error_message(&candidate, &io::Error::other("heap limit reached")),
            "Cannot search file /large.txt: a line or multiline buffer exceeds the 64 MiB safety limit. Narrow the path or search without multiline."
        );
        assert_eq!(
            capture_limit_error(&candidate),
            "Cannot search file /large.txt: matching content and context exceed the 64 MiB safety limit. Narrow the pattern or reduce context."
        );
    }
}
