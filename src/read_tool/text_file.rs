//! Strict incremental decoding, line collection, and budget closure for text reads.

use super::{MAX_LINE_CHARS, TOTAL_COUNT_SIZE_LIMIT, UNBOUNDED_LINE_LIMIT, binary_error};
use crate::budget::{LineTokenCounter, TokenBudget, estimate_tokens};
use crate::encoding::{EncodingDecision, StreamDecodeFailure, validate_file_encoding};
use crate::head_note::{CoverageTotal, CoveredRange, HeadMetric, HeadNote};
use crate::model::ToolResponse;
use crate::paths::io_error_message;
use std::fs;
use std::path::Path;

pub(super) fn read_text_file(
    path: &Path,
    path_display: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    explicit_encoding: Option<&str>,
    binary_type: Option<&str>,
    budget: TokenBudget,
) -> ToolResponse {
    let offset = offset.unwrap_or(1);
    let limit = limit.unwrap_or(UNBOUNDED_LINE_LIMIT);
    if offset == 0 {
        return ToolResponse::error("Invalid offset value: 0. Expected an integer >= 1.");
    }
    if limit == 0 {
        return ToolResponse::error("Invalid limit value: 0. Expected an integer >= 1.");
    }
    let file_size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) => return ToolResponse::error(io_error_message(path, &error)),
    };
    let validated = match validate_file_encoding(path, explicit_encoding) {
        Ok(EncodingDecision::Text(validated)) => validated,
        Ok(EncodingDecision::Binary) => return binary_error(path_display, binary_type),
        Ok(EncodingDecision::Rejected(rejection)) => {
            return ToolResponse::error(rejection.message(path_display));
        }
        Err(error) => return ToolResponse::error(io_error_message(path, &error)),
    };
    if validated.total_lines == 0 {
        return HeadNote::new(path_display, HeadMetric::count(0, "line", "lines"))
            .into_text_response("");
    }
    let transcoding_fact = validated.transcoding_fact();

    let total_is_known = file_size <= TOTAL_COUNT_SIZE_LIMIT;
    let mut collector = LineCollector::new(offset, limit, budget.value, total_is_known);
    let exhausted = match validated.stream_text(path, |chunk| collector.push(chunk)) {
        Ok(exhausted) => exhausted,
        Err(StreamDecodeFailure::Io(error)) => {
            return ToolResponse::error(io_error_message(path, &error));
        }
        Err(StreamDecodeFailure::Malformed) => {
            return ToolResponse::error(validated.malformed_rejection().message(path_display));
        }
    };
    if exhausted {
        collector.finish_eof();
    }
    collector.into_response(path_display, file_size, transcoding_fact, budget)
}

struct LineCollector {
    offset: usize,
    request_end: usize,
    budget: usize,
    total_is_known: bool,
    line_number: usize,
    current_prefix: String,
    current_chars: usize,
    current_ends_with_cr: bool,
    decoded_any: bool,
    last_was_newline: bool,
    rendered: Vec<String>,
    body_tokens: LineTokenCounter,
    storage_saturated: bool,
    total_lines: usize,
    stopped_early: bool,
}

impl LineCollector {
    fn new(offset: usize, limit: usize, budget: usize, total_is_known: bool) -> Self {
        Self {
            offset,
            request_end: offset.saturating_add(limit.saturating_sub(1)),
            budget,
            total_is_known,
            line_number: 1,
            current_prefix: String::new(),
            current_chars: 0,
            current_ends_with_cr: false,
            decoded_any: false,
            last_was_newline: false,
            rendered: Vec::new(),
            body_tokens: LineTokenCounter::default(),
            storage_saturated: false,
            total_lines: 0,
            stopped_early: false,
        }
    }

    fn push(&mut self, text: &str) -> bool {
        for ch in text.chars() {
            self.decoded_any = true;
            if self.line_number > self.request_end && !self.total_is_known {
                self.total_lines = self.line_number;
                self.stopped_early = true;
                return false;
            }
            if ch == '\n' {
                self.last_was_newline = true;
                if !self.finish_line() {
                    self.stopped_early = true;
                    return false;
                }
                continue;
            }
            self.last_was_newline = false;
            self.current_chars = self.current_chars.saturating_add(1);
            self.current_ends_with_cr = ch == '\r';
            if self.should_capture_current() && self.current_chars <= MAX_LINE_CHARS {
                self.current_prefix.push(ch);
            }
        }
        true
    }

    fn should_capture_current(&self) -> bool {
        !self.storage_saturated
            && self.line_number >= self.offset
            && self.line_number <= self.request_end
    }

    fn finish_line(&mut self) -> bool {
        self.total_lines = self.line_number;
        if self.line_number > self.request_end {
            self.reset_line();
            return self.total_is_known;
        }
        if self.line_number >= self.offset && !self.storage_saturated {
            if self.current_ends_with_cr && self.current_chars <= MAX_LINE_CHARS {
                self.current_prefix.pop();
            }
            let total_chars = self
                .current_chars
                .saturating_sub(usize::from(self.current_ends_with_cr));
            let content = if total_chars <= MAX_LINE_CHARS {
                std::mem::take(&mut self.current_prefix)
            } else {
                format!(
                    "{}... [line truncated: {total_chars} chars total]",
                    self.current_prefix
                )
            };
            let rendered = format!("{}\t{content}", self.line_number);
            let body_tokens = self.body_tokens.push(&rendered);
            self.rendered.push(rendered);
            if body_tokens > self.budget {
                self.storage_saturated = true;
                if !self.total_is_known {
                    self.reset_line();
                    return false;
                }
            }
        }
        self.reset_line();
        true
    }

    fn reset_line(&mut self) {
        self.line_number = self.line_number.saturating_add(1);
        self.current_prefix.clear();
        self.current_chars = 0;
        self.current_ends_with_cr = false;
    }

    fn finish_eof(&mut self) {
        if self.last_was_newline || self.decoded_any || self.current_chars > 0 {
            let _ = self.finish_line();
        }
        self.total_is_known = true;
    }

    fn into_response(
        mut self,
        path_display: &str,
        file_size: u64,
        transcoding_fact: Option<String>,
        budget: TokenBudget,
    ) -> ToolResponse {
        if self.total_lines < self.offset {
            let note =
                HeadNote::new(path_display, HeadMetric::count(0, "line", "lines")).fact(format!(
                    "file has {} {}",
                    self.total_lines,
                    if self.total_lines == 1 {
                        "line"
                    } else {
                        "lines"
                    }
                ));
            return note.into_text_response("");
        }

        loop {
            let shown = self.rendered.len();
            if shown == 0 && (self.stopped_early || self.storage_saturated) {
                return ToolResponse::error(format!(
                    "{}={} is too small to return the response head note and one content line. That budget is fixed for this session; retrying cannot raise it.",
                    budget.variable, budget.value
                ));
            }
            let last = self.offset.saturating_add(shown.saturating_sub(1));
            let total = if self.total_is_known {
                CoverageTotal::Exact(self.total_lines)
            } else {
                CoverageTotal::FileBytes(file_size)
            };
            let mut note = HeadNote::new(
                path_display,
                HeadMetric::Coverage {
                    unit: "lines",
                    ranges: vec![CoveredRange::new(self.offset, last)],
                    total,
                },
            );
            if let Some(fact) = transcoding_fact.as_deref() {
                note = note.fact(fact);
            }
            let body = self.rendered.join("\n");
            let output = note.render_with_body(&body);
            if estimate_tokens(&output) <= budget.value {
                return ToolResponse::text(output);
            }
            if self.rendered.pop().is_none() {
                return ToolResponse::error(format!(
                    "{}={} is too small to return the response head note and one content line. That budget is fixed for this session; retrying cannot raise it.",
                    budget.variable, budget.value
                ));
            }
            self.storage_saturated = true;
        }
    }
}
