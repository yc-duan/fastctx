//! Mode-specific ripgrep sinks and range-backed content capture.

use crate::operation::{WorkCheckpoint, WorkStop};
use crate::search_text::{RangeText, SearchText};
use grep_matcher::{Match, Matcher};
use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, Sink, SinkContext, SinkContextKind, SinkError, SinkMatch};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

const OCCURRENCE_CHECK_INTERVAL: usize = 256;
const OCCURRENCE_BYTE_CHECK_INTERVAL: usize = 64 * 1024;

/// Content capture options shared by the line and occurrence plans.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ContentSpec {
    pub(crate) multiline: bool,
    pub(crate) skip_entries: usize,
    pub(crate) max_selected_entries: Option<usize>,
    pub(crate) capture_match_text: bool,
    pub(crate) before_context: usize,
    pub(crate) after_context: usize,
    pub(crate) capture_heap_limit_bytes: usize,
}

/// The minimum search work required by one grep output mode.
#[derive(Clone, Copy, Debug)]
pub(crate) enum GrepSearchPlan {
    Exists,
    Count,
    ContentLine(ContentSpec),
    ContentOccurrence(ContentSpec),
}

impl GrepSearchPlan {
    pub(crate) fn content_multiline(self) -> Option<bool> {
        match self {
            Self::Exists | Self::Count => None,
            Self::ContentLine(spec) | Self::ContentOccurrence(spec) => Some(spec.multiline),
        }
    }

    pub(crate) fn before_context(self) -> usize {
        match self {
            Self::ContentLine(spec) | Self::ContentOccurrence(spec) => spec.before_context,
            Self::Exists | Self::Count => 0,
        }
    }

    pub(crate) fn after_context(self) -> usize {
        match self {
            Self::ContentLine(spec) | Self::ContentOccurrence(spec) => spec.after_context,
            Self::Exists | Self::Count => 0,
        }
    }

    pub(crate) fn with_content_window(
        self,
        skip_entries: usize,
        max_selected_entries: Option<usize>,
    ) -> Self {
        match self {
            Self::ContentLine(mut spec) => {
                spec.skip_entries = skip_entries;
                spec.max_selected_entries = max_selected_entries;
                Self::ContentLine(spec)
            }
            Self::ContentOccurrence(mut spec) => {
                spec.skip_entries = skip_entries;
                spec.max_selected_entries = max_selected_entries;
                Self::ContentOccurrence(spec)
            }
            Self::Exists | Self::Count => self,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LineMatchSpan {
    pub(crate) line_number: usize,
    pub(crate) match_char_start: usize,
    pub(crate) match_char_len: usize,
}

#[derive(Clone, Debug)]
struct TextRange {
    backing: Arc<SearchText>,
    range: Range<u64>,
    cached: OnceLock<Arc<str>>,
}

impl TextRange {
    fn new(backing: Arc<SearchText>, range: Range<u64>) -> Self {
        Self {
            backing,
            range,
            cached: OnceLock::new(),
        }
    }

    fn as_str(&self) -> io::Result<&str> {
        if let Some(text) = self.cached.get() {
            return Ok(text);
        }
        match self.backing.range_str(self.range.clone())? {
            RangeText::Borrowed(text) => Ok(text),
            RangeText::Owned(text) => {
                let _ = self.cached.set(text);
                Ok(self
                    .cached
                    .get()
                    .expect("the captured temp range was initialized"))
            }
        }
    }

    fn len(&self) -> usize {
        usize::try_from(self.range.end.saturating_sub(self.range.start)).unwrap_or(usize::MAX)
    }
}

#[derive(Debug)]
pub(crate) struct CapturedLine {
    text: TextRange,
    chars: OnceLock<Arc<[char]>>,
}

impl CapturedLine {
    fn new(backing: Arc<SearchText>, range: Range<u64>) -> Self {
        Self {
            text: TextRange::new(backing, range),
            chars: OnceLock::new(),
        }
    }

    pub(crate) fn as_str(&self) -> io::Result<&str> {
        self.text.as_str()
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.text.len()
    }

    pub(crate) fn chars(&self) -> io::Result<&[char]> {
        if self.chars.get().is_none() {
            let chars = Arc::from(self.as_str()?.chars().collect::<Vec<_>>());
            let _ = self.chars.set(chars);
        }
        Ok(self
            .chars
            .get()
            .expect("the captured line character index was initialized"))
    }

    pub(crate) fn char_count(&self) -> io::Result<usize> {
        Ok(self.chars()?.len())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Occurrence {
    matched_text: Option<TextRange>,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) line_spans: Vec<LineMatchSpan>,
}

impl Occurrence {
    pub(crate) fn matched_text(&self) -> io::Result<&str> {
        self.matched_text
            .as_ref()
            .map(TextRange::as_str)
            .unwrap_or(Ok(""))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ContentEntry {
    MatchingLine(usize),
    Occurrence {
        start_line: usize,
        occurrence_index: usize,
    },
}

#[derive(Debug)]
pub(crate) struct ContentPayload {
    pub(crate) lines: BTreeMap<usize, CapturedLine>,
    pub(crate) occurrences: BTreeMap<usize, Vec<Occurrence>>,
    pub(crate) entries: Vec<ContentEntry>,
    occurrence_total: usize,
    pub(crate) total_lines: usize,
    pub(crate) has_trailing_newline: bool,
}

#[derive(Debug)]
enum FilePayload {
    Exists,
    Count { occurrence_total: usize },
    Content(ContentPayload),
}

#[derive(Debug)]
pub(crate) struct FileResult {
    path: String,
    payload: FilePayload,
}

impl FileResult {
    fn exists(path: String) -> Self {
        Self {
            path,
            payload: FilePayload::Exists,
        }
    }

    pub(crate) fn count(path: String, occurrence_total: usize) -> Self {
        Self {
            path,
            payload: FilePayload::Count { occurrence_total },
        }
    }

    fn with_content(path: String, content: ContentPayload) -> Self {
        Self {
            path,
            payload: FilePayload::Content(content),
        }
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn occurrence_count(&self) -> usize {
        match &self.payload {
            FilePayload::Exists => 1,
            FilePayload::Count { occurrence_total } => *occurrence_total,
            FilePayload::Content(content) => content.occurrence_total,
        }
    }

    pub(crate) fn content(&self) -> &ContentPayload {
        match &self.payload {
            FilePayload::Content(content) => content,
            FilePayload::Exists | FilePayload::Count { .. } => {
                unreachable!("only content results carry captured text")
            }
        }
    }

    pub(crate) fn content_mut(&mut self) -> &mut ContentPayload {
        match &mut self.payload {
            FilePayload::Content(content) => content,
            FilePayload::Exists | FilePayload::Count { .. } => {
                unreachable!("only content results carry captured text")
            }
        }
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.content().entries.len()
    }

    pub(crate) fn trim_entries(&mut self, start: usize, end: usize) {
        let entries = &mut self.content_mut().entries;
        entries.drain(..start);
        entries.truncate(end - start);
    }
}

pub(crate) struct SinkOutput {
    pub(crate) result: Option<FileResult>,
    pub(crate) entries_seen: usize,
}

#[derive(Debug)]
pub(crate) enum GrepSinkError {
    Io(io::Error),
    Search(String),
    Stopped(WorkStop),
    CaptureOverflow,
    CountOverflow,
}

impl fmt::Display for GrepSinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Search(message) => formatter.write_str(message),
            Self::Stopped(WorkStop::RequestCancelled) => formatter.write_str("request cancelled"),
            Self::Stopped(WorkStop::EpochRetired) => formatter.write_str("search epoch retired"),
            Self::CaptureOverflow => formatter.write_str("grep capture limit exceeded"),
            Self::CountOverflow => formatter.write_str("grep occurrence count overflowed"),
        }
    }
}

impl std::error::Error for GrepSinkError {}

impl SinkError for GrepSinkError {
    fn error_message<T: fmt::Display>(message: T) -> Self {
        Self::Search(message.to_string())
    }

    fn error_io(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SinkMetrics {
    pub(crate) sink_matches: usize,
    pub(crate) occurrences_examined: usize,
    pub(crate) line_layout_builds: usize,
    pub(crate) line_layout_bytes: usize,
    #[cfg(test)]
    pub(crate) source_bytes_copied: usize,
}

struct SinkControl<'a> {
    matcher: &'a RegexMatcher,
    operation: Option<&'a dyn WorkCheckpoint>,
    occurrences_since_check: usize,
    bytes_since_check: usize,
    metrics: SinkMetrics,
}

impl<'a> SinkControl<'a> {
    fn new(matcher: &'a RegexMatcher, operation: Option<&'a dyn WorkCheckpoint>) -> Self {
        Self {
            matcher,
            operation,
            occurrences_since_check: 0,
            bytes_since_check: 0,
            metrics: SinkMetrics::default(),
        }
    }

    fn checkpoint(&self) -> Result<(), GrepSinkError> {
        match self.operation.map(WorkCheckpoint::check_work) {
            Some(Err(stop)) => Err(GrepSinkError::Stopped(stop)),
            Some(Ok(())) | None => Ok(()),
        }
    }

    fn sink_match_checkpoint(&mut self) -> Result<(), GrepSinkError> {
        self.checkpoint()?;
        #[cfg(test)]
        if let Some(operation) = self.operation {
            operation.stage(crate::operation::TestStage::SinkMatch);
        }
        self.checkpoint()?;
        self.metrics.sink_matches = self.metrics.sink_matches.saturating_add(1);
        Ok(())
    }

    fn occurrence_checkpoint(&mut self, span_bytes: usize) -> Result<(), GrepSinkError> {
        self.metrics.occurrences_examined = self.metrics.occurrences_examined.saturating_add(1);
        self.occurrences_since_check = self.occurrences_since_check.saturating_add(1);
        self.bytes_since_check = self.bytes_since_check.saturating_add(span_bytes);
        if self.occurrences_since_check < OCCURRENCE_CHECK_INTERVAL
            && self.bytes_since_check < OCCURRENCE_BYTE_CHECK_INTERVAL
        {
            return Ok(());
        }
        self.occurrences_since_check = 0;
        self.bytes_since_check = 0;
        self.checkpoint()?;
        #[cfg(test)]
        if let Some(operation) = self.operation {
            operation.stage(crate::operation::TestStage::OccurrenceBatch);
        }
        self.checkpoint()
    }
}

fn impossible_matcher_error(error: grep_matcher::NoError) -> GrepSinkError {
    let _ = error;
    unreachable!("RegexMatcher's error type has no constructible value")
}

struct ExistsSink<'a> {
    control: SinkControl<'a>,
    matched: bool,
}

impl Sink for ExistsSink<'_> {
    type Error = GrepSinkError;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        matched: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        self.control.sink_match_checkpoint()?;
        let bytes = matched.bytes();
        let matcher = self.control.matcher;
        let iter = matcher.try_find_iter(bytes, |found| {
            if is_synthetic_trailing_empty(bytes, found) {
                return Ok(true);
            }
            self.control
                .occurrence_checkpoint(found.end().saturating_sub(found.start()))?;
            self.matched = true;
            Ok(false)
        });
        match iter {
            Err(error) => Err(impossible_matcher_error(error)),
            Ok(Err(error)) => Err(error),
            Ok(Ok(())) => Ok(!self.matched),
        }
    }
}

struct CountSink<'a> {
    control: SinkControl<'a>,
    occurrence_total: usize,
}

impl Sink for CountSink<'_> {
    type Error = GrepSinkError;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        matched: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        self.control.sink_match_checkpoint()?;
        let bytes = matched.bytes();
        let matcher = self.control.matcher;
        let iter = matcher.try_find_iter(bytes, |found| {
            if is_synthetic_trailing_empty(bytes, found) {
                return Ok(true);
            }
            self.control
                .occurrence_checkpoint(found.end().saturating_sub(found.start()))?;
            self.occurrence_total = self
                .occurrence_total
                .checked_add(1)
                .ok_or(GrepSinkError::CountOverflow)?;
            Ok(true)
        });
        match iter {
            Err(error) => Err(impossible_matcher_error(error)),
            Ok(Err(error)) => Err(error),
            Ok(Ok(())) => Ok(true),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentMode {
    Line,
    Occurrence,
}

struct ContentSink<'a> {
    control: SinkControl<'a>,
    backing: Arc<SearchText>,
    mode: ContentMode,
    spec: ContentSpec,
    occurrences: BTreeMap<usize, Vec<Occurrence>>,
    entries: Vec<ContentEntry>,
    lines: BTreeMap<usize, CapturedLine>,
    occurrence_total: usize,
    entries_seen: usize,
    selected_entry_count: usize,
    last_matching_line: Option<usize>,
    current_line_selected: bool,
    recent_lines: VecDeque<(usize, CapturedLine)>,
    recent_bytes: usize,
    stored_bytes: usize,
    after_context_until: usize,
    capture_overflow: bool,
}

impl<'a> ContentSink<'a> {
    fn new(
        matcher: &'a RegexMatcher,
        operation: Option<&'a dyn WorkCheckpoint>,
        mode: ContentMode,
        spec: ContentSpec,
        backing: Arc<SearchText>,
    ) -> Self {
        Self {
            control: SinkControl::new(matcher, operation),
            backing,
            mode,
            spec,
            occurrences: BTreeMap::new(),
            entries: Vec::new(),
            lines: BTreeMap::new(),
            occurrence_total: 0,
            entries_seen: 0,
            selected_entry_count: 0,
            last_matching_line: None,
            current_line_selected: false,
            recent_lines: VecDeque::with_capacity(spec.before_context.min(1_024)),
            recent_bytes: 0,
            stored_bytes: 0,
            after_context_until: 0,
            capture_overflow: false,
        }
    }

    fn reserve(&mut self, additional: usize) -> bool {
        if self
            .stored_bytes
            .saturating_add(self.recent_bytes)
            .saturating_add(additional)
            > self.spec.capture_heap_limit_bytes
        {
            self.capture_overflow = true;
            false
        } else {
            true
        }
    }

    fn store_line(&mut self, line_number: usize, line: CapturedLine) {
        if self.lines.contains_key(&line_number) {
            return;
        }
        let len = line.byte_len();
        if !self.reserve(len) {
            return;
        }
        self.stored_bytes = self.stored_bytes.saturating_add(len);
        self.lines.insert(line_number, line);
    }

    fn push_recent(&mut self, line_number: usize, line: CapturedLine) {
        if self.spec.before_context == 0
            || self
                .recent_lines
                .back()
                .is_some_and(|(recent_line, _)| *recent_line == line_number)
        {
            return;
        }
        while self.recent_lines.len() >= self.spec.before_context {
            if let Some((_, removed)) = self.recent_lines.pop_front() {
                self.recent_bytes = self.recent_bytes.saturating_sub(removed.byte_len());
            }
        }
        let len = line.byte_len();
        if !self.reserve(len) {
            return;
        }
        self.recent_bytes = self.recent_bytes.saturating_add(len);
        self.recent_lines.push_back((line_number, line));
    }

    fn commit_recent(&mut self) {
        let recent = std::mem::take(&mut self.recent_lines);
        self.recent_bytes = 0;
        for (line_number, line) in recent {
            if self.lines.contains_key(&line_number) {
                continue;
            }
            self.stored_bytes = self.stored_bytes.saturating_add(line.byte_len());
            self.lines.insert(line_number, line);
        }
    }

    fn increment_entries_seen(&mut self) -> Result<(), GrepSinkError> {
        self.entries_seen = self
            .entries_seen
            .checked_add(1)
            .ok_or(GrepSinkError::CountOverflow)?;
        Ok(())
    }

    fn select_line(&mut self, line_number: usize) -> Result<bool, GrepSinkError> {
        if self.last_matching_line == Some(line_number) {
            return Ok(self.current_line_selected);
        }
        self.last_matching_line = Some(line_number);
        self.increment_entries_seen()?;
        self.current_line_selected = self.entries_seen > self.spec.skip_entries
            && self
                .spec
                .max_selected_entries
                .is_none_or(|limit| self.selected_entry_count < limit);
        if self.current_line_selected {
            self.selected_entry_count = self
                .selected_entry_count
                .checked_add(1)
                .ok_or(GrepSinkError::CountOverflow)?;
            self.entries.push(ContentEntry::MatchingLine(line_number));
            self.commit_recent();
            self.after_context_until = self
                .after_context_until
                .max(line_number.saturating_add(self.spec.after_context));
        }
        Ok(self.current_line_selected)
    }

    fn select_occurrence(&mut self, end_line: usize) -> Result<bool, GrepSinkError> {
        self.increment_entries_seen()?;
        let selected = self.entries_seen > self.spec.skip_entries
            && self
                .spec
                .max_selected_entries
                .is_none_or(|limit| self.selected_entry_count < limit);
        if selected {
            self.selected_entry_count = self
                .selected_entry_count
                .checked_add(1)
                .ok_or(GrepSinkError::CountOverflow)?;
            self.commit_recent();
            self.after_context_until = self
                .after_context_until
                .max(end_line.saturating_add(self.spec.after_context));
        }
        Ok(selected)
    }

    fn limit_reached(&self) -> bool {
        self.spec
            .max_selected_entries
            .is_some_and(|limit| self.selected_entry_count >= limit)
    }

    fn store_occurrence(&mut self, pending: PendingOccurrence) {
        let metadata_bytes = std::mem::size_of::<Occurrence>().saturating_add(
            pending
                .line_spans
                .len()
                .saturating_mul(std::mem::size_of::<LineMatchSpan>()),
        );
        let text_bytes = if self.spec.capture_match_text {
            usize::try_from(
                pending
                    .match_range
                    .end
                    .saturating_sub(pending.match_range.start),
            )
            .unwrap_or(usize::MAX)
        } else {
            0
        };
        if !self.reserve(metadata_bytes.saturating_add(text_bytes)) {
            return;
        }
        self.stored_bytes = self
            .stored_bytes
            .saturating_add(metadata_bytes)
            .saturating_add(text_bytes);
        let occurrences = self.occurrences.entry(pending.start_line).or_default();
        let occurrence_index = occurrences.len();
        occurrences.push(Occurrence {
            matched_text: self
                .spec
                .capture_match_text
                .then(|| TextRange::new(Arc::clone(&self.backing), pending.match_range)),
            start_line: pending.start_line,
            end_line: pending.end_line,
            line_spans: pending.line_spans,
        });
        if self.mode == ContentMode::Occurrence {
            self.entries.push(ContentEntry::Occurrence {
                start_line: pending.start_line,
                occurrence_index,
            });
        }
    }

    fn capture_callback_lines(
        &mut self,
        layout: &LineLayout,
        selected_lines: &LineIntervals,
        absolute_start: u64,
    ) -> Result<(), GrepSinkError> {
        let mut interval_cursor = 0_usize;
        for (line_delta, line) in layout.lines.iter().enumerate() {
            let line_number = layout.first_line_number.saturating_add(line_delta);
            let selected = selected_lines.contains(line_number, &mut interval_cursor);
            if selected || line_number <= self.after_context_until {
                self.store_line(
                    line_number,
                    CapturedLine::new(
                        Arc::clone(&self.backing),
                        absolute_range(absolute_start, line.content_start..line.content_end)?,
                    ),
                );
            }
            if self.spec.before_context > 0 {
                self.push_recent(
                    line_number,
                    CapturedLine::new(
                        Arc::clone(&self.backing),
                        absolute_range(absolute_start, line.content_start..line.content_end)?,
                    ),
                );
            }
            if self.capture_overflow {
                return Ok(());
            }
        }
        Ok(())
    }
}

impl Sink for ContentSink<'_> {
    type Error = GrepSinkError;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        matched: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        self.control.sink_match_checkpoint()?;
        let bytes = matched.bytes();
        let absolute_start = matched.absolute_byte_offset();
        let first_line_number = matched.line_number().unwrap_or(1) as usize;
        let mut layout = None;
        let mut cursor = OccurrenceCursor::default();
        let mut selected_lines = LineIntervals::default();
        let mut pending = Vec::new();
        let mode = self.mode;
        let matcher = self.control.matcher;
        let iter = matcher.try_find_iter(bytes, |found| {
            if is_synthetic_trailing_empty(bytes, found) {
                return Ok(true);
            }
            self.control
                .occurrence_checkpoint(found.end().saturating_sub(found.start()))?;
            let mut located = None;
            let selected = match mode {
                ContentMode::Line => self.select_line(first_line_number)?,
                ContentMode::Occurrence => {
                    if layout.is_none() {
                        layout = Some(LineLayout::new(bytes, first_line_number));
                        self.control.metrics.line_layout_builds =
                            self.control.metrics.line_layout_builds.saturating_add(1);
                        self.control.metrics.line_layout_bytes = self
                            .control
                            .metrics
                            .line_layout_bytes
                            .saturating_add(bytes.len());
                    }
                    let indexes = layout
                        .as_ref()
                        .expect("the occurrence layout was initialized")
                        .locate(found, &mut cursor);
                    let end_line = first_line_number.saturating_add(indexes.1);
                    located = Some(indexes);
                    self.select_occurrence(end_line)?
                }
            };
            self.occurrence_total = self
                .occurrence_total
                .checked_add(1)
                .ok_or(GrepSinkError::CountOverflow)?;
            if selected {
                if layout.is_none() {
                    layout = Some(LineLayout::new(bytes, first_line_number));
                    self.control.metrics.line_layout_builds =
                        self.control.metrics.line_layout_builds.saturating_add(1);
                    self.control.metrics.line_layout_bytes = self
                        .control
                        .metrics
                        .line_layout_bytes
                        .saturating_add(bytes.len());
                }
                let layout = layout
                    .as_mut()
                    .expect("a selected occurrence has one line layout");
                let (start_index, end_index) =
                    located.unwrap_or_else(|| layout.locate(found, &mut cursor));
                let start_line = first_line_number.saturating_add(start_index);
                let end_line = first_line_number.saturating_add(end_index);
                selected_lines.insert(start_line, end_line);
                pending.push(PendingOccurrence {
                    match_range: absolute_range(absolute_start, found.start()..found.end())?,
                    start_line,
                    end_line,
                    line_spans: layout.line_spans(found, start_index, end_index),
                });
            }
            Ok(match mode {
                ContentMode::Line => selected,
                ContentMode::Occurrence => !self.limit_reached(),
            })
        });
        match iter {
            Err(error) => return Err(impossible_matcher_error(error)),
            Ok(Err(error)) => return Err(error),
            Ok(Ok(())) => {}
        }
        if !pending.is_empty() {
            for occurrence in pending {
                self.store_occurrence(occurrence);
                if self.capture_overflow {
                    break;
                }
            }
            if !self.capture_overflow {
                self.capture_callback_lines(
                    layout
                        .as_ref()
                        .expect("captured occurrences have one line layout"),
                    &selected_lines,
                    absolute_start,
                )?;
            }
        }
        if self.capture_overflow {
            return Err(GrepSinkError::CaptureOverflow);
        }
        Ok(!self.limit_reached())
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        self.control.checkpoint()?;
        if let Some(line_number) = context.line_number() {
            let line_number = line_number as usize;
            let capture_after = line_number <= self.after_context_until;
            let capture_recent = self.spec.before_context > 0
                && matches!(
                    context.kind(),
                    SinkContextKind::Before | SinkContextKind::After
                );
            if capture_after || capture_recent {
                let bytes = strip_line_terminator(context.bytes());
                let range = absolute_range(context.absolute_byte_offset(), 0..bytes.len())?;
                if capture_after {
                    self.store_line(
                        line_number,
                        CapturedLine::new(Arc::clone(&self.backing), range.clone()),
                    );
                }
                if capture_recent {
                    self.push_recent(
                        line_number,
                        CapturedLine::new(Arc::clone(&self.backing), range),
                    );
                }
            }
        }
        if self.capture_overflow {
            Err(GrepSinkError::CaptureOverflow)
        } else {
            Ok(true)
        }
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        self.control.checkpoint()?;
        self.recent_lines.clear();
        self.recent_bytes = 0;
        self.after_context_until = 0;
        Ok(true)
    }
}

enum PlanSinkKind<'a> {
    Exists(ExistsSink<'a>),
    Count(CountSink<'a>),
    Content(Box<ContentSink<'a>>),
}

/// One concrete sink dispatch point without mode booleans in hot callbacks.
pub(crate) struct PlanSink<'a> {
    inner: PlanSinkKind<'a>,
}

impl<'a> PlanSink<'a> {
    pub(crate) fn new(
        matcher: &'a RegexMatcher,
        plan: GrepSearchPlan,
        operation: Option<&'a dyn WorkCheckpoint>,
        content_backing: Option<Arc<SearchText>>,
    ) -> Self {
        let inner = match plan {
            GrepSearchPlan::Exists => PlanSinkKind::Exists(ExistsSink {
                control: SinkControl::new(matcher, operation),
                matched: false,
            }),
            GrepSearchPlan::Count => PlanSinkKind::Count(CountSink {
                control: SinkControl::new(matcher, operation),
                occurrence_total: 0,
            }),
            GrepSearchPlan::ContentLine(spec) => PlanSinkKind::Content(Box::new(ContentSink::new(
                matcher,
                operation,
                ContentMode::Line,
                {
                    debug_assert!(!spec.multiline);
                    spec
                },
                content_backing
                    .clone()
                    .expect("content plans require one immutable UTF-8 backing"),
            ))),
            GrepSearchPlan::ContentOccurrence(spec) => {
                PlanSinkKind::Content(Box::new(ContentSink::new(
                    matcher,
                    operation,
                    ContentMode::Occurrence,
                    spec,
                    content_backing.expect("content plans require one immutable UTF-8 backing"),
                )))
            }
        };
        Self { inner }
    }

    pub(crate) fn into_output(
        self,
        path: String,
        total_lines: usize,
        has_trailing_newline: bool,
    ) -> SinkOutput {
        match self.inner {
            PlanSinkKind::Exists(sink) => SinkOutput {
                result: sink.matched.then(|| FileResult::exists(path)),
                entries_seen: usize::from(sink.matched),
            },
            PlanSinkKind::Count(sink) => SinkOutput {
                result: (sink.occurrence_total > 0)
                    .then(|| FileResult::count(path, sink.occurrence_total)),
                entries_seen: usize::from(sink.occurrence_total > 0),
            },
            PlanSinkKind::Content(sink) => {
                let sink = *sink;
                let entries_seen = sink.entries_seen;
                let result = (!sink.entries.is_empty()).then(|| {
                    FileResult::with_content(
                        path,
                        ContentPayload {
                            lines: sink.lines,
                            occurrences: sink.occurrences,
                            entries: sink.entries,
                            occurrence_total: sink.occurrence_total,
                            total_lines,
                            has_trailing_newline,
                        },
                    )
                });
                SinkOutput {
                    result,
                    entries_seen,
                }
            }
        }
    }

    #[cfg(test)]
    fn metrics(&self) -> SinkMetrics {
        match &self.inner {
            PlanSinkKind::Exists(sink) => sink.control.metrics,
            PlanSinkKind::Count(sink) => sink.control.metrics,
            PlanSinkKind::Content(sink) => sink.control.metrics,
        }
    }
}

impl Sink for PlanSink<'_> {
    type Error = GrepSinkError;

    fn matched(
        &mut self,
        searcher: &Searcher,
        matched: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        match &mut self.inner {
            PlanSinkKind::Exists(sink) => sink.matched(searcher, matched),
            PlanSinkKind::Count(sink) => sink.matched(searcher, matched),
            PlanSinkKind::Content(sink) => sink.matched(searcher, matched),
        }
    }

    fn context(
        &mut self,
        searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        match &mut self.inner {
            PlanSinkKind::Content(sink) => sink.context(searcher, context),
            PlanSinkKind::Exists(_) | PlanSinkKind::Count(_) => Ok(true),
        }
    }

    fn context_break(&mut self, searcher: &Searcher) -> Result<bool, Self::Error> {
        match &mut self.inner {
            PlanSinkKind::Content(sink) => sink.context_break(searcher),
            PlanSinkKind::Exists(_) | PlanSinkKind::Count(_) => Ok(true),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct LogicalLine {
    content_start: usize,
    content_end: usize,
    full_end: usize,
}

struct LineLayout<'a> {
    bytes: &'a [u8],
    first_line_number: usize,
    lines: Vec<LogicalLine>,
    char_cursors: Vec<CharCursor>,
}

#[derive(Clone, Copy, Debug)]
struct CharCursor {
    byte_offset: usize,
    char_offset: usize,
}

impl<'a> LineLayout<'a> {
    fn new(bytes: &'a [u8], first_line_number: usize) -> Self {
        let mut lines = Vec::new();
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
            lines.push(LogicalLine {
                content_start: line_start,
                content_end,
                full_end: index + 1,
            });
            line_start = index + 1;
        }
        if line_start < bytes.len() || lines.is_empty() {
            lines.push(LogicalLine {
                content_start: line_start,
                content_end: bytes.len(),
                full_end: bytes.len(),
            });
        }
        let char_cursors = lines
            .iter()
            .map(|line| CharCursor {
                byte_offset: line.content_start,
                char_offset: 0,
            })
            .collect();
        Self {
            bytes,
            first_line_number,
            lines,
            char_cursors,
        }
    }

    fn locate(&self, found: Match, cursor: &mut OccurrenceCursor) -> (usize, usize) {
        while cursor.start + 1 < self.lines.len()
            && !offset_is_in_line(&self.lines, cursor.start, found.start())
        {
            cursor.start += 1;
        }
        cursor.end = cursor.end.max(cursor.start);
        let end_offset = if found.end() > found.start() {
            found.end() - 1
        } else {
            found.start()
        };
        while cursor.end + 1 < self.lines.len()
            && !offset_is_in_line(&self.lines, cursor.end, end_offset)
        {
            cursor.end += 1;
        }
        (cursor.start, cursor.end)
    }

    fn line_spans(
        &mut self,
        found: Match,
        start_index: usize,
        end_index: usize,
    ) -> Vec<LineMatchSpan> {
        let mut spans = Vec::with_capacity(end_index.saturating_sub(start_index) + 1);
        for line_index in start_index..=end_index {
            let line = self.lines[line_index];
            let overlap_start = found.start().max(line.content_start).min(line.content_end);
            let overlap_end = found.end().min(line.content_end).max(overlap_start);
            let anchor = if overlap_start < overlap_end {
                overlap_start
            } else if found.start() <= line.content_start {
                line.content_start
            } else {
                line.content_end
            };
            let match_char_start = self.char_offset(line_index, anchor);
            let match_char_len = if overlap_start < overlap_end {
                self.char_offset(line_index, overlap_end)
                    .saturating_sub(match_char_start)
            } else {
                0
            };
            spans.push(LineMatchSpan {
                line_number: self.first_line_number.saturating_add(line_index),
                match_char_start,
                match_char_len,
            });
        }
        spans
    }

    fn char_offset(&mut self, line_index: usize, absolute: usize) -> usize {
        let line = self.lines[line_index];
        let target = absolute.clamp(line.content_start, line.content_end);
        let cursor = &mut self.char_cursors[line_index];
        debug_assert!(target >= cursor.byte_offset);
        if target > cursor.byte_offset {
            let text =
                unsafe { std::str::from_utf8_unchecked(&self.bytes[cursor.byte_offset..target]) };
            cursor.char_offset = cursor.char_offset.saturating_add(text.chars().count());
            cursor.byte_offset = target;
        }
        cursor.char_offset
    }
}

#[derive(Default)]
struct OccurrenceCursor {
    start: usize,
    end: usize,
}

#[derive(Default)]
struct LineIntervals {
    ranges: Vec<(usize, usize)>,
}

impl LineIntervals {
    fn insert(&mut self, start: usize, end: usize) {
        if let Some(last) = self.ranges.last_mut()
            && start <= last.1.saturating_add(1)
        {
            last.1 = last.1.max(end);
        } else {
            self.ranges.push((start, end));
        }
    }

    fn contains(&self, line: usize, cursor: &mut usize) -> bool {
        while *cursor < self.ranges.len() && self.ranges[*cursor].1 < line {
            *cursor += 1;
        }
        self.ranges
            .get(*cursor)
            .is_some_and(|(start, end)| *start <= line && line <= *end)
    }
}

struct PendingOccurrence {
    match_range: Range<u64>,
    start_line: usize,
    end_line: usize,
    line_spans: Vec<LineMatchSpan>,
}

fn offset_is_in_line(lines: &[LogicalLine], index: usize, offset: usize) -> bool {
    let line = lines[index];
    offset < line.full_end || (index + 1 == lines.len() && offset <= line.full_end)
}

fn strip_line_terminator(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

fn absolute_range(base: u64, local: Range<usize>) -> Result<Range<u64>, GrepSinkError> {
    let start = u64::try_from(local.start)
        .ok()
        .and_then(|offset| base.checked_add(offset));
    let end = u64::try_from(local.end)
        .ok()
        .and_then(|offset| base.checked_add(offset));
    match (start, end) {
        (Some(start), Some(end)) if start <= end => Ok(start..end),
        _ => Err(GrepSinkError::Search(
            "ripgrep reported a byte offset that overflowed the search source".to_string(),
        )),
    }
}

fn is_synthetic_trailing_empty(bytes: &[u8], found: Match) -> bool {
    found.start() == found.end() && found.end() == bytes.len() && bytes.ends_with(b"\n")
}

#[cfg(test)]
mod tests {
    use super::{ContentSpec, GrepSearchPlan, GrepSinkError, PlanSink};
    use crate::operation::{RequestWorkGuard, TestStage};
    use grep_matcher::LineTerminator;
    use grep_regex::RegexMatcherBuilder;
    use grep_searcher::SearcherBuilder;
    use rmcp::model::RequestId;
    use std::io::Cursor;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio_util::sync::CancellationToken;

    fn matcher(pattern: &str, multiline: bool) -> grep_regex::RegexMatcher {
        let mut builder = RegexMatcherBuilder::new();
        builder
            .multi_line(true)
            .crlf(true)
            .dot_matches_new_line(multiline);
        if multiline {
            builder.line_terminator(None);
        }
        builder.build(pattern).unwrap()
    }

    fn search(
        pattern: &str,
        bytes: &[u8],
        plan: GrepSearchPlan,
    ) -> (super::SinkOutput, super::SinkMetrics) {
        let multiline = plan.content_multiline().unwrap_or(false);
        let matcher = matcher(pattern, multiline);
        let mut searcher = SearcherBuilder::new();
        searcher
            .line_number(true)
            .line_terminator(LineTerminator::crlf())
            .multi_line(multiline)
            .before_context(plan.before_context())
            .after_context(plan.after_context());
        let content_backing = plan
            .content_multiline()
            .is_some()
            .then(|| crate::search_text::SearchText::capture(Cursor::new(bytes), None).unwrap());
        let mut sink = PlanSink::new(&matcher, plan, None, content_backing);
        searcher
            .build()
            .search_slice(&matcher, bytes, &mut sink)
            .unwrap();
        let metrics = sink.metrics();
        let total_lines = if bytes.is_empty() {
            0
        } else {
            bytes
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
                .saturating_add(1)
        };
        (
            sink.into_output(
                "/dense.txt".to_string(),
                total_lines,
                bytes.ends_with(b"\n"),
            ),
            metrics,
        )
    }

    fn content_spec(multiline: bool, capture_match_text: bool) -> ContentSpec {
        ContentSpec {
            multiline,
            skip_entries: 0,
            max_selected_entries: None,
            capture_match_text,
            before_context: 0,
            after_context: 0,
            capture_heap_limit_bytes: 64 * 1024 * 1024,
        }
    }

    #[test]
    fn exists_stops_at_one_occurrence_and_count_builds_no_line_layout() {
        let bytes = "hit ".repeat(65_536);
        let (exists, exists_metrics) = search("hit", bytes.as_bytes(), GrepSearchPlan::Exists);
        assert!(exists.result.is_some());
        assert_eq!(exists_metrics.occurrences_examined, 1);
        assert_eq!(exists_metrics.line_layout_builds, 0);

        let (count, count_metrics) = search("hit", bytes.as_bytes(), GrepSearchPlan::Count);
        assert_eq!(count.result.unwrap().occurrence_count(), 65_536);
        assert_eq!(count_metrics.occurrences_examined, 65_536);
        assert_eq!(count_metrics.line_layout_builds, 0);
    }

    #[test]
    fn dense_content_builds_one_layout_and_keeps_range_backed_occurrences() {
        let bytes = "界hit ".repeat(8_192);
        let (output, metrics) = search(
            "hit",
            bytes.as_bytes(),
            GrepSearchPlan::ContentLine(content_spec(false, false)),
        );
        let result = output.result.unwrap();
        let content = result.content();
        assert_eq!(metrics.sink_matches, 1);
        assert_eq!(metrics.line_layout_builds, 1);
        assert_eq!(metrics.occurrences_examined, 8_192);
        assert_eq!(content.entries, [super::ContentEntry::MatchingLine(1)]);
        assert_eq!(content.occurrences[&1].len(), 8_192);
        assert_eq!(content.lines[&1].as_str().unwrap(), bytes);
        assert_eq!(content.lines[&1].char_count().unwrap(), 5 * 8_192);
        assert_eq!(metrics.source_bytes_copied, 0);
    }

    #[test]
    fn dense_mode_counters_follow_the_declared_doubling_curve() {
        for occurrences in [512, 1_024, 2_048, 4_096, 8_192, 16_384, 32_768, 65_536] {
            let bytes = "x".repeat(occurrences);

            let (exists, exists_metrics) = search("x", bytes.as_bytes(), GrepSearchPlan::Exists);
            assert!(exists.result.is_some());
            assert_eq!(exists_metrics.occurrences_examined, 1);
            assert_eq!(exists_metrics.line_layout_builds, 0);

            let (count, count_metrics) = search("x", bytes.as_bytes(), GrepSearchPlan::Count);
            assert_eq!(count.result.unwrap().occurrence_count(), occurrences);
            assert_eq!(count_metrics.sink_matches, 1);
            assert_eq!(count_metrics.occurrences_examined, occurrences);
            assert_eq!(count_metrics.line_layout_builds, 0);

            let (content, content_metrics) = search(
                "x",
                bytes.as_bytes(),
                GrepSearchPlan::ContentLine(content_spec(false, false)),
            );
            let result = content.result.unwrap();
            assert_eq!(result.content().occurrences[&1].len(), occurrences);
            assert_eq!(content_metrics.sink_matches, 1);
            assert_eq!(content_metrics.occurrences_examined, occurrences);
            assert_eq!(content_metrics.line_layout_builds, 1);
            assert_eq!(content_metrics.line_layout_bytes, occurrences);
        }
    }

    #[test]
    fn range_backed_payloads_are_send_and_static() {
        fn assert_send_static<T: Send + 'static>() {}

        assert_send_static::<super::FileResult>();
        assert_send_static::<crate::search_text::SearchText>();
    }

    #[test]
    fn skipped_dense_lines_do_not_build_layouts_or_enumerate_their_tail() {
        let first = "hit ".repeat(65_536);
        let bytes = format!("{first}\nhit hit\n{first}\n");
        let mut spec = content_spec(false, false);
        spec.skip_entries = 1;
        spec.max_selected_entries = Some(1);
        let (output, metrics) = search("hit", bytes.as_bytes(), GrepSearchPlan::ContentLine(spec));
        let result = output.result.unwrap();
        let content = result.content();
        assert_eq!(content.entries, [super::ContentEntry::MatchingLine(2)]);
        assert_eq!(content.occurrences[&2].len(), 2);
        assert_eq!(metrics.sink_matches, 2);
        assert_eq!(metrics.occurrences_examined, 3);
        assert_eq!(metrics.line_layout_builds, 1);
    }

    #[test]
    fn temp_backed_content_uses_absolute_ranges_across_searcher_buffers() {
        let prefix_lines = (8 * 1024 * 1024 / 6) + 2;
        let mut bytes = b"quiet\n".repeat(prefix_lines);
        bytes.extend_from_slice(b"hit\n");
        let backing = crate::search_text::SearchText::capture(Cursor::new(&bytes), None).unwrap();
        assert!(backing.is_temp());

        let matcher = matcher("hit", false);
        let mut builder = SearcherBuilder::new();
        builder
            .line_number(true)
            .line_terminator(LineTerminator::crlf());
        let mut searcher = builder.build();
        let plan = GrepSearchPlan::ContentLine(content_spec(false, false));
        let mut sink = PlanSink::new(&matcher, plan, None, Some(Arc::clone(&backing)));
        let reader = backing.open_reader().unwrap();
        searcher.search_reader(&matcher, reader, &mut sink).unwrap();

        let output = sink.into_output("/temp.txt".to_string(), prefix_lines + 2, true);
        let result = output.result.unwrap();
        let line_number = prefix_lines + 1;
        assert_eq!(
            result.content().entries,
            [super::ContentEntry::MatchingLine(line_number)]
        );
        assert_eq!(
            result.content().lines[&line_number].as_str().unwrap(),
            "hit"
        );
    }

    #[test]
    fn occurrence_plan_pages_unicode_matches_without_byte_char_confusion() {
        let (output, _) = search(
            "界hit",
            "前界hit后 界hit尾\n".as_bytes(),
            GrepSearchPlan::ContentOccurrence(content_spec(false, true)),
        );
        let result = output.result.unwrap();
        let occurrences = &result.content().occurrences[&1];
        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].matched_text().unwrap(), "界hit");
        assert_eq!(occurrences[0].line_spans[0].match_char_start, 1);
        assert_eq!(occurrences[0].line_spans[0].match_char_len, 4);
        assert_eq!(occurrences[1].line_spans[0].match_char_start, 7);
        assert_eq!(result.content().entries.len(), 2);
    }

    #[test]
    fn cancellation_at_sink_match_stops_before_match_or_occurrence_work() {
        let cancellation = CancellationToken::new();
        let cancellation_for_hook = cancellation.clone();
        let fired = Arc::new(AtomicBool::new(false));
        let fired_for_hook = Arc::clone(&fired);
        let hook = Arc::new(move |stage| {
            if stage == TestStage::SinkMatch && !fired_for_hook.swap(true, Ordering::AcqRel) {
                cancellation_for_hook.cancel();
            }
        });
        let (_guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(800), cancellation, hook);
        let matcher = matcher("hit", false);
        let mut builder = SearcherBuilder::new();
        builder
            .line_number(true)
            .line_terminator(LineTerminator::crlf());
        let mut searcher = builder.build();
        let mut sink = PlanSink::new(&matcher, GrepSearchPlan::Count, Some(&operation), None);

        let error = searcher
            .search_slice(&matcher, b"hit hit\n", &mut sink)
            .unwrap_err();

        assert!(matches!(
            error,
            GrepSinkError::Stopped(crate::operation::WorkStop::RequestCancelled)
        ));
        assert!(fired.load(Ordering::Acquire));
        let metrics = sink.metrics();
        assert_eq!(metrics.sink_matches, 0);
        assert_eq!(metrics.occurrences_examined, 0);
        assert_eq!(metrics.line_layout_builds, 0);
    }

    #[test]
    fn dense_occurrence_cancellation_stops_at_the_declared_batch_boundary() {
        let cancelled = CancellationToken::new();
        let cancellation_for_hook = cancelled.clone();
        let fired = Arc::new(AtomicBool::new(false));
        let fired_for_hook = Arc::clone(&fired);
        let hook = Arc::new(move |stage| {
            if stage == TestStage::OccurrenceBatch && !fired_for_hook.swap(true, Ordering::AcqRel) {
                cancellation_for_hook.cancel();
            }
        });
        let (_guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(801), cancelled, hook);
        let matcher = matcher("x", false);
        let mut builder = SearcherBuilder::new();
        builder
            .line_number(true)
            .line_terminator(LineTerminator::crlf());
        let mut searcher = builder.build();
        let mut sink = PlanSink::new(&matcher, GrepSearchPlan::Count, Some(&operation), None);
        let error = searcher
            .search_slice(&matcher, "x".repeat(10_000).as_bytes(), &mut sink)
            .unwrap_err();
        assert!(
            matches!(
                error,
                GrepSinkError::Stopped(crate::operation::WorkStop::RequestCancelled)
            ),
            "{error:?}"
        );
        assert!(fired.load(Ordering::Acquire));
        assert_eq!(sink.metrics().occurrences_examined, 256);
    }

    #[test]
    fn one_large_occurrence_reaches_the_declared_byte_cancellation_boundary() {
        let cancelled = CancellationToken::new();
        let cancellation_for_hook = cancelled.clone();
        let hook = Arc::new(move |stage| {
            if stage == TestStage::OccurrenceBatch {
                cancellation_for_hook.cancel();
            }
        });
        let (_guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(802), cancelled, hook);
        let matcher = matcher("x+", false);
        let mut builder = SearcherBuilder::new();
        builder
            .line_number(true)
            .line_terminator(LineTerminator::crlf());
        let mut searcher = builder.build();
        let mut sink = PlanSink::new(&matcher, GrepSearchPlan::Count, Some(&operation), None);
        let error = searcher
            .search_slice(&matcher, "x".repeat(65_536).as_bytes(), &mut sink)
            .unwrap_err();
        assert!(matches!(
            error,
            GrepSinkError::Stopped(crate::operation::WorkStop::RequestCancelled)
        ));
        assert_eq!(sink.metrics().occurrences_examined, 1);
    }
}
