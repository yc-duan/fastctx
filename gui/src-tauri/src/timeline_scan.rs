//! An explicit export walks pages independently of the visible reader window.

use fastctx_types::{
    session::SessionEvent,
    timeline_pages::{
        PAGE_BYTES, PAGE_EVENTS, TimelineAsk, TimelineEvent, TimelineFrom, TimelinePlace, TimelineShown, TimelineTold,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub(crate) struct Cursor {
    revision: String,
    after: TimelinePlace,
}
#[derive(Serialize)]
pub(crate) struct Scanned {
    events: Vec<SessionEvent>,
    next: Option<Cursor>,
}

async fn page(
    reader: &crate::timeline_watch::Shared,
    from: TimelineFrom,
) -> Result<(u64, Vec<TimelineEvent>, bool, bool), String> {
    let (world, resident, machine) = {
        let r = reader.lock().await;
        (r.world.clone(), r.resident.clone(), r.machine.clone())
    };
    let response = tokio::select! {
        response = world.timeline(machine, TimelineAsk::Page { resident, from, max_events: PAGE_EVENTS, max_bytes: PAGE_BYTES }) => response.map_err(|e| e.to_string())?,
        () = reader.closed() => return Err("the conversation window closed".into()),
    };
    match response {
        TimelineTold::Page { revision: Some(revision), events, older, newer } => {
            Ok((revision, events, older.is_some(), newer.is_some()))
        }
        _ => Err("the timeline is not available for a complete export".into()),
    }
}

pub(crate) async fn read(reader: crate::timeline_watch::Shared, cursor: Option<Cursor>) -> Result<Scanned, String> {
    let first = cursor.is_none();
    let expected =
        cursor.as_ref().map(|c| c.revision.parse::<u64>().map_err(|_| "invalid export revision")).transpose()?;
    let from = cursor.map_or(TimelineFrom::Latest, |c| TimelineFrom::After { place: c.after });
    let (revision, mut events, mut older, mut newer) = page(&reader, from).await?;
    if expected.is_some_and(|r| r != revision) {
        return Err("the conversation changed during export; copy it again".into());
    }
    // The public API has no earliest selector. Discover its first page without
    // retaining the intervening pages, then stream forward from that cursor.
    while first && older {
        let place = events.first().ok_or("an export page did not advance")?.place.clone();
        let (current, before, has_older, has_newer) = page(&reader, TimelineFrom::Before { place }).await?;
        if current != revision {
            return Err("the conversation changed during export; copy it again".into());
        }
        if before.is_empty() {
            return Err("an export page did not advance".into());
        }
        events = before;
        older = has_older;
        newer = has_newer;
    }
    let mut output = Vec::new();
    let mut bytes = 0;
    let mut after = None;
    let total = events.len();
    for (at, event) in events.into_iter().enumerate() {
        let full = match event.shown {
            TimelineShown::Event { event } => serde_json::from_value(event).map_err(|e| e.to_string())?,
            TimelineShown::Oversized { bytes, object, .. } => {
                let raw = crate::timeline_body::fetch(
                    reader.clone(),
                    crate::timeline_window::Object { at: 0, bytes, object },
                    false,
                )
                .await?;
                serde_json::from_str(raw.get()).map_err(|e| e.to_string())?
            }
        };
        bytes += serde_json::to_vec(&full).map_or(0, |b| b.len());
        output.push(full);
        after = Some(event.place);
        if bytes >= PAGE_BYTES as usize {
            newer |= at + 1 < total;
            break;
        }
    }
    Ok(Scanned {
        events: output,
        next: if newer { after.map(|after| Cursor { revision: revision.to_string(), after }) } else { None },
    })
}

#[fastctx_gui_command::command]
pub(crate) async fn conversation_scan(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    cursor: Option<Cursor>,
) -> Result<Scanned, String> {
    let reader = shell.timeline_readers.get(id).ok_or("the conversation is no longer watched")?;
    read(reader, cursor).await
}
