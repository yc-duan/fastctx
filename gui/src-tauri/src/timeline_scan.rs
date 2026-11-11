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
    #[serde(default)]
    seeking: bool,
}
#[derive(Serialize)]
pub(crate) struct Truncated {
    at: usize,
    bytes: u64,
}
#[derive(Serialize)]
pub(crate) struct Scanned {
    events: Vec<SessionEvent>,
    truncated: Vec<Truncated>,
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
        response = tokio::time::timeout(std::time::Duration::from_secs(30), world.timeline(machine, TimelineAsk::Page { resident, from, max_events: PAGE_EVENTS, max_bytes: PAGE_BYTES })) => response.map_err(|_| "reading the timeline page exceeded 30 s")?.map_err(|e| e.to_string())?,
        () = reader.closed() => return Err("the conversation window closed".into()),
    };
    match response {
        TimelineTold::Page { revision: Some(revision), events, older, newer, .. } => {
            Ok((revision, events, older.is_some(), newer.is_some()))
        }
        _ => Err("the timeline is not available for a complete export".into()),
    }
}

pub(crate) async fn read(reader: crate::timeline_watch::Shared, cursor: Option<Cursor>) -> Result<Scanned, String> {
    let seeking = cursor.as_ref().is_none_or(|c| c.seeking);
    let expected =
        cursor.as_ref().map(|c| c.revision.parse::<u64>().map_err(|_| "invalid export revision")).transpose()?;
    let from = cursor.map_or(TimelineFrom::Latest, |c| {
        if c.seeking { TimelineFrom::Before { place: c.after } } else { TimelineFrom::After { place: c.after } }
    });
    let (revision, events, older, mut newer) = page(&reader, from).await?;
    if expected.is_some_and(|r| r != revision) {
        return Err("the conversation changed during export; copy it again".into());
    }
    // Seeking is itself paged: one invocation performs at most one page read,
    // so a caller can cancel and report progress before reaching the start.
    if seeking && older {
        let after = events.first().ok_or("an export page did not advance")?.place.clone();
        return Ok(Scanned {
            events: Vec::new(),
            truncated: Vec::new(),
            next: Some(Cursor { revision: revision.to_string(), after, seeking: true }),
        });
    }
    let mut output = Vec::new();
    let mut truncated = Vec::new();
    let mut bytes = 0;
    let mut after = None;
    let total = events.len();
    for (at, event) in events.into_iter().enumerate() {
        let fallback = if matches!(event.shown, TimelineShown::Oversized { bytes, .. } if bytes > crate::timeline_body::EXPANDED_BYTES)
        {
            crate::timeline_window::Stored::from(event.clone()).event
        } else {
            None
        };
        let full = match event.shown {
            TimelineShown::Event { event } => serde_json::from_value(event).map_err(|e| e.to_string())?,
            TimelineShown::Oversized { bytes, .. } if bytes > crate::timeline_body::EXPANDED_BYTES => {
                truncated.push(Truncated { at: output.len(), bytes });
                fallback.ok_or("the oversized event has no readable preview")?
            }
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
        truncated,
        next: if newer {
            after.map(|after| Cursor { revision: revision.to_string(), after, seeking: false })
        } else {
            None
        },
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
