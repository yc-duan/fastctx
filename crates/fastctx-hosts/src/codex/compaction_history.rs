//! Completed compactions after a known turn, without loading older transcripts.

use super::{CodexHost, server::AppServer};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

impl CodexHost {
    /// Proves that context was compacted after `after`, regardless of who
    /// requested it. This does not identify a particular compact/start RPC.
    /// A missing boundary or unreadable history is an error, not absence.
    pub async fn compacted_after(self: &Arc<Self>, thread: &str, after: &str) -> Result<bool, String> {
        tokio::time::timeout(Duration::from_secs(60), async {
            let server = self.server().await?;
            match read(&server, thread, after).await {
                Err(error) if error == super::not_loaded(thread) => {
                    self.ensure_loaded(&server, thread).await?;
                    read(&server, thread, after).await
                }
                result => result,
            }
        })
        .await
        .map_err(|_| "Codex did not establish compaction history within 60 seconds".to_string())?
    }
}

async fn read(server: &AppServer, thread: &str, after: &str) -> Result<bool, String> {
    let mut request = json!({"threadId":thread,"sortDirection":"desc","itemsView":"notLoaded","limit":32});
    let mut compacted = false;
    loop {
        let page = server.request("thread/turns/list", request.clone()).await?;
        for turn in data(&page)? {
            let id = turn["id"].as_str().ok_or("Codex returned a turn without an ID")?;
            if id == after {
                return Ok(compacted);
            }
            if !compacted {
                compacted = has_compaction(server, thread, id).await?;
            }
        }
        if !advance(&mut request, &page)? {
            return Err(format!("Codex history no longer contains the failed turn {after}"));
        }
    }
}

async fn has_compaction(server: &AppServer, thread: &str, turn: &str) -> Result<bool, String> {
    let mut request = json!({"threadId":thread,"turnId":turn,"sortDirection":"desc","limit":100});
    loop {
        let page = server.request("thread/items/list", request.clone()).await?;
        for entry in data(&page)? {
            if entry["turnId"].as_str() != Some(turn) {
                return Err("Codex returned an item from a different turn".into());
            }
            // Persistent history contains this item only after compaction
            // completed; item/started notifications are not part of this list.
            if entry["item"]["type"] == "contextCompaction" {
                return Ok(true);
            }
        }
        if !advance(&mut request, &page)? {
            return Ok(false);
        }
    }
}

fn data(page: &Value) -> Result<&[Value], String> {
    page["data"].as_array().map(Vec::as_slice).ok_or_else(|| "Codex returned history without data".into())
}

fn advance(request: &mut Value, page: &Value) -> Result<bool, String> {
    match page.get("nextCursor") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::String(cursor)) if request["cursor"].as_str() != Some(cursor) => {
            request["cursor"] = json!(cursor);
            Ok(true)
        }
        _ => Err("Codex returned an invalid history cursor".into()),
    }
}
