//! The World envelope is also a durable contract in host-owned history.

use serde::{Deserialize, Serialize};

use crate::attachments::Attachment;
use crate::{MachineName, NoticeId};

use super::{Happened, SessionEvent, Speaker, UserMessage};

const HEAD: &str = "=== from fastctx (user attachments, v2) ===\n";
/// The head written before 2026-10-09 (UTC); histories still hold it.
const HEAD_V1: &str = "=== from the World (user attachments, v1) ===\n";
const WORDS: &str = "\n=== user's words (verbatim) ===\n";
const EXPLANATION: &str = "The user attached these files; sizes are in bytes. Look at one with `fastctx view --attachment ID` in a script on any machine you work on; images come back as images. A script on owner.machine can also use the original at path as a file.";

/// Metadata belongs to the World; text after the delimiter is the user's
/// exact input. A delivery id lets native history and live receipts agree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedMessage {
    pub delivery: NoticeId,
    pub from: MachineName,
    pub attachments: Vec<Attachment>,
}

/// Restores individual World messages from a native host item. Codex can
/// place several text blocks in one item, so each envelope retains its own
/// delivery identity. Native content and detail remain available verbatim.
pub fn restore(event: SessionEvent) -> Vec<SessionEvent> {
    let Happened::Heard { text, message, turn, .. } = &event.happened else { return vec![event] };
    let mut decoded = Vec::new();
    if let Some(message) = message {
        for block in &message.content {
            if let Some(text) = block.get("text").and_then(serde_json::Value::as_str)
                && let Some((metadata, words)) = AttachedMessage::read(text)
            {
                decoded.push((metadata, words.to_string()));
            }
        }
    }
    if decoded.is_empty()
        && let Some((metadata, words)) = AttachedMessage::read(text)
    {
        decoded.push((metadata, words.to_string()));
    }
    if decoded.is_empty() {
        return vec![event];
    }
    let pictures = message.as_ref().map(|message| super::images::delivered(message)).unwrap_or_default();
    decoded
        .into_iter()
        .map(|(metadata, text)| {
            let mut restored = metadata.message(&text);
            restored.images = pictures
                .iter()
                .filter(|(delivery, _)| delivery.as_ref() == Some(&metadata.delivery))
                .map(|(_, image)| image.clone())
                .collect();
            if let Some(native) = message {
                restored.content = native.content.clone();
                restored.detail = native.detail.clone();
                restored.presentation = native.presentation.clone();
            }
            SessionEvent {
                at: event.at,
                happened: Happened::Heard {
                    delivery: Some(metadata.delivery.clone()),
                    turn: turn.clone(),
                    from: Some(metadata.speaker()),
                    text,
                    message: Some(Box::new(restored)),
                    notice: None,
                },
            }
        })
        .collect()
}

impl AttachedMessage {
    pub fn render(&self, text: &str) -> String {
        let metadata = serde_json::to_string(self).expect("attachment metadata serializes");
        format!("{HEAD}{EXPLANATION}\n{metadata}{WORDS}{text}")
    }

    /// Recognizes only the complete versioned envelope, never a bare path
    /// or a host's unrelated attachment. Unrecognized input stays verbatim.
    pub fn read(text: &str) -> Option<(Self, &str)> {
        // The explanation is prose; the versioned header and JSON carry
        // the contract, so clarifying that prose does not invalidate history.
        let (_, body) = text.strip_prefix(HEAD).or_else(|| text.strip_prefix(HEAD_V1))?.split_once('\n')?;
        let (metadata, text) = body.split_once(WORDS)?;
        let metadata: Self = serde_json::from_str(metadata).ok()?;
        (!metadata.attachments.is_empty()).then_some((metadata, text))
    }

    pub fn message(&self, text: &str) -> UserMessage {
        UserMessage {
            id: self.delivery.to_string(),
            client_id: None,
            content: vec![serde_json::json!({"type": "text", "text": text})],
            hook_fragments: Vec::new(),
            presentation: serde_json::Value::Null,
            detail: serde_json::Value::Null,
            attachments: self.attachments.clone(),
            images: Vec::new(),
        }
    }

    pub fn speaker(&self) -> Speaker {
        Speaker::User { machine: self.from.clone() }
    }
}
