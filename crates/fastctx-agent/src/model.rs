//! Protocol-neutral model messages and clients. Requests execute on this machine.
mod anthropic;
mod http;
mod openai;

pub use anthropic::Anthropic;
pub use fastctx_types::agent::{ModelProtocol, ModelSettings};
pub use openai::OpenAi;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}
impl Usage {
    /// Input includes cached input, so cache counters must not be added twice.
    pub fn total(&self) -> u64 {
        self.input.saturating_add(self.output)
    }
    pub fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User { text: String },
    Assistant { text: String, calls: Vec<ToolCall>, raw: Value },
    Results { results: Vec<ToolResult> },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
    pub error: bool,
}
#[derive(Clone, Debug)]
pub struct Request {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
    pub max_output: u64,
}
#[derive(Clone, Debug)]
pub struct Response {
    pub message: Message,
    pub usage: Usage,
    pub stop: Stop,
    pub stop_reason: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    End,
    Tools,
    Limit,
    Refusal,
    Other(String),
}
#[derive(Clone, Debug)]
pub struct Error {
    pub reason: String,
    pub retryable: bool,
    pub retry_after: Option<std::time::Duration>,
    pub usage: Usage,
    pub usage_known: bool,
}
impl Error {
    pub fn protocol(reason: impl Into<String>) -> Self {
        Self { reason: reason.into(), retryable: true, retry_after: None, usage: Usage::default(), usage_known: false }
    }
}
pub type ModelFuture<'a> = Pin<Box<dyn Future<Output = Result<Response, Error>> + Send + 'a>>;
pub trait Model: Send + Sync {
    fn complete<'a>(&'a self, request: &'a Request) -> ModelFuture<'a>;
}

/// Credentials are supplied by the host and never serialized into run snapshots.
pub fn client(settings: ModelSettings, key: String) -> Box<dyn Model> {
    match settings.protocol {
        ModelProtocol::OpenAi => Box::new(OpenAi::new(settings, key)),
        ModelProtocol::Anthropic => Box::new(Anthropic::new(settings, key)),
    }
}
