//! Exact o200k_base token accounting and text response assembly.

use crate::{ToolContent, ToolResponse};
use std::cell::RefCell;

/// Default output budget with 15% headroom below the Codex host's approximate 10k-token limit.
pub const DEFAULT_TOKEN_BUDGET: usize = 8_500;
/// Environment variable for the global text budget.
pub const GLOBAL_TOKEN_BUDGET_ENV: &str = "FASTCTX_TOKEN_BUDGET";
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReservationLevel {
    Full,
    Summary,
    None,
}

#[derive(Clone, Debug)]
struct ResponseReservationState {
    variable: &'static str,
    configured_budget: usize,
    full_line: String,
    full_tokens: usize,
    summary_line: String,
    summary_tokens: usize,
    level: ReservationLevel,
}

thread_local! {
    static RESPONSE_RESERVATION: RefCell<Option<ResponseReservationState>> = const { RefCell::new(None) };
}

/// One thread-local response-budget reservation installed around a single tool formatter.
pub(crate) struct ResponseReservation {
    previous: Option<ResponseReservationState>,
    active: bool,
}

/// The status line selected after the formatter has had a chance to request a fallback.
pub(crate) struct ResponseReservationOutcome {
    pub(crate) configured_budget: usize,
    pub(crate) line: Option<String>,
    pub(crate) summary_line: String,
}

impl ResponseReservation {
    /// Reserves the exact rendered status-line cost before the tool reads its budget.
    pub(crate) fn install(
        variable: &'static str,
        full_line: String,
        summary_line: String,
    ) -> Option<Self> {
        let configured_budget = configured_tool_token_budget(variable).ok()?.value;
        Some(Self::install_with_budget(
            variable,
            configured_budget,
            full_line,
            summary_line,
        ))
    }

    fn install_with_budget(
        variable: &'static str,
        configured_budget: usize,
        full_line: String,
        summary_line: String,
    ) -> Self {
        // A status line is inserted with at most one extra blank-line separator. Counting
        // the pieces independently is conservative because joining pieces can merge tokens
        // across the boundary but cannot require more than encoding both pieces separately.
        let separator_tokens = estimate_tokens("\n\n");
        let full_tokens = estimate_tokens(&full_line).saturating_add(separator_tokens);
        let summary_tokens = estimate_tokens(&summary_line).saturating_add(separator_tokens);
        // A successful text response always has user content, so an exact-fit status would
        // consume the whole budget and must yield to the next tier.
        let level = if full_tokens < configured_budget {
            ReservationLevel::Full
        } else if summary_tokens < configured_budget {
            ReservationLevel::Summary
        } else {
            ReservationLevel::None
        };
        let state = ResponseReservationState {
            variable,
            configured_budget,
            full_line,
            full_tokens,
            summary_line,
            summary_tokens,
            level,
        };
        let previous = RESPONSE_RESERVATION.with(|slot| slot.borrow_mut().replace(state));
        Self {
            previous,
            active: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn install_for_test(
        variable: &'static str,
        configured_budget: usize,
        full_line: String,
        summary_line: String,
    ) -> Self {
        Self::install_with_budget(variable, configured_budget, full_line, summary_line)
    }

    /// Relaxes full status to the summary and then to no status.
    pub(crate) fn downgrade(&self) -> bool {
        downgrade_response_reservation(None)
    }

    /// Restores any outer reservation and returns the line selected for this response.
    pub(crate) fn finish(mut self) -> ResponseReservationOutcome {
        let state = RESPONSE_RESERVATION.with(|slot| slot.borrow_mut().take());
        RESPONSE_RESERVATION.with(|slot| *slot.borrow_mut() = self.previous.take());
        self.active = false;
        let state = state.expect("an installed response reservation must remain active");
        let summary_line = state.summary_line.clone();
        let line = match state.level {
            ReservationLevel::Full => Some(state.full_line),
            ReservationLevel::Summary => Some(state.summary_line),
            ReservationLevel::None => None,
        };
        ResponseReservationOutcome {
            configured_budget: state.configured_budget,
            line,
            summary_line,
        }
    }
}

impl Drop for ResponseReservation {
    fn drop(&mut self) {
        if self.active {
            RESPONSE_RESERVATION.with(|slot| {
                *slot.borrow_mut() = self.previous.take();
            });
        }
    }
}

/// Reads the global text budget, rejecting invalid configuration instead of silently falling back.
pub fn token_budget() -> Result<usize, String> {
    Ok(apply_response_reservation(
        GLOBAL_TOKEN_BUDGET_ENV,
        configured_token_budget()?,
    ))
}

fn configured_token_budget() -> Result<usize, String> {
    match crate::session::var(GLOBAL_TOKEN_BUDGET_ENV) {
        Ok(value) => parse_token_budget(GLOBAL_TOKEN_BUDGET_ENV, &value),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_TOKEN_BUDGET),
        Err(std::env::VarError::NotUnicode(_)) => Err(
            "Invalid FASTCTX_TOKEN_BUDGET value: expected a UTF-8 positive integer.".to_string(),
        ),
    }
}

/// Reads a tool budget; omission inherits the global value and explicit values may not exceed it.
pub fn tool_token_budget(variable: &'static str) -> Result<TokenBudget, String> {
    let mut budget = configured_tool_token_budget(variable)?;
    budget.value = apply_response_reservation(variable, budget.value);
    Ok(budget)
}

fn configured_tool_token_budget(variable: &'static str) -> Result<TokenBudget, String> {
    let global = configured_token_budget()?;
    match crate::session::var(variable) {
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
    let Ok(global) = configured_token_budget() else {
        return DEFAULT_TOKEN_BUDGET;
    };
    match crate::session::var(variable) {
        Ok(value) => parse_token_budget(variable, &value)
            .ok()
            .filter(|value| *value <= global)
            .unwrap_or(global),
        Err(_) => global,
    }
}

fn apply_response_reservation(variable: &'static str, configured: usize) -> usize {
    RESPONSE_RESERVATION.with(|slot| {
        let slot = slot.borrow();
        let Some(state) = slot.as_ref().filter(|state| state.variable == variable) else {
            return configured;
        };
        let reserved = match state.level {
            ReservationLevel::Full => state.full_tokens,
            ReservationLevel::Summary => state.summary_tokens,
            ReservationLevel::None => 0,
        };
        state.configured_budget.saturating_sub(reserved)
    })
}

fn downgrade_response_reservation(variable: Option<&'static str>) -> bool {
    RESPONSE_RESERVATION.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot
            .as_mut()
            .filter(|state| variable.is_none_or(|variable| state.variable == variable))
        else {
            return false;
        };
        state.level = match state.level {
            // Keep at least one token available for the user-facing response body.
            ReservationLevel::Full if state.summary_tokens < state.configured_budget => {
                ReservationLevel::Summary
            }
            ReservationLevel::Full | ReservationLevel::Summary => ReservationLevel::None,
            ReservationLevel::None => return false,
        };
        true
    })
}

/// Relaxes the background status by one tier and returns the newly available tool budget.
pub(crate) fn relax_tool_token_budget(
    variable: &'static str,
) -> Result<Option<TokenBudget>, String> {
    if !downgrade_response_reservation(Some(variable)) {
        return Ok(None);
    }
    tool_token_budget(variable).map(Some)
}

/// Makes room for an irreducible response note before an operation with side effects begins.
pub(crate) fn tool_token_budget_for_required(
    variable: &'static str,
    required_tokens: usize,
) -> Result<TokenBudget, String> {
    loop {
        let budget = tool_token_budget(variable)?;
        if budget.value >= required_tokens || !downgrade_response_reservation(Some(variable)) {
            return Ok(budget);
        }
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

/// Cheap incremental estimate used to stop collecting a text page near its budget.
/// The completed head-first response is always counted exactly before delivery.
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

    /// Returns the accumulated fragment estimate.
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
        ErrorBudgetAdapter, ErrorClass, assemble_text, estimate_tokens, parse_token_budget,
    };
    use crate::{ToolContent, ToolResponse};

    #[test]
    fn tokenizer_matches_known_o200k_base_vectors() {
        assert_eq!(estimate_tokens("hello world"), 2);
        assert_eq!(estimate_tokens("界字"), 2);
        assert_eq!(estimate_tokens("{\"a\":1}"), 5);
    }

    #[test]
    fn every_error_family_is_exactly_bounded_for_every_tiny_budget() {
        let independent = tiktoken_rs::o200k_base_singleton();
        let cases = [
            (
                ErrorClass::Budget,
                "FASTCTX_GREP_TOKEN_BUDGET=1 is too small to return the grep head note and one result. Increase it and retry.",
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
