//! What an event of a history is known by in the timeline. An event that
//! says what it is (an item, words handed over, a turn's start or end) keeps
//! its name however its content changes; any other is named by its content.
//! The identity is the name and the content together, so an event whose
//! content changed is another item in place of the one it was, and a history
//! is compared with the timeline by identities alone.

use fastctx_types::session::SessionEvent;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub type Identity = [u8; 32];

/// What names an event apart from what it holds, when something does.
pub fn name(event: &SessionEvent) -> Option<String> {
    fastctx_types::likeness::event_name(event)
}

/// Pair an original live event with its durable preview, without numbering
/// repeated occurrences (`fastctx_types::likeness`).
pub fn likeness(event: &SessionEvent, encoded: &[u8]) -> [u8; 32] {
    fastctx_types::likeness::likeness(event, encoded)
}

/// As `likeness`, of an event whose encoding's SHA-256 is `content`.
pub fn likeness_of(event: &SessionEvent, content: &[u8; 32]) -> [u8; 32] {
    fastctx_types::likeness::likeness_of(event, content)
}

fn half(digest: &[u8]) -> [u8; 16] {
    digest[..16].try_into().expect("half a SHA-256")
}

/// The identity of each event of a history, given each as it is encoded.
/// Events of one name are told apart by their order among themselves.
pub fn identities(history: &[SessionEvent], encoded: &[Vec<u8>]) -> Vec<Identity> {
    let mut seen: HashMap<[u8; 16], u32> = HashMap::new();
    history
        .iter()
        .zip(encoded)
        .map(|(event, encoded)| {
            let content = Sha256::digest(encoded);
            let mut named = match name(event) {
                Some(name) => half(&Sha256::digest(name.as_bytes())),
                None => half(&content),
            };
            let before = seen.entry(named).or_insert(0);
            if *before > 0 {
                let mut again = Sha256::new();
                again.update(named);
                again.update(before.to_le_bytes());
                named = half(&again.finalize());
            }
            *before += 1;
            let mut identity = [0; 32];
            identity[..16].copy_from_slice(&named);
            identity[16..].copy_from_slice(&content[..16]);
            identity
        })
        .collect()
}
