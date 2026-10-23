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
    /// OpenAI-compatible endpoints differ here; Anthropic always uses max_tokens.
    #[serde(default, skip_serializing_if = "OutputLimit::is_default")]
    pub output_limit: OutputLimit,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputLimit {
    #[default]
    MaxCompletionTokens,
    MaxTokens,
}
impl OutputLimit {
    pub fn parameter(self) -> &'static str {
        match self {
            Self::MaxCompletionTokens => "max_completion_tokens",
            Self::MaxTokens => "max_tokens",
        }
    }
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
