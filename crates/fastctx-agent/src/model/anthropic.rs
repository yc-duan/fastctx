//! Anthropic-compatible Messages with client tool results grouped in one turn.
use super::*;
use serde_json::{Value, json};

pub struct Anthropic {
    settings: ModelSettings,
    key: String,
}
impl Anthropic {
    pub fn new(settings: ModelSettings, key: String) -> Self {
        Self { settings, key }
    }
}
impl Model for Anthropic {
    fn complete<'a>(&'a self, request: &'a Request) -> ModelFuture<'a> {
        Box::pin(async move {
            let mut messages = Vec::new();
            for message in &request.messages {
                messages.push(match message {
                    Message::User { text } => json!({"role":"user","content":text}),
                    Message::Assistant { raw, .. } => json!({"role":"assistant","content":raw}),
                    Message::Results { results } => json!({"role":"user","content":results.iter().map(|r| json!({"type":"tool_result","tool_use_id":r.id,"content":r.content,"is_error":r.error})).collect::<Vec<_>>()}),
                });
            }
            let tools: Vec<_> = request
                .tools
                .iter()
                .map(|t| json!({"name":t.name,"description":t.description,"input_schema":t.parameters}))
                .collect();
            let body = json!({"model":self.settings.model,"system":request.system,"messages":messages,"tools":tools,"stream":false,"max_tokens":request.max_output});
            let value = http::post(
                &self.settings.endpoint,
                &[("x-api-key", &self.key), ("anthropic-version", "2023-06-01")],
                body,
            )
            .await?;
            let u = &value["usage"];
            let cache_read = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
            let cache_write = u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            let usage = Usage {
                input: http::number(u, "input_tokens")?.saturating_add(cache_read).saturating_add(cache_write),
                output: http::number(u, "output_tokens")?,
                cache_read,
                cache_write,
            };
            decode(&value, usage.clone()).map_err(|mut error| {
                error.usage = usage;
                error.usage_known = true;
                error
            })
        })
    }
}
fn decode(value: &Value, usage: Usage) -> Result<Response, Error> {
    if value["type"] != "message" || value["role"] != "assistant" {
        return Err(Error::protocol("invalid Messages response type or role"));
    }
    let raw = value.get("content").ok_or_else(|| Error::protocol("missing content"))?.clone();
    let mut text = String::new();
    let mut calls = Vec::new();
    for block in raw.as_array().ok_or_else(|| Error::protocol("invalid content blocks"))? {
        match block["type"].as_str() {
            Some("text") => text.push_str(&http::string(block, "text")?),
            Some("tool_use") => calls.push(ToolCall {
                id: http::string(block, "id")?,
                name: http::string(block, "name")?,
                arguments: block["input"].clone(),
            }),
            Some("thinking" | "redacted_thinking") => {}
            _ => return Err(Error::protocol("unsupported Messages content block")),
        }
    }
    let reason = http::string(value, "stop_reason")?;
    let stop = match reason.as_str() {
        "end_turn" => Stop::End,
        "tool_use" => Stop::Tools,
        "max_tokens" | "model_context_window_exceeded" => Stop::Limit,
        "refusal" => Stop::Refusal,
        _ => Stop::Other(reason.clone()),
    };
    if (stop == Stop::Tools) == calls.is_empty() {
        return Err(Error::protocol("tool calls disagree with stop_reason"));
    }
    let mut ids = std::collections::BTreeSet::new();
    if calls.iter().any(|c| c.id.is_empty() || !ids.insert(&c.id) || !c.arguments.is_object()) {
        return Err(Error::protocol("invalid tool call ID or arguments"));
    }
    Ok(Response { message: Message::Assistant { text, calls, raw }, usage, stop, stop_reason: reason })
}
