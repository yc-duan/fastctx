//! Shell output capture, bounded presentation windows, and terminal status notes.

use crate::budget::{
    JOB_OUTPUT_TOKEN_BUDGET_ENV, RUN_TOKEN_BUDGET_ENV, TokenBudget, estimate_tokens, token_budget,
    tool_token_budget, tool_token_budget_for_required,
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
            "(Partial: exited {}; {maximum} lines shown, but one or more long lines were truncated at 2000 chars.)",
            i32::MIN
        ),
        format!(
            "(Partial: showing the first 0 and last 0 of {maximum} lines; exited {}.)",
            i32::MIN
        ),
        format!(
            "(Killed: timed out after {timeout_ms} ms and the process tree was killed; no output captured. Re-run with a larger timeout_ms or use run_background.)"
        ),
        format!(
            "(Killed: timed out after {timeout_ms} ms and the process tree was killed; {maximum} lines captured. Re-run with a larger timeout_ms or use run_background.)"
        ),
        format!(
            "(Killed: timed out after {timeout_ms} ms and the process tree was killed; showing the first 0 and last 0 of {maximum} captured lines. Re-run with a larger timeout_ms or use run_background.)"
        ),
        ring_loss,
    ];
    let required = candidates
        .iter()
        .map(|candidate| estimate_tokens(candidate))
        .max()
        .unwrap_or(0);
    let budget = tool_token_budget_for_required(RUN_TOKEN_BUDGET_ENV, required)?;
    if required <= budget.value {
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
    let required = estimate_tokens(&terminal);
    let budget = tool_token_budget_for_required(budget.variable, required).unwrap_or(budget);
    if required <= budget.value {
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
            "(Killed: timed out after {timeout} ms and the process tree was killed; no output captured. Re-run with a larger timeout_ms or use run_background.)"
        ),
        Some(timeout) => format!(
            "(Killed: timed out after {timeout} ms and the process tree was killed; {total} {} captured. Re-run with a larger timeout_ms or use run_background.)",
            plural(total, "line", "lines")
        ),
        None if total == 0 => format!("(Complete: exited {exit_code}; no output.)"),
        None if had_truncation => format!(
            "(Partial: exited {exit_code}; {total} {} shown, but one or more long lines were truncated at 2000 chars.)",
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
            "(Partial: showing the first {first} and last {last} of {total} lines; exited {exit_code}.)"
        ),
        Some(timeout) => format!(
            "(Killed: timed out after {timeout} ms and the process tree was killed; showing the first {first} and last {last} of {total} captured lines. Re-run with a larger timeout_ms or use run_background.)"
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

/// Reports ring-buffer loss as a bare fact: what to do about it is the caller's
/// call, and the only remedy would be re-running a command whose sheer output
/// volume makes heavy side effects likely (2026-07-25).
pub(crate) fn dropped_note(dropped: u64) -> Option<String> {
    (dropped > 0).then(|| {
        format!(
            "(Note: {dropped} earlier {} {} dropped from the buffer and cannot be retrieved.)",
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

pub(crate) fn plural<'a>(count: u64, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}
