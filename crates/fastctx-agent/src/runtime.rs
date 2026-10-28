//! A run has one deadline, finite requests, tools and conservative token allowance.
use crate::{
    FutureResult,
    model::{Message, Model, Request, Stop, Tool, ToolCall, ToolResult, Usage},
    state,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::time::Instant;

#[derive(Clone, Debug)]
pub struct Limits {
    pub duration: Duration,
    pub request_timeout: Duration,
    pub requests: u32,
    pub tools: u32,
    pub tokens: u64,
    pub output_tokens: u64,
    pub context_bytes: usize,
    pub tool_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(120),
            request_timeout: Duration::from_secs(30),
            requests: 12,
            tools: 16,
            tokens: 64_000,
            output_tokens: 2_048,
            context_bytes: 32_768,
            tool_bytes: 8_192,
        }
    }
}
pub enum Decision {
    Finish(Value),
    Continue(String),
}
pub struct ToolOutput {
    pub value: Value,
    pub finish: Option<Value>,
}
impl ToolOutput {
    pub fn value(value: Value) -> Self {
        Self { value, finish: None }
    }
}

/// Applications own tool effects and validate conclusions. The registry is fixed
/// before the first request; calls to any other name never reach the application.
pub trait Application: Send {
    fn tools(&self) -> Vec<Tool>;
    /// Tools with side effects must be idempotent for the supplied call ID.
    fn call<'a>(&'a mut self, call: &'a ToolCall) -> FutureResult<'a, ToolOutput>;
    fn finish(&mut self, text: &str) -> Result<Decision, String>;
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Run {
    pub messages: Vec<Message>,
    pub usage: Usage,
    pub requests: u32,
    /// Includes in-flight requests and failures whose server usage is unknown.
    pub unreported_requests: u32,
    pub tool_calls: u32,
    pub reserved_tokens: u64,
    pub elapsed_ms: u64,
    pub pending_tool: Option<ToolCall>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub ended: bool,
    pub stop_reason: Option<String>,
}

/// Starts a fresh bounded conversation and checkpoints before each external tool.
/// A snapshot with `ended == false` is interrupted, not a valid conclusion; a host
/// may start a fresh run with current evidence instead of replaying unknown effects.
pub async fn run(
    model: &dyn Model,
    app: &mut dyn Application,
    system: String,
    input: String,
    limits: &Limits,
    path: &Path,
) -> Result<Run, String> {
    let start = Instant::now();
    let deadline = start + limits.duration;
    let mut run = Run { messages: vec![Message::User { text: input }], ..Run::default() };
    state::save(path, &run)?;
    let result = tokio::time::timeout_at(deadline, drive(model, app, system, limits, path, &mut run)).await;
    match result {
        Ok(Ok(value)) => run.result = Some(value),
        Ok(Err(error)) => run.error = Some(error),
        Err(_) => run.error = Some("agent run deadline exceeded".into()),
    }
    run.elapsed_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    run.ended = true;
    state::save(path, &run)?;
    Ok(run)
}
async fn drive(
    model: &dyn Model,
    app: &mut dyn Application,
    system: String,
    limits: &Limits,
    path: &Path,
    run: &mut Run,
) -> Result<Value, String> {
    let tools = app.tools();
    let mut names = std::collections::BTreeSet::new();
    for tool in &tools {
        if !names.insert(&tool.name) {
            return Err("duplicate tool name".into());
        }
    }
    let mut failures = 0;
    loop {
        let request = Request {
            system: system.clone(),
            messages: run.messages.clone(),
            tools: tools.clone(),
            max_output: limits.output_tokens,
        };
        // UTF-8 byte count plus protocol framing is a conservative allowance for
        // text tokenizers. Reserve even failed requests: servers may have billed them.
        let bytes = serde_json::to_vec(&json!({"system":system,"messages":run.messages,"tools":tools}))
            .map_err(|e| e.to_string())?
            .len();
        if bytes > limits.context_bytes {
            return Err("agent context limit exceeded".into());
        }
        let reservation = (bytes as u64).saturating_add(1024).saturating_add(request.max_output);
        if run.requests >= limits.requests || run.reserved_tokens.saturating_add(reservation) > limits.tokens {
            return Err("agent request or token budget exhausted".into());
        }
        run.requests += 1;
        run.unreported_requests += 1;
        run.reserved_tokens += reservation;
        state::save(path, run)?;
        let response = match tokio::time::timeout(limits.request_timeout, model.complete(&request)).await {
            Ok(Ok(response)) => response,
            result => {
                let error = match result {
                    Ok(Err(error)) => error,
                    _ => crate::model::Error::protocol("model request timed out"),
                };
                run.usage.add(&error.usage);
                if error.usage_known {
                    run.unreported_requests -= 1;
                }
                state::save(path, run)?;
                if run.usage.total() > limits.tokens {
                    return Err("model exceeded the token allowance".into());
                }
                if !error.retryable || failures >= 2 {
                    return Err(error.reason);
                }
                failures += 1;
                tokio::time::sleep(error.retry_after.unwrap_or(Duration::from_millis(250 << (failures - 1)))).await;
                continue;
            }
        };
        run.stop_reason = Some(response.stop_reason);
        run.usage.add(&response.usage);
        run.unreported_requests -= 1;
        if run.usage.total() > limits.tokens {
            return Err("model exceeded the token allowance".into());
        }
        let Message::Assistant { text, calls, .. } = &response.message else {
            return Err("model returned a non-assistant message".into());
        };
        let text = text.clone();
        let calls = calls.clone();
        run.messages.push(response.message);
        state::save(path, run)?;
        if response.stop == Stop::Tools {
            if calls.is_empty() {
                return Err("model stopped for tools without a call".into());
            }
            let mut results = Vec::new();
            let mut conclusion = None;
            for call in calls {
                if run.tool_calls >= limits.tools {
                    return Err("agent tool budget exhausted".into());
                }
                run.tool_calls += 1;
                run.pending_tool = Some(call.clone());
                state::save(path, run)?;
                let output = if !tools.iter().any(|t| t.name == call.name) {
                    Err(crate::words::UNKNOWN_TOOL.into())
                } else if conclusion.is_some() {
                    Err("run has already concluded".into())
                } else {
                    app.call(&call).await
                };
                let (content, error) = match output {
                    Ok(output) => {
                        conclusion = output.finish;
                        (output.value.to_string(), false)
                    }
                    Err(error) => (error, true),
                };
                results.push(ToolResult { id: call.id, content: bounded(&content, limits.tool_bytes), error });
                run.pending_tool = None;
                // Save completed results immediately, including within a multi-call batch.
                let mut snapshot = run.clone();
                snapshot.messages.push(Message::Results { results: results.clone() });
                state::save(path, &snapshot)?;
            }
            run.messages.push(Message::Results { results });
            if let Some(value) = conclusion {
                return Ok(value);
            }
        } else if response.stop != Stop::End {
            return Err(format!("model stopped without a conclusion: {:?}", response.stop));
        } else {
            match app.finish(&text) {
                Ok(Decision::Finish(value)) => return Ok(value),
                Ok(Decision::Continue(text)) => run.messages.push(Message::User { text }),
                Err(error) => {
                    if failures >= 2 {
                        return Err(error);
                    }
                    failures += 1;
                    run.messages.push(Message::User { text: crate::words::INVALID_RESULT.into() });
                }
            }
        }
    }
}

/// Cuts only at a UTF-8 boundary and makes truncation visible.
pub fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut end = limit.saturating_sub(16).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &text[..end])
}
