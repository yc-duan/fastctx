//! Startup observations exposed by Claude Code 2.1.296's public CLI.

use serde::Deserialize;

/// A supervisor job, distinct from the concurrent session registry's PID.
#[derive(Clone, Debug, Deserialize)]
pub struct Background {
    pub state: String,
    pub pid: Option<u32>,
}

/// Finds exactly the supervisor job named by `--bg`. A missing PID is also
/// normal before Claude registers its session; it does not prove an exit.
pub fn background(printed: &str, short: &str) -> Result<Option<Background>, String> {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(printed).map_err(|error| format!("reading `claude agents --json --all`: {error}"))?;
    let mut found = rows.into_iter().filter(|row| row["kind"] == "background" && row["id"] == short);
    let Some(row) = found.next() else { return Ok(None) };
    if found.next().is_some() {
        return Err(format!("Claude Code reports more than one background job named {short}"));
    }
    serde_json::from_value(row).map(Some).map_err(|error| format!("reading Claude background job {short}: {error}"))
}

impl Background {
    /// Terminal supervisor outcomes take precedence over individual process
    /// exits: Claude can respawn an initialization failure itself.
    pub fn ended(&self) -> bool {
        matches!(self.state.as_str(), "done" | "failed" | "stopped")
    }

    /// This means blocked, not necessarily a dialog: Claude's own startup
    /// watchdog reports the same state when initialization has stalled.
    pub fn blocked(&self) -> bool {
        self.state == "blocked"
    }
}
