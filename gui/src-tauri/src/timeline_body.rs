//! Explicit expansion of oversized events. Neither opening nor paging calls
//! this path. Full events are handed to the page, never cached by the shell.

use base64::{Engine, engine::general_purpose::STANDARD};
use fastctx_types::timeline_pages::{OBJECT_BYTES, TimelineAsk, TimelineTold};
use serde_json::value::RawValue;

pub(crate) const EXPANDED_BYTES: u64 = 8 << 20;

pub(crate) async fn read(
    reader: crate::timeline_watch::Shared,
    generation: u64,
    at: usize,
) -> Result<Box<RawValue>, String> {
    let _permit = reader
        .bodies
        .clone()
        .try_acquire_owned()
        .map_err(|_| "another event is being opened; try again when it finishes")?;
    let (world, machine, resident, object) = {
        let reader = reader.lock().await;
        if reader.transcript.lock().expect("transcript").generation() != generation {
            return Err("the conversation window changed".into());
        }
        let object = reader.objects.iter().find(|o| o.at == at).cloned().ok_or("this event has no deferred content")?;
        (reader.world.clone(), reader.machine.clone(), reader.resident.clone(), object)
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
            reply = reading => reply.map_err(|e| e.to_string())?,
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
        if !reader.lock().await.objects.iter().any(|held| held.object == object.object) {
            return Err("the event is no longer in the conversation window".into());
        }
    }
    tokio::task::spawn_blocking(move || {
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        // Validate off the runtime threads before replacing a usable preview.
        serde_json::from_str::<fastctx_types::session::SessionEvent>(&text).map_err(|e| e.to_string())?;
        RawValue::from_string(text).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
