//! A block of thinking as Claude Code's own record keeps it, once its row is
//! in the session file: how long the model thought, as Claude Code measured
//! it (`thinkingDurationMs`, from the moment the block started streaming to
//! the moment it ended), and what the request it belongs to cost
//! (`message.usage`, whose `output_tokens_details.thinking_tokens` counts
//! the request's thinking, not the block's).
//!
//! Claude Code writes a response's rows when it records the step, after the
//! response's usage is final, so the row carries the request's final counts
//! (2.1.296). No hook is told either figure: `turn.step` reports the four
//! token counts only and `session.append` hands over the blocks alone.

use fastctx_types::session::{Happened, Item, SessionChange, SessionEvent, What};
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

pub struct Thought {
    pub uuid: String,
    /// When Claude Code stored the row, which is when the thinking ended.
    pub at: Option<Timestamp>,
    pub duration_ms: Option<u64>,
    /// The words it showed of its thinking; empty where it kept them.
    pub text: String,
    pub block: Value,
    /// The response the row is part of, and the model that gave it.
    pub response: Option<String>,
    pub model: Option<String>,
    pub api_error: bool,
    pub usage: Value,
}

impl Thought {
    /// The thinking an assistant row holds, when its one block is thinking.
    pub fn read(row: &Value) -> Option<Thought> {
        if row["type"] != "assistant" {
            return None;
        }
        let message = &row["message"];
        let [block] = message["content"].as_array()?.as_slice() else { return None };
        let text = match block["type"].as_str()? {
            "thinking" => block["thinking"].as_str().unwrap_or_default().to_string(),
            "redacted_thinking" => String::new(),
            _ => return None,
        };
        Some(Thought {
            uuid: row["uuid"].as_str()?.to_string(),
            at: row["timestamp"].as_str().and_then(|at| at.parse().ok()),
            duration_ms: row["thinkingDurationMs"].as_u64(),
            text,
            block: block.clone(),
            response: message["id"].as_str().map(str::to_string),
            model: message["model"].as_str().map(str::to_string),
            api_error: row["isApiErrorMessage"] == true,
            usage: message["usage"].clone(),
        })
    }

    /// The live item `item` of `turn` that this row ended, as Claude Code
    /// recorded it, with the request's usage beside it.
    pub fn events(&self, turn: &str, item: &str) -> Vec<SessionEvent> {
        let at = self.at.unwrap_or_else(Timestamp::now);
        let started_at = match self.duration_ms.and_then(|ms| i64::try_from(ms).ok()) {
            Some(ms) => at.checked_sub(SignedDuration::from_millis(ms)).ok(),
            None => Some(at),
        };
        let mut events = vec![SessionEvent {
            at,
            happened: Happened::Finished {
                turn: turn.to_string(),
                item: Item {
                    draft: None,
                    id: item.to_string(),
                    started_at,
                    detail: Some(self.block.clone()),
                    what: What::Thinking { text: self.text.clone(), duration_ms: self.duration_ms },
                },
            },
        }];
        if let Some(model) = &self.model {
            events.push(SessionEvent {
                at,
                happened: Happened::Changed {
                    turn: Some(turn.to_string()),
                    change: SessionChange::MessageUsage {
                        items: vec![item.to_string()],
                        response: self.response.clone(),
                        model: model.clone(),
                        api_error: self.api_error,
                        usage: self.usage.clone(),
                    },
                },
            });
        }
        events
    }
}
