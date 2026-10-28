//! Model and token observations supplied by the host for one turn.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agents::AgentLimits;
use crate::usage::Usd;

/// Distinct observed values, in their first observed order. An empty list
/// means the host did not report that fact; it does not mean a default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnFacts {
    /// The World's observations, independent of the host's token accounting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_started_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_last_activity_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub world_progress: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub world_compacted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<TurnEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TurnTokens>,
    /// What the turn cost by the host's own cost ledger, where it keeps one
    /// (Claude Code's): the rise of the session's total over the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Usd>,
    /// The login's usage windows as last read when the turn ended; none for
    /// a conversation of an account the login only assists, which does not
    /// see its account's windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<AgentLimits>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnEffort {
    Actual {
        value: Value,
    },
    /// Claude's request override, which its engine can normalize or omit.
    Requested {
        value: Value,
    },
}

/// Only reported usage is counted. An interrupted response without usage
/// and Codex's remote compaction are not observable through these counters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "host", rename_all = "snake_case")]
pub enum TurnTokens {
    Codex {
        /// Includes cached input. Reasoning output is a subset of output.
        input_tokens: u64,
        cached_input_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_write_input_tokens: Option<u64>,
        output_tokens: u64,
        reasoning_output_tokens: u64,
        total_tokens: u64,
    },
    Claude {
        /// Excludes cache reads and writes; the three input counters are separate.
        input_tokens: u64,
        cache_read_input_tokens: u64,
        cache_creation_input_tokens: u64,
        output_tokens: u64,
    },
}

impl TurnFacts {
    pub fn is_empty(&self) -> bool {
        !self.world_progress
            && !self.world_compacted
            && self.world_started_at.is_none()
            && self.world_last_activity_at.is_none()
            && self.models.is_empty()
            && self.efforts.is_empty()
            && self.tokens.is_none()
            && self.cost.is_none()
            && self.limits.is_none()
    }

    pub fn model(&mut self, model: &str) {
        if !self.models.iter().any(|known| known == model) {
            self.models.push(model.to_string());
        }
    }

    pub fn effort(&mut self, effort: TurnEffort) {
        if !self.efforts.contains(&effort) {
            self.efforts.push(effort);
        }
    }
}
