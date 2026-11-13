//! What an event of a history is known by however it is read: its name,
//! where it says what it is (an item, words handed over, a turn's start or
//! end), and its likeness — the name, or the content when it names none,
//! beside the content — which pairs a live event with the same event read
//! from a timeline page, wherever the page was read (a machine's own
//! timeline, or a copy of it on another machine or a phone).

use crate::session::{Happened, SessionEvent};
use sha2::{Digest, Sha256};

/// What names an event apart from what it holds, when something does.
pub fn event_name(event: &SessionEvent) -> Option<String> {
    match &event.happened {
        Happened::Finished { item, .. } => Some(format!("item/{}", item.id)),
        Happened::Heard { delivery: Some(delivery), .. } => {
            Some(format!("delivery/{}", serde_json::to_string(delivery).unwrap_or_default()))
        }
        Happened::Heard { message: Some(message), .. } => Some(format!("words/{}", message.id)),
        Happened::TurnStarted { turn } => Some(format!("started/{turn}")),
        Happened::TurnEnded { turn, .. } => Some(format!("ended/{turn}")),
        _ => None,
    }
}

/// Pairs an original live event with its durable preview, without numbering
/// repeated occurrences. Both halves are SHA-256 prefixes of the original:
/// of its name (or content), then of its content as encoded.
pub fn likeness(event: &SessionEvent, encoded: &[u8]) -> [u8; 32] {
    let content = Sha256::digest(encoded);
    let named = event_name(event).map_or_else(|| half(&content), |name| half(&Sha256::digest(name.as_bytes())));
    let mut likeness = [0; 32];
    likeness[..16].copy_from_slice(&named);
    likeness[16..].copy_from_slice(&content[..16]);
    likeness
}

fn half(digest: &[u8]) -> [u8; 16] {
    digest[..16].try_into().expect("half a SHA-256")
}
