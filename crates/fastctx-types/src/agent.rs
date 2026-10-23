//! Configuration shared by internal agents and their callers. No credentials.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelProtocol {
    OpenAi,
    Anthropic,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelSettings {
    pub protocol: ModelProtocol,
    /// Full endpoint URL, including `/chat/completions` or `/messages`.
    pub endpoint: String,
    pub model: String,
}
