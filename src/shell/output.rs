//! Shell output capture, bounded presentation windows, and terminal status notes.

use crate::budget::{
    JOB_OUTPUT_TOKEN_BUDGET_ENV, RUN_TOKEN_BUDGET_ENV, TokenBudget, estimate_tokens, token_budget,
    tool_token_budget,
};
use crate::model::ToolResponse;
use crate::shell::apply_patch_hint;
use crate::shell::buffer::{BufferedLine, LineRing};
use crate::shell::encoding::{EncodedLine, OutputEncoding, decode_run, run_garble_note};
use crate::shell::normalize::StreamNormalizer;
use std::io::Read;

/// A normalized output stream retained in an eight-megabyte whole-line ring.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CapturedOutput {
    ring: LineRing,
}

impl CapturedOutput {
    pub(crate) fn total_lines(&self) -> u64 {
        self.ring.total_lines()
    }

    pub(crate) fn retained_lines(&self) -> Vec<BufferedLine> {
        self.ring.all()
    }

    pub(crate) fn dropped_lines(&self) -> u64 {
        self.ring.dropped_lines()
    }

    #[cfg(test)]
    fn from_lines(lines: &[(&str, bool)], limit: usize) -> Self {
        let mut ring = LineRing::with_limit(limit);
        for (text, truncated) in lines {
            ring.push(crate::shell::normalize::NormalizedLine {
                bytes: text.as_bytes().to_vec(),
                total_bytes: text.len() as u64,
                terminated: true,
                stream_encoding: None,
                raw_truncated: *truncated,
            });
        }
        Self { ring }
    }
}

/// Reads and normalizes the merged process pipe through EOF without controlling process life.
pub(crate) fn capture_foreground(mut reader: impl Read) -> std::io::Result<CapturedOutput> {
    let mut normalizer = StreamNormalizer::new();
    let mut output = CapturedOutput::default();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let mut lines = Vec::new();
        normalizer.push(&buffer[..read], &mut lines);
        for line in lines {
            output.ring.push(line);
        }
    }
    let mut lines = Vec::new();
    normalizer.finish(&mut lines);
    for line in lines {
        output.ring.push(line);
    }
    Ok(output)
}

/// Rejects an unusably small run budget before a command can cause side effects.
pub(crate) fn validate_run_budget(timeout_ms: u64) -> Result<TokenBudget, String> {
    let budget = run_token_budget()?;
    let maximum = u64::MAX;
    let drop_note = dropped_note(maximum).expect("a positive count always creates a note");
    let ring_loss_terminal = window_terminal(i32::MIN, None, 0, 0, maximum);
    let ring_loss = compose_response(
        Some(&drop_note),
        &[format!("... [{maximum} lines omitted] ...")],
        &ring_loss_terminal,
    );
    let candidates = [
        format!("(Complete: exited {}; no output.)", i32::MIN),
        format!("(Complete: exited {}; {maximum} lines.)", i32::MIN),
        format!(
            "(Partial: exited {}; {maximum} lines shown, but one or more long lines were truncated at 2000 chars. Redirect to a file (command > file 2>&1) and inspect the long line with the read tool's hex view or grep.)",
            i32::MIN
        ),
        format!(
            "(Partial: showing the first 0 and last 0 of {maximum} lines; exited {}. Re-run with output redirected to a file (command > file 2>&1) and page it with the read tool.)",
            i32::MIN
        ),
        format!(
            "(Partial: timed out after {timeout_ms} ms and the process tree was killed; no output captured. Increase timeout_ms or use run_background.)"
        ),
        format!(
            "(Partial: timed out after {timeout_ms} ms and the process tree was killed; {maximum} lines captured. Increase timeout_ms or use run_background.)"
        ),
        format!(
            "(Partial: timed out after {timeout_ms} ms and the process tree was killed; showing the first 0 and last 0 of {maximum} captured lines. Increase timeout_ms or use run_background.)"
        ),
        ring_loss,
    ];
    if candidates
        .iter()
        .all(|candidate| estimate_tokens(candidate) <= budget.value)
    {
        Ok(budget)
    } else {
        Err(budget_too_small_message(budget))
    }
}

pub(crate) fn run_token_budget() -> Result<TokenBudget, String> {
    tool_token_budget(RUN_TOKEN_BUDGET_ENV)
}

pub(crate) fn job_output_token_budget() -> Result<TokenBudget, String> {
    tool_token_budget(JOB_OUTPUT_TOKEN_BUDGET_ENV)
}

pub(crate) fn global_token_budget() -> Result<TokenBudget, String> {
    token_budget().map(|value| TokenBudget {
        value,
        variable: "FASTCTX_TOKEN_BUDGET",
    })
}

pub(crate) fn budget_too_small_message(budget: TokenBudget) -> String {
    format!(
        "{}={} is too small to return the required status note. Increase it and retry.",
        budget.variable, budget.value
    )
}

pub(crate) fn terminal_response(terminal: String, budget: TokenBudget) -> ToolResponse {
    if estimate_tokens(&terminal) <= budget.value {
        ToolResponse::text(terminal)
    } else {
        ToolResponse::error(budget_too_small_message(budget))
    }
}

/// Formats a normal or timed-out foreground result without writing any shell artifacts.
pub(crate) fn format_foreground(
    output: &CapturedOutput,
    command: &str,
    exit_code: i32,
    timeout_ms: Option<u64>,
    encoding: Option<OutputEncoding>,
) -> ToolResponse {
    let budget = match run_token_budget() {
        Ok(budget) => budget,
        Err(error) => return ToolResponse::error(error),
    };
    format_foreground_with_budget(output, command, exit_code, timeout_ms, encoding, budget)
}

fn format_foreground_with_budget(
    output: &CapturedOutput,
    command: &str,
    exit_code: i32,
    timeout_ms: Option<u64>,
    encoding: Option<OutputEncoding>,
    budget: TokenBudget,
) -> ToolResponse {
    let retained = output.retained_lines();
    let encoded = retained
        .iter()
        .map(|line| EncodedLine {
            bytes: &line.bytes,
            total_bytes: line.total_bytes,
            stream_encoding: line.stream_encoding,
            legacy_text: None,
            known_truncated: line.raw_truncated,
        })
        .collect::<Vec<_>>();
    let decoded = decode_run(&encoded, encoding);
    let lines = decoded.lines;
    let total = output.total_lines();
    let dropped = output.dropped_lines();
    let trailing = join_notes(
        decoded.transcoding_note.as_deref(),
        apply_patch_hint::misuse_note(command, exit_code, timeout_ms).as_deref(),
    );

    if dropped == 0 {
        let terminal = full_terminal(exit_code, timeout_ms, total, decoded.had_truncation);
        let leading = run_garble_note(decoded.invalid_sequences);
        let response =
            compose_response_with_tail(leading.as_deref(), &lines, trailing.as_deref(), &terminal);
        if estimate_tokens(&response) <= budget.value {
            return ToolResponse::text(response);
        }
    }

    let window = ForegroundWindow {
        lines: &lines,
        invalid_per_line: &decoded.invalid_sequences_per_line,
        trailing_notes: trailing.as_deref(),
        total,
        dropped,
        exit_code,
        timeout_ms,
    };
    match largest_head_tail_that_fits(&window, budget.value) {
        Some(response) => ToolResponse::text(response),
        None => ToolResponse::error(budget_too_small_message(budget)),
    }
}

fn full_terminal(
    exit_code: i32,
    timeout_ms: Option<u64>,
    total: u64,
    had_truncation: bool,
) -> String {
    match timeout_ms {
        Some(timeout) if total == 0 => format!(
            "(Partial: timed out after {timeout} ms and the process tree was killed; no output captured. Increase timeout_ms or use run_background.)"
        ),
        Some(timeout) => format!(
            "(Partial: timed out after {timeout} ms and the process tree was killed; {total} {} captured. Increase timeout_ms or use run_background.)",
            plural(total, "line", "lines")
        ),
        None if total == 0 => format!("(Complete: exited {exit_code}; no output.)"),
        None if had_truncation => format!(
            "(Partial: exited {exit_code}; {total} {} shown, but one or more long lines were truncated at 2000 chars. Redirect to a file (command > file 2>&1) and inspect the long line with the read tool's hex view or grep.)",
            plural(total, "line", "lines")
        ),
        None => format!(
            "(Complete: exited {exit_code}; {total} {}.)",
            plural(total, "line", "lines")
        ),
    }
}

fn window_terminal(
    exit_code: i32,
    timeout_ms: Option<u64>,
    first: usize,
    last: usize,
    total: u64,
) -> String {
    match timeout_ms {
        None => format!(
            "(Partial: showing the first {first} and last {last} of {total} lines; exited {exit_code}. Re-run with output redirected to a file (command > file 2>&1) and page it with the read tool.)"
        ),
        Some(timeout) => format!(
            "(Partial: timed out after {timeout} ms and the process tree was killed; showing the first {first} and last {last} of {total} captured lines. Increase timeout_ms or use run_background.)"
        ),
    }
}

struct ForegroundWindow<'a> {
    lines: &'a [String],
    invalid_per_line: &'a [u64],
    /// Already-joined notes that sit between the output and the terminal note.
    trailing_notes: Option<&'a str>,
    total: u64,
    dropped: u64,
    exit_code: i32,
    timeout_ms: Option<u64>,
}

#[derive(Clone, Copy)]
struct WindowBounds {
    first: usize,
    last: usize,
}

fn largest_head_tail_that_fits(window: &ForegroundWindow<'_>, budget: usize) -> Option<String> {
    let base = window_candidate(window, WindowBounds { first: 0, last: 0 });
    let base_tokens = estimate_tokens(&base);
    if base_tokens > budget {
        return None;
    }

    let head_target = budget.saturating_sub(base_tokens) / 10;
    let first = largest_prefix_within(window.lines, head_target);
    let remaining = window.lines.len().saturating_sub(first);

    let mut low = 0;
    let mut high = remaining;
    let mut best = base;
    while low <= high {
        let last = low + (high - low) / 2;
        let candidate = window_candidate(window, WindowBounds { first, last });
        if estimate_tokens(&candidate) <= budget {
            best = candidate;
            low = last.saturating_add(1);
        } else if last == 0 {
            break;
        } else {
            high = last - 1;
        }
    }
    Some(best)
}

fn largest_prefix_within(lines: &[String], token_target: usize) -> usize {
    if token_target == 0 {
        return 0;
    }
    let mut low = 0;
    let mut high = lines.len();
    let mut best = 0;
    while low <= high {
        let middle = low + (high - low) / 2;
        let tokens = estimate_tokens(&lines[..middle].join("\n"));
        if tokens <= token_target {
            best = middle;
            low = middle.saturating_add(1);
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    best
}

fn window_candidate(window: &ForegroundWindow<'_>, bounds: WindowBounds) -> String {
    let WindowBounds { first, last } = bounds;
    let mut body = Vec::with_capacity(first.saturating_add(last).saturating_add(1));
    body.extend(window.lines.iter().take(first).cloned());
    let omitted = window
        .total
        .saturating_sub(first.saturating_add(last) as u64);
    if omitted > 0 {
        body.push(format!("... [{omitted} lines omitted] ..."));
    }
    if last > 0 {
        body.extend(window.lines[window.lines.len() - last..].iter().cloned());
    }
    let invalid = window
        .invalid_per_line
        .iter()
        .take(first)
        .chain(window.invalid_per_line.iter().rev().take(last))
        .copied()
        .fold(0_u64, u64::saturating_add);
    let garble_note = run_garble_note(invalid);
    let drop_note = dropped_note(window.dropped);
    let leading = match (drop_note.as_deref(), garble_note.as_deref()) {
        (Some(drop_note), Some(garble_note)) => Some(format!("{drop_note}\n\n{garble_note}")),
        (Some(drop_note), None) => Some(drop_note.to_string()),
        (None, Some(garble_note)) => Some(garble_note.to_string()),
        (None, None) => None,
    };
    let terminal = window_terminal(
        window.exit_code,
        window.timeout_ms,
        first,
        last,
        window.total,
    );
    compose_response_with_tail(leading.as_deref(), &body, window.trailing_notes, &terminal)
}

/// Joins two optional notes with the same separator `compose_response_with_tail` uses between them.
fn join_notes(first: Option<&str>, second: Option<&str>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n{second}")),
        (Some(only), None) | (None, Some(only)) => Some(only.to_string()),
        (None, None) => None,
    }
}

pub(crate) fn dropped_note(dropped: u64) -> Option<String> {
    (dropped > 0).then(|| {
        format!(
            "(Note: {dropped} earlier {} {} dropped from the buffer and cannot be retrieved; redirect the command to a file for the full log.)",
            plural(dropped, "line", "lines"),
            if dropped == 1 { "was" } else { "were" }
        )
    })
}

pub(crate) fn compose_response(
    leading_note: Option<&str>,
    lines: &[String],
    terminal: &str,
) -> String {
    compose_response_with_tail(leading_note, lines, None, terminal)
}

pub(crate) fn compose_response_with_tail(
    leading_note: Option<&str>,
    lines: &[String],
    trailing_note: Option<&str>,
    terminal: &str,
) -> String {
    let mut notes = Vec::with_capacity(2);
    if let Some(note) = trailing_note {
        notes.push(note.to_string());
    }
    notes.push(terminal.to_string());
    let body = if lines.is_empty() {
        notes.join("\n")
    } else {
        format!("{}\n\n{}", lines.join("\n"), notes.join("\n"))
    };
    match leading_note {
        Some(note) => format!("{note}\n\n{body}"),
        None => body,
    }
}

#[cfg(test)]
pub(crate) fn compose_lines(lines: &[String], note: &str) -> String {
    let body = lines.join("\n");
    if lines.is_empty() {
        note.to_string()
    } else {
        format!("{body}\n\n{note}")
    }
}

pub(crate) fn plural<'a>(count: u64, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

#[cfg(test)]
mod tests {
    use super::{
        CapturedOutput, capture_foreground, compose_lines, dropped_note,
        format_foreground_with_budget, full_terminal, window_terminal,
    };
    use crate::budget::TokenBudget;

    fn budget(value: usize) -> TokenBudget {
        TokenBudget {
            value,
            variable: "FASTCTX_RUN_TOKEN_BUDGET",
        }
    }

    #[test]
    fn complete_timeout_and_long_line_status_notes_match_the_contract() {
        assert_eq!(
            format_foreground_with_budget(
                &CapturedOutput::default(),
                "true",
                0,
                None,
                None,
                budget(8_500),
            ),
            crate::ToolResponse::text("(Complete: exited 0; no output.)")
        );
        let one = CapturedOutput::from_lines(&[("one", false)], usize::MAX);
        assert_eq!(
            format_foreground_with_budget(&one, "true", 42, None, None, budget(8_500)),
            crate::ToolResponse::text("one\n\n(Complete: exited 42; 1 line.)")
        );
        let long = capture_foreground(std::io::Cursor::new(vec![b'x'; 3_000])).unwrap();
        let long_line = format!(
            "{}... [line truncated: 3000 bytes total]",
            "x".repeat(2_000)
        );
        assert_eq!(
            format_foreground_with_budget(&long, "true", 0, None, None, budget(8_500)),
            crate::ToolResponse::text(format!(
                "{long_line}\n\n(Partial: exited 0; 1 line shown, but one or more long lines were truncated at 2000 chars. Redirect to a file (command > file 2>&1) and inspect the long line with the read tool's hex view or grep.)"
            ))
        );
        assert_eq!(
            format_foreground_with_budget(
                &CapturedOutput::default(),
                "true",
                137,
                Some(1),
                None,
                budget(8_500),
            ),
            crate::ToolResponse::text(
                "(Partial: timed out after 1 ms and the process tree was killed; no output captured. Increase timeout_ms or use run_background.)"
            )
        );
    }

    #[test]
    fn budget_truncation_keeps_a_head_and_a_larger_tail_without_spilling() {
        let owned = (1..=200)
            .map(|index| (format!("line-{index:03} payload payload payload"), false))
            .collect::<Vec<_>>();
        let borrowed = owned
            .iter()
            .map(|(line, truncated)| (line.as_str(), *truncated))
            .collect::<Vec<_>>();
        let output = CapturedOutput::from_lines(&borrowed, usize::MAX);
        let response = format_foreground_with_budget(&output, "true", 0, None, None, budget(160));
        let text = match &response.content[0] {
            crate::ToolContent::Text(text) => text,
            crate::ToolContent::Image { .. } => panic!("expected text"),
        };
        assert!(text.contains("line-001"), "{text}");
        assert!(text.contains("line-200"), "{text}");
        assert!(text.contains("... ["), "{text}");
        assert!(text.ends_with(
            "Re-run with output redirected to a file (command > file 2>&1) and page it with the read tool.)"
        ));
        assert!(crate::budget::estimate_tokens(text) <= 160);
    }

    #[test]
    fn ring_loss_is_reported_at_the_page_front_and_in_the_partial_terminal() {
        let per_line = std::mem::size_of::<crate::shell::buffer::BufferedLine>() + 4;
        let output = CapturedOutput::from_lines(&[("one", false), ("two", false)], per_line);
        let response = format_foreground_with_budget(&output, "true", 0, None, None, budget(8_500));
        let text = match &response.content[0] {
            crate::ToolContent::Text(text) => text,
            crate::ToolContent::Image { .. } => panic!("expected text"),
        };
        assert!(text.starts_with(
            "(Note: 1 earlier line was dropped from the buffer and cannot be retrieved; redirect the command to a file for the full log.)"
        ));
        assert!(text.contains("of 2 lines; exited 0."));
    }

    #[test]
    fn empty_output_line_and_drop_note_grammar_are_exact() {
        assert_eq!(compose_lines(&[String::new()], "(status)"), "\n\n(status)");
        assert_eq!(
            dropped_note(2).unwrap(),
            "(Note: 2 earlier lines were dropped from the buffer and cannot be retrieved; redirect the command to a file for the full log.)"
        );
    }

    #[test]
    fn foreground_terminal_matrix_is_frozen_independently_of_window_selection() {
        assert_eq!(
            full_terminal(7, Some(50), 2, false),
            "(Partial: timed out after 50 ms and the process tree was killed; 2 lines captured. Increase timeout_ms or use run_background.)"
        );
        assert_eq!(
            window_terminal(42, None, 2, 9, 20),
            "(Partial: showing the first 2 and last 9 of 20 lines; exited 42. Re-run with output redirected to a file (command > file 2>&1) and page it with the read tool.)"
        );
        assert_eq!(
            window_terminal(137, Some(500), 1, 8, 20),
            "(Partial: timed out after 500 ms and the process tree was killed; showing the first 1 and last 8 of 20 captured lines. Increase timeout_ms or use run_background.)"
        );
    }
}
