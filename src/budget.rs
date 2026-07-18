//! Exact o200k_base token accounting and text response assembly.

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
    use super::{LineTokenCounter, assemble_text, estimate_tokens, parse_token_budget};

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
