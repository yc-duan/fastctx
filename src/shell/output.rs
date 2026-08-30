//! Shell output capture, bounded presentation windows, and leading result facts.

use crate::budget::{
    JOB_OUTPUT_TOKEN_BUDGET_ENV, RUN_TOKEN_BUDGET_ENV, TokenBudget, estimate_tokens, token_budget,
    tool_token_budget, tool_token_budget_for_required,
};
use crate::head_note::{CoverageTotal, CoveredRange, HeadMetric, HeadNote};
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
    let ring_loss = run_head(i32::MIN, None, maximum, &[], true, maximum, &[])
        .render_with_body(&format!("... [{maximum} lines omitted] ..."));
    let candidates = [
        run_head(i32::MIN, None, 0, &[], false, 0, &[]).render(),
        run_head(
            i32::MIN,
            None,
            maximum,
            &[CoveredRange::new(1, usize::MAX)],
            true,
            0,
            &[],
        )
        .render(),
        run_head(i32::MIN, Some(timeout_ms), 0, &[], false, 0, &[]).render(),
        run_head(i32::MIN, Some(timeout_ms), maximum, &[], false, 0, &[]).render(),
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
        "{}={} is too small to return the required response head note. Increase it and retry.",
        budget.variable, budget.value
    )
}

fn run_head(
    exit_code: i32,
    timeout_ms: Option<u64>,
    total: u64,
    ranges: &[CoveredRange],
    had_truncation: bool,
    dropped: u64,
    extra_facts: &[String],
) -> HeadNote {
    let total_usize = usize::try_from(total).unwrap_or(usize::MAX);
    let metric = if ranges.is_empty() || (ranges.len() == 1 && ranges[0].is(1, total_usize)) {
        HeadMetric::count(total_usize, "line", "lines")
    } else {
        HeadMetric::Coverage {
            unit: "lines",
            ranges: ranges.to_vec(),
            total: CoverageTotal::Exact(total_usize),
        }
    };
    let mut head = HeadNote::new("run", metric);
    head = match timeout_ms {
        Some(timeout) => head.fact(format!("timed out after {timeout} ms, process tree killed")),
        None => head.fact(format!("exited {exit_code}")),
    };
    if ranges.is_empty() && total > 0 {
        head = head.fact("captured output omitted from the body at the FastCtx budget");
    }
    if had_truncation {
        head = head.fact("one or more shown lines truncated at 2000 chars");
    }
    if dropped > 0 {
        head = head.fact(format!(
            "{dropped} earlier {} dropped from the in-memory buffer and cannot be retrieved",
            plural(dropped, "line", "lines")
        ));
    }
    for fact in extra_facts {
        head = head.fact(fact);
    }
    head
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
    let mut facts = Vec::new();
    facts.extend(decoded.transcoding_note);
    facts.extend(apply_patch_hint::misuse_note(
        command, exit_code, timeout_ms,
    ));

    if dropped == 0 {
        facts.extend(run_garble_note(decoded.invalid_sequences));
        let ranges = (!lines.is_empty()).then(|| CoveredRange::new(1, lines.len()));
        let response = run_head(
            exit_code,
            timeout_ms,
            total,
            ranges.as_slice(),
            decoded.had_truncation,
            0,
            &facts,
        )
        .render_with_body(&lines.join("\n"));
        if estimate_tokens(&response) <= budget.value {
            return ToolResponse::text(response);
        }
    }

    let window = ForegroundWindow {
        lines: &lines,
        invalid_per_line: &decoded.invalid_sequences_per_line,
        truncated_per_line: &decoded.truncated_per_line,
        facts: &facts,
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

struct ForegroundWindow<'a> {
    lines: &'a [String],
    invalid_per_line: &'a [u64],
    truncated_per_line: &'a [bool],
    facts: &'a [String],
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
    let had_truncation = window
        .truncated_per_line
        .iter()
        .take(first)
        .chain(window.truncated_per_line.iter().rev().take(last))
        .any(|truncated| *truncated);
    let mut facts = window.facts.to_vec();
    facts.extend(run_garble_note(invalid));
    let retained_first = window.dropped.saturating_add(1);
    let mut ranges = Vec::with_capacity(2);
    if first > 0 {
        let first_end = retained_first
            .saturating_add(first as u64)
            .saturating_sub(1);
        ranges.push(CoveredRange::new(
            usize::try_from(retained_first).unwrap_or(usize::MAX),
            usize::try_from(first_end).unwrap_or(usize::MAX),
        ));
    }
    if last > 0 {
        let tail_first = window.total.saturating_sub(last as u64).saturating_add(1);
        ranges.push(CoveredRange::new(
            usize::try_from(tail_first).unwrap_or(usize::MAX),
            usize::try_from(window.total).unwrap_or(usize::MAX),
        ));
    }
    run_head(
        window.exit_code,
        window.timeout_ms,
        window.total,
        &ranges,
        had_truncation,
        window.dropped,
        &facts,
    )
    .render_with_body(&body.join("\n"))
}

pub(crate) fn plural<'a>(count: u64, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}
