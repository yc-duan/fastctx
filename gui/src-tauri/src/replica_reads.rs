//! What a page asks of a conversation it shows from its timeline
//! (`replica`): a part of it, read where its reader came to, and an event
//! kept whole as an object, read when the reader opens it. Neither is held
//! here once it is handed to the page.

use crate::replica::{From, Part, Shared};
use base64::{Engine, engine::general_purpose::STANDARD};
use fastctx_types::timeline_pages::{OBJECT_BYTES, TimelineAsk, TimelineTold};
use serde_json::value::RawValue;
use std::time::Duration;

/// The longest event opened whole.
pub(crate) const EXPANDED_BYTES: u64 = 8 << 20;
/// How long opening one event whole may take.
const OPENED_WITHIN: Duration = Duration::from_secs(60);

/// The part of the timeline read from `from`; none when the timeline was
/// made anew since the page asked, which is told the page by itself.
#[fastctx_gui_command::command]
pub(crate) async fn conversation_page(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    from: From,
) -> Result<Option<Part>, String> {
    let reader = shell.readers.get(id).ok_or("the conversation is no longer watched")?;
    if reader.generation() != generation {
        return Ok(None);
    }
    let read = reader.read(from).await.map_err(|error| error.to_string())?;
    Ok((read.part.generation == generation).then_some(read.part))
}

/// An event opened whole, and the object it was kept as.
#[derive(serde::Serialize)]
pub(crate) struct Expanded {
    object: String,
    event: Box<RawValue>,
}

/// Reads the event kept whole as `object`, of `bytes` bytes, as a page
/// named it.
#[fastctx_gui_command::command]
pub(crate) async fn conversation_object(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    object: String,
    bytes: u64,
) -> Result<Expanded, String> {
    let reader = shell.readers.get(id).ok_or("the conversation is no longer watched")?;
    let event = whole(&reader, &object, bytes).await?;
    Ok(Expanded { object, event })
}

/// The event kept whole as `object`: every byte of it read, checked
/// against its name, and read as an event before it is handed on.
pub(crate) async fn whole(reader: &Shared, object: &str, size: u64) -> Result<Box<RawValue>, String> {
    if size > EXPANDED_BYTES {
        return Err("This event exceeds 8 MiB. Use the complete output file, when the tool provides one.".into());
    }
    let deadline = tokio::time::Instant::now() + OPENED_WITHIN;
    // One at a time for each conversation: a reader opens one event, and
    // the rest wait their turn.
    let _turn = tokio::select! {
        turn = tokio::time::timeout_at(deadline, reader.bodies.acquire()) =>
            turn.map_err(|_| "waiting to open the event exceeded 60 s")?.map_err(|_| "the content reader closed")?,
        () = reader.closed() => return Err("the conversation window closed".into()),
    };
    let mut bytes = Vec::with_capacity(size as usize);
    while (bytes.len() as u64) < size {
        let length = (size - bytes.len() as u64).min(u64::from(OBJECT_BYTES)) as u32;
        let asked = TimelineAsk::Object {
            resident: reader.resident.clone(),
            object: object.to_string(),
            offset: bytes.len() as u64,
            length,
        };
        let told = tokio::time::timeout_at(deadline, reader.ask(asked))
            .await
            .map_err(|_| "opening the event exceeded 60 s")?
            .map_err(|error| error.to_string())?;
        let TimelineTold::Object { base64: Some(encoded) } = told else {
            return Err("the event's content is not available now".into());
        };
        let chunk = STANDARD.decode(encoded).map_err(|error| error.to_string())?;
        if chunk.is_empty() || chunk.len() > length as usize {
            return Err("the event object returned an invalid range".into());
        }
        bytes.extend(chunk);
    }
    let named = object.to_string();
    tokio::task::spawn_blocking(move || {
        use sha2::{Digest, Sha256};
        if format!("{:x}", Sha256::digest(&bytes)) != named {
            return Err("the event content did not match its object digest".into());
        }
        let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
        serde_json::from_str::<fastctx_types::session::SessionEvent>(&text).map_err(|error| error.to_string())?;
        RawValue::from_string(text).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
