//! Durable observations delivered to the conversation that registered a sentinel.
use crate::{MachineName, SentinelId, sentinel::Observation};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentinelNotice {
    pub sentinel: SentinelId,
    pub generation: u64,
    pub machine: MachineName,
    pub at: Timestamp,
    pub due: Timestamp,
    pub condition: String,
    pub tell: String,
    pub event: Event,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Alarm { index: usize },
    Satisfied { reason: String, evidence: Vec<Observation> },
    Unable { reason: String },
}
