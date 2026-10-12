//! A native user picture addressed through the public per-turn item list.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};

use super::{CodexHost, server::Unanswered};

pub enum UserPicture {
    Local(std::path::PathBuf),
    Inline { mime: String, base64: String },
}

impl CodexHost {
    /// The selected native picture. The outer `None` means this app-server
    /// has only legacy history; the inner `None` means no picture is recorded
    /// at that position in its authoritative item list.
    pub async fn user_picture(
        self: &Arc<Self>,
        thread: &str,
        turn: &str,
        message: u32,
        image: u32,
    ) -> Result<Option<Option<UserPicture>>, String> {
        let server = self.server().await?;
        let mut params = json!({"threadId": thread, "turnId": turn, "sortDirection": "asc"});
        let mut ids = HashSet::new();
        loop {
            let page = match server.ask("thread/items/list", params.clone()).await {
                Ok(page) => page,
                Err(Unanswered::Rpc { code: -32601, data, .. }) if data.is_null() => return Ok(None),
                Err(error) => return Err(error.to_string()),
            };
            let entries = page["data"].as_array().ok_or("Codex supplied no per-turn item list")?;
            for entry in entries {
                let item = &entry["item"];
                if item["type"] != "userMessage" {
                    continue;
                }
                let id = item["id"].as_str().ok_or("Codex supplied a user submission without its identity")?;
                if !ids.insert(id.to_string()) {
                    continue;
                }
                if ids.len() - 1 != message as usize {
                    continue;
                }
                let blocks = item["content"].as_array().ok_or("Codex supplied user words without content")?;
                let Some(block) = blocks
                    .iter()
                    .filter(|block| matches!(block["type"].as_str(), Some("image" | "localImage")))
                    .nth(image as usize)
                else {
                    return Ok(Some(None));
                };
                let picture = match block["type"].as_str() {
                    Some("localImage") => UserPicture::Local(
                        block["path"].as_str().ok_or("Codex supplied a local picture without its path")?.into(),
                    ),
                    Some("image") => {
                        let url = block["url"].as_str().ok_or("Codex supplied a picture without its URL")?;
                        let (mime, data) = url
                            .strip_prefix("data:")
                            .and_then(|data| data.split_once(";base64,"))
                            .ok_or("Codex supplied a picture that is not inline image data")?;
                        UserPicture::Inline { mime: mime.into(), base64: data.into() }
                    }
                    _ => unreachable!("selected image blocks"),
                };
                return Ok(Some(Some(picture)));
            }
            match page["nextCursor"].as_str() {
                Some(cursor) => params["cursor"] = Value::String(cursor.into()),
                None => return Ok(Some(None)),
            }
        }
    }
}
