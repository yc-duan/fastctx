//! Exact o200k_base token accounting and text response assembly.

use crate::operation::{WorkCheckpoint, WorkStop};
use crate::{ToolContent, ToolResponse};
use std::fmt;
use std::sync::Arc;

/// Default output budget with 15% headroom below the Codex host's approximate 10k-token limit.
pub const DEFAULT_TOKEN_BUDGET: usize = 8_500;
/// Environment variable for the read-specific budget.
pub const READ_TOKEN_BUDGET_ENV: &str = "FASTCTX_READ_TOKEN_BUDGET";
/// Environment variable for the grep-specific budget.
pub const GREP_TOKEN_BUDGET_ENV: &str = "FASTCTX_GREP_TOKEN_BUDGET";
/// Environment variable for the glob-specific budget.
pub const GLOB_TOKEN_BUDGET_ENV: &str = "FASTCTX_GLOB_TOKEN_BUDGET";
/// Per-tool budget for foreground shell output.
pub const RUN_TOKEN_BUDGET_ENV: &str = "FASTCTX_RUN_TOKEN_BUDGET";
/// Per-tool budget for background job output polling.
pub const JOB_OUTPUT_TOKEN_BUDGET_ENV: &str = "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET";

/// Effective budget for one tool and the variable that supplied it, used for precise errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenBudget {
    /// Effective token ceiling.
    pub value: usize,
    /// Environment variable supplying the value; inherited budgets point to the global variable.
    pub variable: &'static str,
}

/// Reads the global text budget, rejecting invalid configuration instead of silently falling back.
pub fn token_budget() -> Result<usize, String> {
    match std::env::var("FASTCTX_TOKEN_BUDGET") {
        Ok(value) => parse_token_budget("FASTCTX_TOKEN_BUDGET", &value),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_TOKEN_BUDGET),
        Err(std::env::VarError::NotUnicode(_)) => Err(
            "Invalid FASTCTX_TOKEN_BUDGET value: expected a UTF-8 positive integer.".to_string(),
        ),
    }
}

/// Reads a tool budget; omission inherits the global value and explicit values may not exceed it.
pub fn tool_token_budget(variable: &'static str) -> Result<TokenBudget, String> {
    let global = token_budget()?;
    match std::env::var(variable) {
        Ok(value) => {
            let value = parse_token_budget(variable, &value)?;
            if value > global {
                return Err(format!(
                    "{variable}={value} exceeds FASTCTX_TOKEN_BUDGET={global}. Increase the global budget or lower the per-tool budget."
                ));
            }
            Ok(TokenBudget { value, variable })
        }
        Err(std::env::VarError::NotPresent) => Ok(TokenBudget {
            value: global,
            variable: "FASTCTX_TOKEN_BUDGET",
        }),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!(
            "Invalid {variable} value: expected a UTF-8 positive integer."
        )),
    }
}

/// Returns a safe ceiling for formatting a budget-configuration error before
/// the requested tool budget itself can be trusted.
pub(crate) fn error_budget_hint(variable: &'static str) -> usize {
    let Ok(global) = token_budget() else {
        return DEFAULT_TOKEN_BUDGET;
    };
    match std::env::var(variable) {
        Ok(value) => parse_token_budget(variable, &value)
            .ok()
            .filter(|value| *value <= global)
            .unwrap_or(global),
        Err(_) => global,
    }
}

fn parse_token_budget(variable: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            format!("Invalid {variable} value \"{value}\": expected a positive integer.")
        })
}

/// Counts text with the same o200k_base tokenizer used by the Codex host.
pub fn estimate_tokens(text: &str) -> usize {
    bpe_openai::o200k_base().count(text)
}

/// Diagnostic family used to select the contractually ordered tiny-budget fallbacks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ErrorClass {
    Budget,
    Cancelled,
    Other,
}

/// Fits every grep/glob error response to its effective o200k budget without
/// changing ordinary diagnostics that already fit.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ErrorBudgetAdapter<'a> {
    budget: usize,
    variable: &'a str,
}

impl<'a> ErrorBudgetAdapter<'a> {
    pub(crate) fn new(budget: usize, variable: &'a str) -> Self {
        Self { budget, variable }
    }

    /// Builds one text error using the exact fallback order from the search contract.
    pub(crate) fn error(self, class: ErrorClass, message: impl Into<String>) -> ToolResponse {
        let message = message.into();
        if estimate_tokens(&message) <= self.budget {
            return ToolResponse::error(message);
        }

        let fallbacks = match class {
            ErrorClass::Budget => vec![
                format!("Increase {}.", self.variable),
                "Budget too small.".to_string(),
                "Budget.".to_string(),
            ],
            ErrorClass::Cancelled => {
                vec!["Request cancelled.".to_string(), "Cancelled.".to_string()]
            }
            ErrorClass::Other => vec!["Error; increase budget.".to_string(), "Error.".to_string()],
        };
        for fallback in fallbacks {
            if estimate_tokens(&fallback) <= self.budget {
                return ToolResponse::error(fallback);
            }
        }
        ToolResponse::error(String::new())
    }

    /// Applies the adapter to an existing text error and leaves successes untouched.
    pub(crate) fn adapt(self, response: ToolResponse) -> ToolResponse {
        if !response.is_error {
            return response;
        }
        let [ToolContent::Text(message)] = response.content.as_slice() else {
            return response;
        };
        let class = classify_error(message);
        self.error(class, message.clone())
    }
}

fn classify_error(message: &str) -> ErrorClass {
    if message == "Request cancelled." || message == "Cancelled." {
        ErrorClass::Cancelled
    } else {
        let lower = message.to_ascii_lowercase();
        if message.contains("TOKEN_BUDGET")
            || lower.contains("budget too small")
            || lower.contains("too small to return")
        {
            ErrorClass::Budget
        } else {
            ErrorClass::Other
        }
    }
}

/// Exact incremental state at one rendered-prefix boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TokenCheckpoint {
    committed_tokens: usize,
    unresolved_tail: Arc<str>,
}

/// Failures produced while maintaining exact tokenizer checkpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenCountError {
    /// Request cancellation or speculative epoch retirement.
    Stopped(WorkStop),
    /// The platform token counter could not represent the exact result.
    Overflow,
    /// The locked o200k tokenizer unexpectedly enabled normalization.
    UnsupportedNormalization,
}

impl fmt::Display for TokenCountError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped(WorkStop::RequestCancelled) => formatter.write_str("Request cancelled."),
            Self::Stopped(WorkStop::EpochRetired) => {
                formatter.write_str("The render generation was retired.")
            }
            Self::Overflow => {
                formatter.write_str("The exact token count overflowed this platform.")
            }
            Self::UnsupportedNormalization => formatter.write_str(
                "The locked o200k tokenizer unexpectedly changed its normalization contract.",
            ),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ExactPrefixMetrics {
    pub(crate) prefix_appends: usize,
    pub(crate) suffix_probes: usize,
    pub(crate) full_tokenizer_calls: usize,
}

/// Exact o200k counter that permanently commits closed pre-token pieces and
/// retains only the final piece that a later append can still change.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExactPrefixCounter {
    committed_tokens: usize,
    unresolved_tail: String,
    #[cfg(test)]
    metrics: ExactPrefixMetrics,
}

impl ExactPrefixCounter {
    /// Restores a counter at an immutable prefix checkpoint.
    pub(crate) fn from_checkpoint(checkpoint: &TokenCheckpoint) -> Self {
        Self {
            committed_tokens: checkpoint.committed_tokens,
            unresolved_tail: checkpoint.unresolved_tail.to_string(),
            #[cfg(test)]
            metrics: ExactPrefixMetrics::default(),
        }
    }

    /// Appends one exact output fragment.
    pub(crate) fn append(
        &mut self,
        fragment: &str,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<(), TokenCountError> {
        check_token_work(operation)?;
        #[cfg(test)]
        {
            self.metrics.prefix_appends = self.metrics.prefix_appends.saturating_add(1);
        }
        if fragment.is_empty() {
            return Ok(());
        }

        let mut combined = String::with_capacity(self.unresolved_tail.len() + fragment.len());
        combined.push_str(&self.unresolved_tail);
        combined.push_str(fragment);
        let tokenizer = bpe_openai::o200k_base();
        let normalized = tokenizer.normalize(combined.as_str());
        if normalized.as_str() != combined {
            return Err(TokenCountError::UnsupportedNormalization);
        }

        let mut pending = None;
        for piece in tokenizer.split(normalized.as_str()) {
            check_token_work(operation)?;
            if let Some(closed) = pending.replace(piece) {
                self.committed_tokens = self
                    .committed_tokens
                    .checked_add(tokenizer.bpe.count(closed.as_bytes()))
                    .ok_or(TokenCountError::Overflow)?;
            }
        }
        self.unresolved_tail.clear();
        if let Some(tail) = pending {
            self.unresolved_tail.push_str(tail);
        }
        check_token_work(operation)
    }

    /// Saves the exact state after the current rendered prefix.
    pub(crate) fn checkpoint(&self) -> TokenCheckpoint {
        TokenCheckpoint {
            committed_tokens: self.committed_tokens,
            unresolved_tail: Arc::from(self.unresolved_tail.as_str()),
        }
    }

    /// Counts one candidate suffix without assembling or re-tokenizing its prefix.
    pub(crate) fn count_with_suffix(
        &mut self,
        checkpoint: &TokenCheckpoint,
        suffix: &str,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<usize, TokenCountError> {
        #[cfg(test)]
        {
            self.metrics.suffix_probes = self.metrics.suffix_probes.saturating_add(1);
        }
        let mut tail = String::with_capacity(checkpoint.unresolved_tail.len() + suffix.len());
        tail.push_str(&checkpoint.unresolved_tail);
        tail.push_str(suffix);
        checkpoint
            .committed_tokens
            .checked_add(count_exact_pieces(&tail, operation)?)
            .ok_or(TokenCountError::Overflow)
    }

    /// Independently verifies the fully assembled response exactly once.
    pub(crate) fn verify_full(
        &mut self,
        text: &str,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<usize, TokenCountError> {
        check_token_work(operation)?;
        #[cfg(test)]
        {
            self.metrics.full_tokenizer_calls = self.metrics.full_tokenizer_calls.saturating_add(1);
        }
        let count = estimate_tokens(text);
        check_token_work(operation)?;
        Ok(count)
    }

    #[cfg(test)]
    pub(crate) fn metrics(&self) -> ExactPrefixMetrics {
        self.metrics
    }
}

fn count_exact_pieces(
    text: &str,
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<usize, TokenCountError> {
    check_token_work(operation)?;
    let tokenizer = bpe_openai::o200k_base();
    let normalized = tokenizer.normalize(text);
    if normalized.as_str() != text {
        return Err(TokenCountError::UnsupportedNormalization);
    }
    let mut total = 0_usize;
    for piece in tokenizer.split(normalized.as_str()) {
        check_token_work(operation)?;
        total = total
            .checked_add(tokenizer.bpe.count(piece.as_bytes()))
            .ok_or(TokenCountError::Overflow)?;
    }
    check_token_work(operation)?;
    Ok(total)
}

fn check_token_work(operation: Option<&dyn WorkCheckpoint>) -> Result<(), TokenCountError> {
    match operation.map(WorkCheckpoint::check_work) {
        Some(Err(stop)) => Err(TokenCountError::Stopped(stop)),
        Some(Ok(())) | None => Ok(()),
    }
}

/// Incremental line-boundary counter used while building paged responses.
///
/// Each logical line is encoded independently, including the separator before
/// it. This is deliberately conservative relative to encoding the final text
/// in one pass because BPE merges cannot cross the chosen line boundary.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LineTokenCounter {
    tokens: usize,
    has_line: bool,
}

impl LineTokenCounter {
    /// Appends one output line and returns the accumulated token count.
    pub fn push(&mut self, line: &str) -> usize {
        if self.has_line {
            self.tokens = self.tokens.saturating_add(estimate_tokens("\n"));
        }
        self.tokens = self.tokens.saturating_add(estimate_tokens(line));
        self.has_line = true;
        self.tokens
    }

    /// Returns the accumulated conservative count.
    pub fn tokens(&self) -> usize {
        self.tokens
    }
}

/// Assembles body lines and notes with LF separators without appending a hidden final newline.
pub fn assemble_text(lines: &[String], notes: &[String]) -> String {
    let mut text = lines.join("\n");
    if !notes.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&notes.join("\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{
        ErrorBudgetAdapter, ErrorClass, ExactPrefixCounter, LineTokenCounter, assemble_text,
        estimate_tokens, parse_token_budget,
    };
    use crate::{ToolContent, ToolResponse};

    #[test]
    fn tokenizer_matches_known_o200k_base_vectors() {
        assert_eq!(estimate_tokens("hello world"), 2);
        assert_eq!(estimate_tokens("界字"), 2);
        assert_eq!(estimate_tokens("{\"a\":1}"), 5);
    }

    #[test]
    fn incremental_counter_is_conservative_at_line_boundaries() {
        let lines = ["alpha", "界字", "{\"a\":1}"];
        let mut counter = LineTokenCounter::default();
        for line in lines {
            counter.push(line);
        }
        assert!(counter.tokens() >= estimate_tokens(&lines.join("\n")));
    }

    #[test]
    fn exact_prefix_checkpoints_match_full_o200k_for_every_chunking() {
        let text = "Alpha界 123\n\n punctuation///\r\n最后 e\u{301} tail";
        let boundaries = std::iter::once(0)
            .chain(text.char_indices().map(|(index, _)| index).skip(1))
            .chain(std::iter::once(text.len()))
            .collect::<Vec<_>>();

        for split in boundaries {
            let mut counter = ExactPrefixCounter::default();
            counter.append(&text[..split], None).unwrap();
            let checkpoint = counter.checkpoint();
            let exact = counter
                .count_with_suffix(&checkpoint, &text[split..], None)
                .unwrap();
            assert_eq!(exact, estimate_tokens(text), "split={split}");
        }
    }

    #[test]
    fn checkpoint_restore_preserves_exact_prefix_and_suffix_counts() {
        let mut counter = ExactPrefixCounter::default();
        counter.append("one long", None).unwrap();
        let checkpoint = counter.checkpoint();
        counter.append(" prefix that is discarded", None).unwrap();

        let mut restored = ExactPrefixCounter::from_checkpoint(&checkpoint);
        restored.append(" replacement", None).unwrap();
        let restored_checkpoint = restored.checkpoint();
        let suffix = "\n\n(Complete.)";
        assert_eq!(
            restored
                .count_with_suffix(&restored_checkpoint, suffix, None)
                .unwrap(),
            estimate_tokens("one long replacement\n\n(Complete.)")
        );
    }

    #[test]
    fn exact_prefix_handles_o200k_whitespace_lookahead_across_appends() {
        let fragments = ["a ", " ", "b", "\r", "\n", "   ", "tail"];
        let mut expected = String::new();
        let mut counter = ExactPrefixCounter::default();
        for fragment in fragments {
            expected.push_str(fragment);
            counter.append(fragment, None).unwrap();
            let checkpoint = counter.checkpoint();
            assert_eq!(
                counter.count_with_suffix(&checkpoint, "", None).unwrap(),
                estimate_tokens(&expected)
            );
        }
        let checkpoint = counter.checkpoint();
        assert_eq!(
            counter.count_with_suffix(&checkpoint, "", None).unwrap(),
            estimate_tokens(&expected)
        );
    }

    #[test]
    fn every_error_family_is_exactly_bounded_for_every_tiny_budget() {
        let independent = tiktoken_rs::o200k_base_singleton();
        let cases = [
            (
                ErrorClass::Budget,
                "FASTCTX_GREP_TOKEN_BUDGET=1 is too small to return the required grep continuation note. Increase it and retry.",
            ),
            (ErrorClass::Cancelled, "Request cancelled."),
            (
                ErrorClass::Other,
                "Path does not exist: /a/diagnostic/that/is/intentionally/long.",
            ),
            (
                ErrorClass::Other,
                "Permission denied while accessing /a/protected/search/root.",
            ),
            (
                ErrorClass::Other,
                "Cannot determine the encoding of /a/legacy/file.txt; candidates: windows-1252, gbk, shift_jis.",
            ),
        ];
        for budget in 1..=32 {
            for (class, message) in cases {
                let response = ErrorBudgetAdapter::new(budget, "FASTCTX_GREP_TOKEN_BUDGET")
                    .error(class, message);
                assert!(response.is_error);
                let [ToolContent::Text(text)] = response.content.as_slice() else {
                    panic!("expected one text error");
                };
                assert!(
                    independent.encode_ordinary(text).len() <= budget,
                    "independent oracle exceeded budget={budget}, text={text:?}"
                );
            }
        }
    }

    #[test]
    fn adapter_preserves_fitting_errors_and_infers_cancellation() {
        let original = "Path does not exist: /tmp/missing.";
        assert_eq!(
            ErrorBudgetAdapter::new(8_500, "FASTCTX_TOKEN_BUDGET")
                .adapt(ToolResponse::error(original)),
            ToolResponse::error(original)
        );
        let response = ErrorBudgetAdapter::new(1, "FASTCTX_TOKEN_BUDGET")
            .adapt(ToolResponse::error("Request cancelled."));
        let [ToolContent::Text(text)] = response.content.as_slice() else {
            panic!("expected one text error");
        };
        assert!(estimate_tokens(text) <= 1);
    }

    #[test]
    fn assembly_has_one_blank_line_before_adjacent_notes_and_no_final_newline() {
        assert_eq!(
            assemble_text(
                &["body".to_string()],
                &["(first)".to_string(), "(second)".to_string()]
            ),
            "body\n\n(first)\n(second)"
        );
    }

    #[test]
    fn invalid_budget_values_fail_with_the_exact_actionable_message() {
        assert_eq!(
            parse_token_budget("FASTCTX_TOKEN_BUDGET", "0").unwrap_err(),
            "Invalid FASTCTX_TOKEN_BUDGET value \"0\": expected a positive integer."
        );
        assert_eq!(
            parse_token_budget("FASTCTX_TOKEN_BUDGET", "many").unwrap_err(),
            "Invalid FASTCTX_TOKEN_BUDGET value \"many\": expected a positive integer."
        );
        assert_eq!(
            parse_token_budget("FASTCTX_READ_TOKEN_BUDGET", "0").unwrap_err(),
            "Invalid FASTCTX_READ_TOKEN_BUDGET value \"0\": expected a positive integer."
        );
    }
}
