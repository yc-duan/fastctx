//! Legacy public financial structs ignore extra fields. Their asset envelope
//! must close those nested maps so a financial share cannot carry private links.
use crate::{Error, json, usage::Usage};
use serde_json::Value;

pub(crate) fn decode(body: &[u8]) -> Result<Usage, Error> {
    let value = Value::Object(json::object(body)?);
    if let Some(amount) = value.get("amount") {
        exact(amount.get("tokens"), &["input", "cache_read", "cache_write", "output", "reasoning"])?;
        exact(amount.get("kinds"), &["input", "cache_read", "cache_write", "output"])?;
    }
    if let Some(call) = value.get("call") {
        exact(call.get("timing"), &["started_at", "first_at", "ended_at", "approximate"])?;
    }
    for window in value.get("windows").and_then(Value::as_array).into_iter().flatten() {
        exact(Some(window), &["name", "minutes", "used_tenths", "resets_at", "since"])?;
    }
    for reset in value.get("resets").and_then(Value::as_array).into_iter().flatten() {
        exact(
            Some(reset),
            &["window", "since", "confirmed_at", "resets_at", "before_used_tenths", "used_tenths", "source"],
        )?;
    }
    if let Some(conversation) = value.get("conversation") {
        exact(Some(conversation), &["scope", "object"])?;
        exact(conversation.get("scope"), &["hub", "scope"])?;
    }
    json::typed(value)
}
fn exact(value: Option<&Value>, keys: &[&str]) -> Result<(), Error> {
    if let Some(map) = value.and_then(Value::as_object) {
        json::exact(map, keys)?;
    }
    Ok(())
}
