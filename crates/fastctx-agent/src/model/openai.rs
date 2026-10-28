//! OpenAI-compatible Chat Completions, with native assistant messages preserved.
use super::*;
use serde_json::{Value, json};

pub struct OpenAi {
    settings: ModelSettings,
    key: String,
}
impl OpenAi {
    pub fn new(settings: ModelSettings, key: String) -> Self {
        Self { settings, key }
    }
}
impl Model for OpenAi {
    fn complete<'a>(&'a self, request: &'a Request) -> ModelFuture<'a> {
        Box::pin(async move {
            let mut messages = vec![json!({"role":"system","content":request.system})];
            for message in &request.messages {
                match message {
                    Message::User { text } => messages.push(json!({"role":"user","content":text})),
                    Message::Assistant { raw, .. } => messages.push(raw.clone()),
                    Message::Results { results } => {
                        for result in results {
                            messages.push(json!({"role":"tool","tool_call_id":result.id,"content":result.content}));
                        }
                    }
                }
            }
            let tools: Vec<_> = request.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect();
            let mut body = json!({"model":self.settings.model,"messages":messages,"tools":tools,"stream":false});
            body[self.settings.output_limit.parameter()] = json!(request.max_output);
            let auth = format!("Bearer {}", self.key);
            let value = http::post(&self.settings.endpoint, &[("authorization", &auth)], body).await?;
            let usage_value = &value["usage"];
            let usage = Usage {
                input: http::number(usage_value, "prompt_tokens")?,
                output: http::number(usage_value, "completion_tokens")?,
                cache_read: usage_value
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_write: usage_value
                    .pointer("/prompt_tokens_details/cache_write_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
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
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .ok_or_else(|| Error::protocol("model returned no choices"))?;
    let raw = choice.get("message").ok_or_else(|| Error::protocol("model returned no message"))?.clone();
    if raw["role"] != "assistant" {
        return Err(Error::protocol("model returned wrong role"));
    }
    let reason = http::string(choice, "finish_reason")?;
    let stop = match reason.as_str() {
        "stop" => Stop::End,
        "tool_calls" => Stop::Tools,
        "length" => Stop::Limit,
        "content_filter" => Stop::Refusal,
        _ => Stop::Other(reason.clone()),
    };
    let mut calls = Vec::new();
    if let Some(values) = raw.get("tool_calls") {
        for value in values.as_array().ok_or_else(|| Error::protocol("invalid tool_calls"))? {
            if value["type"] != "function" {
                return Err(Error::protocol("unsupported tool call type"));
            }
            calls.push(ToolCall {
                id: http::string(value, "id")?,
                name: http::string(&value["function"], "name")?,
                arguments: serde_json::from_str(&http::string(&value["function"], "arguments")?)
                    .map_err(|e| Error::protocol(format!("invalid tool arguments: {e}")))?,
            });
        }
    }
    if (stop == Stop::Tools) == calls.is_empty() {
        return Err(Error::protocol("tool calls disagree with finish_reason"));
    }
    let mut ids = std::collections::BTreeSet::new();
    if calls.iter().any(|c| c.id.is_empty() || !ids.insert(&c.id) || !c.arguments.is_object()) {
        return Err(Error::protocol("invalid tool call ID or arguments"));
    }
    Ok(Response {
        message: Message::Assistant { text: raw["content"].as_str().unwrap_or("").into(), calls, raw },
        usage,
        stop,
        stop_reason: reason,
    })
}
