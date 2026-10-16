//! What the drill finds, said as it finds it: a line for each check of a
//! scenario, `PASS` or `FAIL`, and what was measured.

use std::fmt::Display;

pub struct Report {
    scenario: &'static str,
    pub passed: usize,
    pub failed: usize,
}

impl Report {
    pub fn new(scenario: &'static str) -> Self {
        Self { scenario, passed: 0, failed: 0 }
    }

    /// Says whether `what` holds, with what was measured; whether it did.
    pub fn check(&mut self, ok: bool, what: impl Display, measured: impl Display) -> bool {
        let measured = measured.to_string();
        let said = if measured.is_empty() { String::new() } else { format!(" ({measured})") };
        if ok {
            self.passed += 1;
            println!("PASS {}: {what}{said}", self.scenario);
        } else {
            self.failed += 1;
            println!("FAIL {}: {what}{said}", self.scenario);
        }
        ok
    }

    /// Says what happened, not a check.
    pub fn note(&self, what: impl Display) {
        println!("  {}: {what}", self.scenario);
    }
}

/// A duration as the report says it.
pub fn seconds(took: std::time::Duration) -> String {
    format!("{:.1} s", took.as_secs_f64())
}
