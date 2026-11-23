//! An event as the timeline keeps it. The host's raw record of an item or a
//! message, which the event carries for the machine it lives on, repeats the
//! bytes of every picture in it; the timeline keeps that record without them,
//! since the pictures are read from the host's own record when they are shown.
//! An event so kept is always written with the likeness of the event as the
//! host told it, so that a later writing of the same event is known to be
//! the same, whichever form an earlier release kept it in: a kept event is
//! never written again only for the way it keeps its pictures.

use fastctx_content::v2::{EVENT, Entry, OVERSIZED};
use fastctx_types::session::{Happened, SessionEvent};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// `event` without the pictures' bytes its raw records repeat; `None` when
/// it carries none.
pub(super) fn form(event: &SessionEvent) -> Option<SessionEvent> {
    match &event.happened {
        Happened::Heard { message: Some(message), .. } => {
            let carries = message.content.iter().any(carries) || carries(&message.detail);
            if !carries {
                return None;
            }
            let mut kept = event.clone();
            if let Happened::Heard { message: Some(message), .. } = &mut kept.happened {
                // Named by their bytes first, while the bytes are here.
                fastctx_net::history::fingerprints(message);
                for block in &mut message.content {
                    strip(block);
                }
                strip(&mut message.detail);
            }
            Some(kept)
        }
        Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } => {
            if !item.detail.as_ref().is_some_and(carries) {
                return None;
            }
            let mut kept = event.clone();
            if let Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } =
                &mut kept.happened
                && let Some(detail) = &mut item.detail
            {
                strip(detail);
            }
            Some(kept)
        }
        _ => None,
    }
}

/// Whether `value` holds a picture's bytes anywhere.
fn carries(value: &Value) -> bool {
    match value {
        Value::Object(map) => image_data(map).is_some_and(|data| !data.is_empty()) || map.values().any(carries),
        Value::Array(items) => items.iter().any(carries),
        Value::String(text) => inline_payload(text).is_some_and(|payload| !payload.is_empty()),
        _ => false,
    }
}

/// Leaves out of `value` the bytes of every picture in it, keeping its shape.
fn strip(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("image")
                && let Some(Value::Object(source)) = map.get_mut("source")
                && source.get("type").and_then(Value::as_str) == Some("base64")
                && let Some(Value::String(data)) = source.get_mut("data")
            {
                data.clear();
            }
            for value in map.values_mut() {
                strip(value);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip),
        Value::String(text) => {
            if let Some(payload) = inline_payload(text).filter(|payload| !payload.is_empty()) {
                let kept = text.len() - payload.len();
                text.truncate(kept);
            }
        }
        _ => {}
    }
}

/// The base64 bytes of an image block, `{"type": "image", "source": {"type":
/// "base64", "data": …}}`.
fn image_data(map: &serde_json::Map<String, Value>) -> Option<&str> {
    if map.get("type").and_then(Value::as_str) != Some("image") {
        return None;
    }
    let source = map.get("source")?.as_object()?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }
    source.get("data")?.as_str()
}

/// The payload of a picture written inline as `data:image/…;base64,…`.
fn inline_payload(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("data:image/")?;
    rest.split_once(";base64,").map(|(_, payload)| payload)
}

/// The likeness the record `body` keeps or makes of its event: told by an
/// event kept apart, made of an event kept whole.
pub(super) fn likeness_in(body: &[u8]) -> Option<[u8; 32]> {
    match body.split_first() {
        Some((&OVERSIZED, told)) => {
            let told: Value = serde_json::from_slice(told).ok()?;
            serde_json::from_value(told.get("likeness")?.clone()).ok()
        }
        Some((&EVENT, encoded)) => {
            let event: SessionEvent = serde_json::from_slice(encoded).ok()?;
            let content: [u8; 32] = Sha256::digest(encoded).into();
            Some(super::identity::likeness_of(&event, &content))
        }
        _ => None,
    }
}

/// Whether `entry` already keeps what the record `body` with `objects`
/// would: the same record, or a record of the same event as the host told it.
pub(super) fn unchanged(entry: &Entry, body: &[u8], objects: &[fastctx_content::ObjectRef]) -> bool {
    (entry.body == body && entry.objects == objects)
        || likeness_in(&entry.body).is_some_and(|kept| likeness_in(body) == Some(kept))
}
