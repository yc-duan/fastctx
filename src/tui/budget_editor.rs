//! Free-form percentage entry for the five per-tool output budgets.
//!
//! Arrow keys nudge a share by one point, which covers correcting a value in view. Reaching an
//! arbitrary share that way would take dozens of keystrokes, so this editor accepts the number
//! directly — and accepts `auto` to hand the tool back to its tier default, which is otherwise
//! unreachable once an explicit share exists.

use crate::control::settings::ToolBudgetLevel;

/// Editable per-tool share plus the last validation failure.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct BudgetEditor {
    /// Raw text as typed, including a trailing percent sign if the user entered one.
    pub(crate) input: String,
    /// Validation failure from the most recent submission, cleared by any further edit.
    pub(crate) error: Option<ToolBudgetInputError>,
}

/// User-input failure category for the TUI's editable per-tool budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolBudgetInputError {
    /// The field was submitted with nothing in it.
    Empty,
    /// The text is neither `auto` nor a whole number.
    NotInteger,
    /// The number parsed but falls outside `1..=100`.
    OutOfRange,
}

/// Parses a submitted value.
///
/// `auto` returns the tool to its tier default; an empty edit is rejected rather than silently
/// treated as `auto`, because the two mean different things and a stray Enter should not
/// discard an explicit share.
pub(crate) fn parse_input(input: &str) -> Result<Option<ToolBudgetLevel>, ToolBudgetInputError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ToolBudgetInputError::Empty);
    }
    if input.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    let digits = input.strip_suffix('%').unwrap_or(input).trim();
    let percent = digits
        .parse::<i64>()
        .map_err(|_| ToolBudgetInputError::NotInteger)?;
    u8::try_from(percent)
        .ok()
        .and_then(ToolBudgetLevel::from_percent)
        .map(Some)
        .ok_or(ToolBudgetInputError::OutOfRange)
}

#[cfg(test)]
mod tests {
    use super::{ToolBudgetInputError, parse_input};
    use crate::control::settings::ToolBudgetLevel;

    #[test]
    fn accepts_bare_percentages_and_the_ui_spelling() {
        assert_eq!(parse_input("20"), Ok(Some(ToolBudgetLevel::Percent(20))));
        assert_eq!(
            parse_input(" 20 % "),
            Ok(Some(ToolBudgetLevel::Percent(20)))
        );
        assert_eq!(parse_input("1"), Ok(Some(ToolBudgetLevel::Percent(1))));
    }

    #[test]
    fn a_full_share_normalizes_to_inheritance() {
        // 100% and "omit the per-tool variable" are the same configuration; keeping two
        // representations would let the same setting round-trip into different files.
        assert_eq!(parse_input("100"), Ok(Some(ToolBudgetLevel::Inherit)));
    }

    #[test]
    fn auto_clears_the_override_and_empty_does_not() {
        assert_eq!(parse_input("auto"), Ok(None));
        assert_eq!(parse_input("AUTO"), Ok(None));
        assert_eq!(parse_input(""), Err(ToolBudgetInputError::Empty));
        assert_eq!(parse_input("   "), Err(ToolBudgetInputError::Empty));
    }

    #[test]
    fn rejects_values_outside_the_representable_range() {
        // Zero would resolve to a budget the server refuses; the rest simply cannot be shares.
        assert_eq!(parse_input("0"), Err(ToolBudgetInputError::OutOfRange));
        assert_eq!(parse_input("101"), Err(ToolBudgetInputError::OutOfRange));
        assert_eq!(parse_input("-5"), Err(ToolBudgetInputError::OutOfRange));
        assert_eq!(
            parse_input("99999999999999999999"),
            Err(ToolBudgetInputError::NotInteger)
        );
        assert_eq!(parse_input("half"), Err(ToolBudgetInputError::NotInteger));
        assert_eq!(parse_input("20.5"), Err(ToolBudgetInputError::NotInteger));
    }
}
