//! An event as the timeline keeps it. The host's raw record of an item or a
//! message, which the event carries for the machine it lives on, often
//! repeats the bytes of a picture the event already holds once (a tool's
//! output, a message's own blocks); the timeline keeps that raw record
//! without the bytes of each picture it repeats, and with those of any it
//! alone holds. An event so kept is always written with the likeness of the
//! event as the host told it, so that a later writing of the same event is
//! known to be the same, whichever form an earlier release kept it in: a
//! kept event is never written again only for the way it keeps its pictures.

use std::collections::HashSet;

use fastctx_content::v2::{EVENT, Entry, OVERSIZED};
use fastctx_types::session::{Happened, SessionEvent};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// `event` without the pictures' bytes its raw record repeats; `None` when
/// it repeats none.
pub(super) fn form(event: &SessionEvent) -> Option<SessionEvent> {
    let detail = match &event.happened {
        Happened::Heard { message: Some(message), .. } => Some(&message.detail),
        Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } => {
            item.detail.as_ref()
        }
        _ => None,
    }?;
    if !carries(detail) {
        return None;
    }
    // The pictures the event holds outside its raw record, by their bytes.
    let mut kept = event.clone();
    let raw = match &mut kept.happened {
        Happened::Heard { message: Some(message), .. } => std::mem::take(&mut message.detail),
        Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } => {
            item.detail.take().unwrap_or_default()
        }
        _ => return None,
    };
    let mut elsewhere = HashSet::new();
    pictures(&serde_json::to_value(&kept).ok()?, &mut elsewhere);
    let mut raw = raw;
    if !strip(&mut raw, &elsewhere) {
        return None;
    }
    match &mut kept.happened {
        Happened::Heard { message: Some(message), .. } => message.detail = raw,
        Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } => {
            item.detail = Some(raw);
        }
        _ => return None,
    }
    Some(kept)
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

/// The digest of a picture's base64 bytes, as they are compared.
fn digest(payload: &str) -> [u8; 32] {
    Sha256::digest(payload.as_bytes()).into()
}

/// Adds to `found` the digest of every picture's bytes in `value`: in a raw
/// record's forms (`image_data`), inline, or as the World holds a tool's
/// picture (`Content::Image`, `{"type": "image", "mime": …, "base64": …}`).
fn pictures(value: &Value, found: &mut HashSet<[u8; 32]>) {
    match value {
        Value::Object(map) => {
            let held = image_data(map).or_else(|| {
                (map.get("type").and_then(Value::as_str) == Some("image"))
                    .then(|| map.get("base64")?.as_str())
                    .flatten()
            });
            if let Some(payload) = held.filter(|payload| !payload.is_empty()) {
                found.insert(digest(payload));
            }
            map.values().for_each(|value| pictures(value, found));
        }
        Value::Array(items) => items.iter().for_each(|value| pictures(value, found)),
        Value::String(text) => {
            if let Some(payload) = inline_payload(text).filter(|payload| !payload.is_empty()) {
                found.insert(digest(payload));
            }
        }
        _ => {}
    }
}

/// Leaves out of `value` the bytes of every picture in it that `elsewhere`
/// holds, keeping its shape; says whether it left out any.
fn strip(value: &mut Value, elsewhere: &HashSet<[u8; 32]>) -> bool {
    match value {
        Value::Object(map) => {
            let repeated = image_data(map).is_some_and(|data| !data.is_empty() && elsewhere.contains(&digest(data)));
            if repeated {
                if let Some(Value::Object(source)) = map.get_mut("source")
                    && let Some(Value::String(data)) = source.get_mut("data")
                {
                    data.clear();
                }
                for field in ["data", "result"] {
                    if let Some(Value::String(data)) = map.get_mut(field) {
                        data.clear();
                    }
                }
            }
            map.values_mut().fold(repeated, |left, value| strip(value, elsewhere) || left)
        }
        Value::Array(items) => items.iter_mut().fold(false, |left, value| strip(value, elsewhere) || left),
        Value::String(text) => match inline_payload(text) {
            Some(payload) if !payload.is_empty() && elsewhere.contains(&digest(payload)) => {
                let kept = text.len() - payload.len();
                text.truncate(kept);
                true
            }
            _ => false,
        },
        _ => false,
    }
}

/// The base64 bytes of a picture in a raw record: Claude's image block
/// `{"type": "image", "source": {"type": "base64", "data": …}}`, an MCP image
/// as Codex keeps it `{"type": "image", "mimeType": …, "data": …}`, or a
/// Codex image generation's `{"type": "imageGeneration", "result": …}`.
fn image_data(map: &serde_json::Map<String, Value>) -> Option<&str> {
    match map.get("type").and_then(Value::as_str)? {
        "image" => match map.get("source") {
            Some(source) => {
                let source = source.as_object()?;
                if source.get("type").and_then(Value::as_str) != Some("base64") {
                    return None;
                }
                source.get("data")?.as_str()
            }
            None => map.get("data")?.as_str(),
        },
        "imageGeneration" => map.get("result")?.as_str(),
        _ => None,
    }
}

/// The payload of a picture written inline as `data:image/…;base64,…`.
fn inline_payload(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("data:image/")?;
    rest.split_once(";base64,").map(|(_, payload)| payload)
}

/// The bytes of the object the record of `event` would be kept with, as
/// `assets::body` makes it: offered before the record is written.
pub(crate) fn object_of(event: &SessionEvent) -> Result<Option<Vec<u8>>, serde_json::Error> {
    match form(event) {
        Some(stored) => serde_json::to_vec(&stored).map(Some),
        None => {
            let encoded = serde_json::to_vec(event)?;
            Ok((encoded.len() >= fastctx_content::RECORD_BYTES).then_some(encoded))
        }
    }
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

#[cfg(test)]
#[path = "stored_tests.rs"]
mod tests;
