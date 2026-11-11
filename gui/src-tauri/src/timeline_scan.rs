//! Export fixes its initial end, then reads each event's current version.
//! A rebuilt history restarts the export, at most three attempts in total.

use crate::{timeline_walk as walk, timeline_watch::Shared};
use fastctx_types::{
    session::SessionEvent,
    timeline_pages::{PAGE_BYTES, TimelineEvent, TimelineFrom, TimelinePlace, TimelineShown},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub(crate) struct Cursor {
    revision: String,
    after: TimelinePlace,
    until: TimelinePlace,
    through: Option<String>,
    #[serde(default)]
    seeking: bool,
    #[serde(default)]
    restarts: u8,
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
    restart: bool,
    next: Option<Cursor>,
}

pub(crate) async fn full(reader: &Shared, event: &TimelineEvent) -> Result<(SessionEvent, Option<u64>), String> {
    match &event.shown {
        TimelineShown::Event { event } => Ok((serde_json::from_value(event.clone()).map_err(|e| e.to_string())?, None)),
        TimelineShown::Oversized { bytes, .. } if *bytes > crate::timeline_body::EXPANDED_BYTES => {
            let preview = crate::timeline_window::Stored::from(event.clone())
                .event
                .ok_or("the oversized event has no readable preview")?;
            Ok((preview, Some(*bytes)))
        }
        TimelineShown::Oversized { bytes, object, .. } => {
            let raw = crate::timeline_body::fetch(
                reader.clone(),
                crate::timeline_window::Object { at: 0, bytes: *bytes, object: object.clone() },
                false,
            )
            .await?;
            Ok((serde_json::from_str(raw.get()).map_err(|e| e.to_string())?, None))
        }
    }
}

pub(crate) async fn read(reader: Shared, mut cursor: Option<Cursor>) -> Result<Scanned, String> {
    let mut restart = false;
    let mut restarts = cursor.as_ref().map_or(0, |cursor| cursor.restarts);
    if !walk::continuous(
        &reader,
        cursor
            .as_ref()
            .and_then(|cursor| cursor.through.as_deref())
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|_| "invalid export record cursor")?,
    )
    .await?
    {
        restarts += 1;
        if restarts >= 3 {
            return Err("the history was rebuilt repeatedly during export; copy it again".into());
        }
        cursor = None;
        restart = true;
    }
    let seeking = cursor.as_ref().is_none_or(|cursor| cursor.seeking);
    let from = cursor.as_ref().map_or(TimelineFrom::Latest, |cursor| {
        if cursor.seeking {
            TimelineFrom::Before { place: cursor.after.clone() }
        } else {
            TimelineFrom::After { place: cursor.after.clone() }
        }
    });
    let mut page = walk::page(&reader, from.clone()).await?;
    let until = cursor
        .as_ref()
        .map(|cursor| cursor.until.clone())
        .or_else(|| page.events.last().map(|event| event.place.clone()));
    let Some(until) = until else {
        if page.revision.is_none() {
            return Err("the history is still being prepared".into());
        }
        return Ok(Scanned { events: Vec::new(), truncated: Vec::new(), restart, next: None });
    };
    let through = cursor
        .as_ref()
        .and_then(|cursor| cursor.through.clone())
        .or_else(|| page.through.map(|value| value.to_string()));
    let revision = page.revision.ok_or("the history is still being prepared")?;
    if seeking && page.older {
        let after = page.events.first().ok_or("an export page did not advance")?.place.clone();
        return Ok(Scanned {
            events: Vec::new(),
            truncated: Vec::new(),
            restart,
            next: Some(Cursor { revision: revision.to_string(), after, until, through, seeking: true, restarts }),
        });
    }
    // An object may have lost its reference between Page and Object. Restart
    // this page from its original anchor; discard its uncommitted output.
    for attempt in 0..3 {
        let mut events = Vec::new();
        let mut truncated = Vec::new();
        let mut bytes = 0;
        let mut after = None;
        let mut retry = None;
        let mut more = page.newer;
        for (index, event) in page.events.iter().enumerate() {
            if event.place.order > until.order {
                more = false;
                break;
            }
            match full(&reader, event).await {
                Ok((event_body, preview)) => {
                    if let Some(bytes) = preview {
                        truncated.push(Truncated { at: events.len(), bytes });
                    }
                    bytes += serde_json::to_vec(&event_body).map_or(0, |bytes| bytes.len());
                    events.push(event_body);
                    after = Some(event.place.clone());
                }
                Err(error) => {
                    retry = Some(error);
                    break;
                }
            }
            if event.place.order >= until.order {
                more = false;
                break;
            }
            if bytes >= PAGE_BYTES as usize {
                more |= index + 1 < page.events.len();
                break;
            }
        }
        if let Some(error) = retry {
            if attempt == 2 {
                return Err(format!("the current event could not be read after three page reads: {error}"));
            }
            page = walk::page(&reader, from.clone()).await?;
            continue;
        }
        return Ok(Scanned {
            events,
            truncated,
            restart,
            next: if more {
                after.map(|after| Cursor {
                    revision: page.revision.unwrap_or(revision).to_string(),
                    after,
                    until,
                    through,
                    seeking: false,
                    restarts,
                })
            } else {
                None
            },
        });
    }
    unreachable!("bounded export attempts return")
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
