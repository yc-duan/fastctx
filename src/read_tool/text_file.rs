//! Strict incremental decoding, line collection, and budget closure for text reads.

use super::{DEFAULT_LINE_LIMIT, MAX_LINE_CHARS, TOTAL_COUNT_SIZE_LIMIT, binary_error};
use crate::budget::{LineTokenCounter, TokenBudget, assemble_text, estimate_tokens};
use crate::encoding::{EncodingDecision, StreamDecodeFailure, validate_file_encoding};
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
    let limit = limit.unwrap_or(DEFAULT_LINE_LIMIT);
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
        return ToolResponse::text("Warning: the file exists but is empty.");
    }
    let transcoding_note = validated.transcoding_note();

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
    collector.into_response(transcoding_note, budget)
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
    }

    fn into_response(
        mut self,
        transcoding_note: Option<String>,
        budget: TokenBudget,
    ) -> ToolResponse {
        if self.total_lines < self.offset {
            let noun = if self.total_lines == 1 {
                "line"
            } else {
                "lines"
            };
            return ToolResponse::text(format!(
                "Warning: the file has only {} {noun}, but offset={} was requested.",
                self.total_lines, self.offset,
            ));
        }

        loop {
            let shown = self.rendered.len();
            if shown == 0 && (self.stopped_early || self.storage_saturated) {
                return ToolResponse::error(format!(
                    "{}={} is too small to return the required continuation note. Increase it and retry.",
                    budget.variable, budget.value
                ));
            }
            let last = self.offset.saturating_add(shown.saturating_sub(1));
            let truncated = self.stopped_early
                || self.storage_saturated
                || (shown > 0 && last < self.total_lines);
            let notes = read_notes(
                transcoding_note.as_deref(),
                truncated,
                self.offset,
                shown,
                self.total_lines,
                self.total_is_known,
            );
            let output = assemble_text(&self.rendered, &notes);
            if estimate_tokens(&output) <= budget.value {
                return ToolResponse::text(output);
            }
            if self.rendered.pop().is_none() {
                return ToolResponse::error(format!(
                    "{}={} is too small to return the required continuation note. Increase it and retry.",
                    budget.variable, budget.value
                ));
            }
            self.storage_saturated = true;
        }
    }
}

fn read_notes(
    transcoding_note: Option<&str>,
    truncated: bool,
    offset: usize,
    shown: usize,
    total: usize,
    total_is_known: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(note) = transcoding_note {
        notes.push(note.to_string());
    }
    if shown > 0 {
        let last = offset + shown - 1;
        let span = line_span(offset, last);
        if truncated {
            if total_is_known {
                notes.push(format!(
                    "(Partial: {span} of {total} shown. Continue with offset={}.)",
                    last + 1
                ));
            } else {
                notes.push(format!(
                    "(Partial: {span} shown. Continue with offset={}.)",
                    last + 1
                ));
            }
        } else {
            notes.push(format!(
                "(Complete: reached end of file; {span} of {total} shown.)"
            ));
        }
    }
    notes
}

fn line_span(first: usize, last: usize) -> String {
    if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}-{last}")
    }
}

#[cfg(test)]
mod tests {
    use super::read_text_file;
    use crate::ToolContent;
    use crate::budget::TokenBudget;
    use std::fs;

    #[test]
    fn budget_stops_at_a_unicode_line_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("budget.txt");
        fs::write(
            &path,
            ["界".repeat(20), "文".repeat(20), "字".repeat(20)].join("\n"),
        )
        .unwrap();
        let response = read_text_file(
            &path,
            "budget.txt",
            None,
            None,
            None,
            None,
            TokenBudget {
                value: 50,
                variable: "FASTCTX_TOKEN_BUDGET",
            },
        );
        assert!(!response.is_error);
        let ToolContent::Text(output) = &response.content[0] else {
            panic!("expected text");
        };
        assert_eq!(
            output,
            &format!(
                "1\t{}\n\n(Partial: line 1 of 3 shown. Continue with offset=2.)",
                "界".repeat(20)
            )
        );
        assert!(output.is_char_boundary(output.len()));
    }

    #[test]
    fn budget_too_small_for_one_line_returns_an_actionable_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("tiny-budget.txt");
        fs::write(&path, "content\nmore").unwrap();
        let response = read_text_file(
            &path,
            "tiny-budget.txt",
            None,
            None,
            None,
            None,
            TokenBudget {
                value: 1,
                variable: "FASTCTX_TOKEN_BUDGET",
            },
        );
        assert!(response.is_error);
        assert_eq!(
            response.content,
            vec![ToolContent::Text(
                "FASTCTX_TOKEN_BUDGET=1 is too small to return the required continuation note. Increase it and retry."
                    .to_string()
            )]
        );
    }
}
