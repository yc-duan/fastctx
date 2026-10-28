//! Durable repair receipts. An unchanged layer was checked, not reinstalled.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Checking,
    Unchanged,
    Repaired,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub layer: String,
    pub outcome: Outcome,
    pub detail: String,
    pub actions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub id: String,
    pub version: String,
    pub by: String,
    pub started: Timestamp,
    pub finished: Option<Timestamp>,
    pub process: crate::ProcessIdentity,
    pub steps: Vec<Step>,
    pub success: bool,
    pub unchanged: bool,
    pub error: Option<String>,
}
