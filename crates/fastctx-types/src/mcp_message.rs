//! Framing admitted by the MCP relay. No raw byte or arbitrary JSON tunnel.

use serde_json::Value;

pub const MAX_LINE: usize = 32 * 1024 * 1024;

/// Checks one complete JSON-RPC 2.0 message or nonempty batch. The caller
/// retains the original bytes; validation does not rewrite IDs or payloads.
pub fn parse(line: &[u8]) -> Result<Value, String> {
    if line.len() > MAX_LINE {
        return Err("an MCP message exceeds 32 MiB".into());
    }
    if line.contains(&b'\n') {
        return Err("an MCP message must occupy one line".into());
    }
    let value: Value = serde_json::from_slice(line).map_err(|e| format!("not an MCP JSON message: {e}"))?;
    let valid = match &value {
        Value::Array(items) => !items.is_empty() && items.iter().all(message),
        value => message(value),
    };
    if valid { Ok(value) } else { Err("not a JSON-RPC 2.0 message".into()) }
}

fn message(value: &Value) -> bool {
    let Some(object) = value.as_object() else { return false };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return false;
    }
    let id = object.get("id");
    if id.is_some_and(|id| !id.is_string() && !id.is_i64() && !id.is_u64() && !id.is_null()) {
        return false;
    }
    if let Some(method) = object.get("method") {
        method.is_string()
            && !object.contains_key("result")
            && !object.contains_key("error")
            && object.get("params").is_none_or(|params| params.is_object() || params.is_array())
    } else {
        id.is_some()
            && (object.contains_key("result") != object.contains_key("error"))
            && object.get("error").is_none_or(|error| {
                error.get("code").is_some_and(Value::is_i64) && error.get("message").is_some_and(Value::is_string)
            })
    }
}
