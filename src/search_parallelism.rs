//! Shared detection and validation for grep/glob CPU parallelism.

use std::fmt;

/// Hard ceiling for effective `P`: one request-local base lane plus shared extra lanes.
pub(crate) const MAX_SEARCH_PARALLELISM: usize = 16;

/// Resolved search parallelism using the same ceiling as the executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SearchParallelism {
    /// Engine-visible CPU ceiling derived from `available_parallelism`.
    pub(crate) available: usize,
    /// Explicit user limit, or `None` for automatic parallelism.
    pub(crate) configured: Option<usize>,
    /// Effective `P`, including the request-local base lane.
    pub(crate) effective: usize,
}

/// Range failure for a typed persisted CPU limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SearchParallelismRangeError {
    pub(crate) maximum: usize,
}

impl fmt::Display for SearchParallelismRangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "must be an integer in 1..={} or omitted for automatic parallelism",
            self.maximum
        )
    }
}

/// User-input failure category for the TUI's editable CPU limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchParallelismInputError {
    Empty { maximum: usize },
    NotInteger { maximum: usize },
    OutOfRange { maximum: usize },
}

impl SearchParallelismInputError {
    /// Engine ceiling that should be shown in the recovery hint.
    pub(crate) const fn maximum(self) -> usize {
        match self {
            Self::Empty { maximum }
            | Self::NotInteger { maximum }
            | Self::OutOfRange { maximum } => maximum,
        }
    }
}

/// Detects the exact upper bound used by the search executor.
pub(crate) fn detected_available() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, MAX_SEARCH_PARALLELISM)
}

/// Resolves an optional persisted limit against the current engine-visible ceiling.
pub(crate) fn resolve(
    configured: Option<i64>,
) -> Result<SearchParallelism, SearchParallelismRangeError> {
    resolve_with_available(configured, detected_available())
}

/// Parses a TUI value. `auto` clears the explicit limit; an empty edit is rejected.
pub(crate) fn parse_input(
    input: &str,
    maximum: usize,
) -> Result<Option<i64>, SearchParallelismInputError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(SearchParallelismInputError::Empty { maximum });
    }
    if input.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    let configured = input
        .parse::<i64>()
        .map_err(|_| SearchParallelismInputError::NotInteger { maximum })?;
    validate(configured, maximum)
        .map(|value| Some(value as i64))
        .map_err(|_| SearchParallelismInputError::OutOfRange { maximum })
}

fn resolve_with_available(
    configured: Option<i64>,
    available: usize,
) -> Result<SearchParallelism, SearchParallelismRangeError> {
    let available = available.clamp(1, MAX_SEARCH_PARALLELISM);
    let configured = configured
        .map(|value| validate(value, available))
        .transpose()?;
    Ok(SearchParallelism {
        available,
        configured,
        effective: configured.unwrap_or(available),
    })
}

fn validate(value: i64, maximum: usize) -> Result<usize, SearchParallelismRangeError> {
    usize::try_from(value)
        .ok()
        .filter(|value| (1..=maximum).contains(value))
        .ok_or(SearchParallelismRangeError { maximum })
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_SEARCH_PARALLELISM, SearchParallelismInputError, parse_input, resolve_with_available,
    };

    #[test]
    fn engine_ceiling_and_configured_limit_share_one_resolution_path() {
        let automatic = resolve_with_available(None, 64).unwrap();
        assert_eq!(automatic.available, MAX_SEARCH_PARALLELISM);
        assert_eq!(automatic.configured, None);
        assert_eq!(automatic.effective, MAX_SEARCH_PARALLELISM);

        let configured = resolve_with_available(Some(4), 64).unwrap();
        assert_eq!(configured.available, MAX_SEARCH_PARALLELISM);
        assert_eq!(configured.configured, Some(4));
        assert_eq!(configured.effective, 4);

        let fallback_machine = resolve_with_available(None, 0).unwrap();
        assert_eq!(fallback_machine.available, 1);
        assert_eq!(fallback_machine.effective, 1);
    }

    #[test]
    fn persisted_limits_accept_both_boundaries_and_reject_every_out_of_range_shape() {
        for value in [1, 4, 8] {
            let resolved = resolve_with_available(Some(value), 8).unwrap();
            assert_eq!(resolved.configured, Some(value as usize));
            assert_eq!(resolved.effective, value as usize);
        }
        for value in [i64::MIN, -1, 0, 9, i64::MAX] {
            let error = resolve_with_available(Some(value), 8).unwrap_err();
            assert_eq!(error.maximum, 8);
        }
    }

    #[test]
    fn editable_input_has_distinct_auto_empty_format_and_range_states() {
        assert_eq!(parse_input("auto", 8), Ok(None));
        assert_eq!(parse_input("AUTO", 8), Ok(None));
        assert_eq!(parse_input("1", 8), Ok(Some(1)));
        assert_eq!(parse_input("8", 8), Ok(Some(8)));
        assert_eq!(
            parse_input("   ", 8),
            Err(SearchParallelismInputError::Empty { maximum: 8 })
        );
        for input in ["four", "1.5", "true", "+"] {
            assert_eq!(
                parse_input(input, 8),
                Err(SearchParallelismInputError::NotInteger { maximum: 8 })
            );
        }
        for input in ["-1", "0", "9"] {
            assert_eq!(
                parse_input(input, 8),
                Err(SearchParallelismInputError::OutOfRange { maximum: 8 })
            );
        }
    }
}
