//! Which turn took words the World sent, found by the id it sent them under
//! (`clientUserMessageId`): Codex keeps it with the words in the thread's
//! record, and reads that record without loading the thread.

use std::sync::Arc;

use fastctx_types::session::TurnEnd;
use serde_json::{Value, json};

use super::CodexHost;

/// The turn some words went into, and how it ended as the thread's record
/// says; no end while the record says it runs.
pub struct Submitted {
    pub turn: String,
    pub end: Option<TurnEnd>,
}

impl CodexHost {
    /// The turn of `thread` that took the words sent as `client`; none when
    /// the thread's record has no words sent so.
    pub async fn submitted(self: &Arc<Self>, thread: &str, client: &str) -> Result<Option<Submitted>, String> {
        let server = self.server().await?;
        let items = super::history::pages(&server, "thread/items/list", json!({"threadId": thread, "sortDirection": "asc"}))
            .await
            .map_err(|error| error.to_string())?;
        let mut turn: Option<String> = None;
        let mut said = None;
        for entry in &items {
            let item = &entry["item"];
            let of = entry.get("turnId").and_then(Value::as_str);
            match item.get("type").and_then(Value::as_str) {
                Some("userMessage")
                    if turn.is_none() && item.get("clientId").and_then(Value::as_str) == Some(client) =>
                {
                    turn = of.map(str::to_string);
                }
                Some("agentMessage") if turn.is_some() && of == turn.as_deref() => {
                    said = item.get("text").and_then(Value::as_str).map(str::to_string);
                }
                _ => {}
            }
        }
        let Some(turn) = turn else { return Ok(None) };
        let turns = super::history::pages(
            &server,
            "thread/turns/list",
            json!({"threadId": thread, "sortDirection": "asc", "itemsView": "notLoaded"}),
        )
        .await
        .map_err(|error| error.to_string())?;
        let end = turns.iter().find(|entry| entry.get("id").and_then(Value::as_str) == Some(turn.as_str())).and_then(
            |entry| {
                let status = entry.get("status").and_then(Value::as_str);
                (status != Some("inProgress")).then(|| TurnEnd {
                    facts: None,
                    error: entry.get("error").filter(|error| !error.is_null()).cloned(),
                    how: super::turn_how(status),
                    reason: entry.pointer("/error/message").and_then(Value::as_str).map(str::to_string),
                    duration_ms: entry.get("durationMs").and_then(Value::as_u64),
                    last_message: said,
                })
            },
        );
        Ok(Some(Submitted { turn, end }))
    }
}
