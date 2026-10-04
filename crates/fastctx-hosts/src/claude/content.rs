//! Claude's stored tool-result content, including blocks the World does not
//! interpret. Unknown fields on known blocks are retained too.

use fastctx_types::session::Content;
use serde_json::Value;

pub fn content(value: &Value) -> Vec<Content> {
    match value {
        Value::String(text) => vec![Content::Text { text: text.clone() }],
        Value::Array(blocks) => blocks.iter().map(block).collect(),
        Value::Null => Vec::new(),
        other => vec![Content::Other { block: other.clone() }],
    }
}

fn block(value: &Value) -> Content {
    let Some(object) = value.as_object() else { return Content::Other { block: value.clone() } };
    match value.get("type").and_then(Value::as_str) {
        Some("text") if object.len() == 2 => {
            if let Some(text) = value.get("text").and_then(Value::as_str) {
                return Content::Text { text: text.to_string() };
            }
        }
        Some("image") if object.len() == 2 => {
            if let Some(source) = value.get("source").and_then(Value::as_object)
                && source.len() == 3
                && source.get("type").and_then(Value::as_str) == Some("base64")
                && let (Some(mime), Some(base64)) =
                    (source.get("media_type").and_then(Value::as_str), source.get("data").and_then(Value::as_str))
            {
                return Content::Image { mime: mime.to_string(), base64: base64.to_string() };
            }
        }
        _ => {}
    }
    Content::Other { block: value.clone() }
}
