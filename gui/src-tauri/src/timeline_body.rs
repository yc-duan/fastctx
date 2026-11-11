//! Explicit expansion of oversized events. Neither opening nor paging calls
//! this path. Full events are handed to the page, never cached by the shell.

use base64::{Engine, engine::general_purpose::STANDARD};
use fastctx_types::timeline_pages::{OBJECT_BYTES, TimelineAsk, TimelineTold};
use serde_json::value::RawValue;

pub(crate) const EXPANDED_BYTES: u64 = 8 << 20;

pub(crate) async fn read(
    reader: crate::timeline_watch::Shared,
    _generation: u64,
    object_hash: String,
) -> Result<Box<RawValue>, String> {
    let object = reader
        .lock()
        .await
        .objects
        .iter()
        .find(|o| o.object == object_hash)
        .cloned()
        .ok_or("this event has no deferred content")?;
    fetch(reader, object, true).await
}

pub(crate) async fn fetch(
    reader: crate::timeline_watch::Shared,
    object: crate::timeline_window::Object,
    held: bool,
) -> Result<Box<RawValue>, String> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let permits = if held { &reader.bodies } else { &reader.scan_bodies };
    let _permit = tokio::select! {
        permit = tokio::time::timeout_at(deadline, permits.acquire()) =>
            permit.map_err(|_| "waiting to open the event exceeded 60 s")?.map_err(|_| "the content reader closed")?,
        () = reader.closed() => return Err("the conversation window closed".into()),
    };
    let (world, machine, resident) = {
        let r = reader.lock().await;
        (r.world.clone(), r.machine.clone(), r.resident.clone())
    };
    if object.bytes > EXPANDED_BYTES {
        return Err("This event exceeds 8 MiB. Use the complete output file, when the tool provides one.".into());
    }
    let mut bytes = Vec::with_capacity(object.bytes as usize);
    while (bytes.len() as u64) < object.bytes {
        let length = (object.bytes - bytes.len() as u64).min(u64::from(OBJECT_BYTES)) as u32;
        let reading = world.timeline(
            machine.clone(),
            TimelineAsk::Object {
                resident: resident.clone(),
                object: object.object.clone(),
                offset: bytes.len() as u64,
                length,
            },
        );
        let reply = tokio::select! {
            reply = tokio::time::timeout_at(deadline, reading) => reply.map_err(|_| "opening the event exceeded 60 s")?.map_err(|e| e.to_string())?,
            () = reader.closed() => return Err("the conversation window closed".into()),
        };
        let TimelineTold::Object { base64: Some(encoded) } = reply else {
            return Err("the event's content is not available now".into());
        };
        let chunk = STANDARD.decode(encoded).map_err(|e| e.to_string())?;
        if chunk.is_empty() || chunk.len() > length as usize {
            return Err("the event object returned an invalid range".into());
        }
        bytes.extend(chunk);
        if held && !reader.lock().await.objects.iter().any(|held| held.object == object.object) {
            return Err("the event is no longer in the conversation window".into());
        }
    }
    tokio::task::spawn_blocking(move || {
        use sha2::{Digest, Sha256};
        if format!("{:x}", Sha256::digest(&bytes)) != object.object {
            return Err("the event content did not match its object digest".into());
        }
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        // Validate off the runtime threads before replacing a usable preview.
        serde_json::from_str::<fastctx_types::session::SessionEvent>(&text).map_err(|e| e.to_string())?;
        RawValue::from_string(text).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
