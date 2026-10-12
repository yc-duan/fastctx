//! An image a tool gave back in a thread, read from the thread's record by
//! the tool's item, for a viewer that was sent only a reference to it.

use std::sync::Arc;

use fastctx_types::session::{Content, What};
use serde_json::{Value, json};

use super::{CodexHost, items, server::Unanswered};

impl CodexHost {
    /// The `image`th image of what the item `item` of `turn` gave back, as
    /// its media type and base64; `None` when the record holds no such
    /// image.
    pub async fn tool_image(
        self: &Arc<Self>,
        thread: &str,
        turn: &str,
        item: &str,
        image: u32,
    ) -> Result<Option<(String, String)>, String> {
        let server = self.server().await?;
        let mut params = json!({"threadId": thread, "turnId": turn, "sortDirection": "asc"});
        let found = loop {
            let page = match server.ask("thread/items/list", params.clone()).await {
                Ok(page) => page,
                // Older app-servers list no items; their thread read has all.
                Err(Unanswered::Rpc { code: -32601, data, .. }) if data.is_null() => {
                    let answer =
                        server.request("thread/read", json!({"threadId": thread, "includeTurns": true})).await?;
                    break answer
                        .pointer("/thread/turns")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|entry| entry["id"] == turn)
                        .flat_map(|entry| entry["items"].as_array().into_iter().flatten())
                        .find(|entry| entry["id"] == item)
                        .cloned();
                }
                Err(error) => return Err(error.to_string()),
            };
            let entries = page["data"].as_array().ok_or("Codex supplied no per-turn item list")?;
            if let Some(entry) = entries.iter().find(|entry| entry["item"]["id"] == item) {
                break Some(entry["item"].clone());
            }
            match page["nextCursor"].as_str() {
                Some(cursor) => params["cursor"] = Value::String(cursor.into()),
                None => break None,
            }
        };
        let Some(found) = found else { return Ok(None) };
        let items::Read::Item(read) = items::read(&found, true) else { return Ok(None) };
        let What::Tool(tool) = read.what else { return Ok(None) };
        Ok(tool
            .output
            .into_iter()
            .filter_map(|content| match content {
                Content::Image { mime, base64 } => Some((mime, base64)),
                _ => None,
            })
            .nth(image as usize))
    }
}
