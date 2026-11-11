//! Independent bounded reads for export and search. No call changes the visible window.

use crate::timeline_watch::Shared;
use fastctx_types::timeline_pages::{PAGE_BYTES, PAGE_EVENTS, TimelineAsk, TimelineEvent, TimelineFrom, TimelineTold};

pub(crate) struct Page {
    pub revision: Option<u64>,
    pub through: Option<u64>,
    pub events: Vec<TimelineEvent>,
    pub older: bool,
    pub newer: bool,
}

pub(crate) async fn ask(reader: &Shared, request: TimelineAsk) -> Result<TimelineTold, String> {
    let (world, machine) = {
        let r = reader.lock().await;
        (r.world.clone(), r.machine.clone())
    };
    tokio::select! {
        response = tokio::time::timeout(std::time::Duration::from_secs(30), world.timeline(machine, request)) =>
            response.map_err(|_| "reading the timeline exceeded 30 s")?.map_err(|error| error.to_string()),
        () = reader.closed() => Err("the conversation window closed".into()),
    }
}

pub(crate) async fn page(reader: &Shared, from: TimelineFrom) -> Result<Page, String> {
    let resident = reader.lock().await.resident.clone();
    match ask(reader, TimelineAsk::Page { resident, from, max_events: PAGE_EVENTS, max_bytes: PAGE_BYTES }).await? {
        TimelineTold::Page { revision, through, events, older, newer } => {
            Ok(Page { revision, through, events, older: older.is_some(), newer: newer.is_some() })
        }
        _ => Err("the timeline did not answer a page".into()),
    }
}

pub(crate) async fn continuous(reader: &Shared, through: Option<u64>) -> Result<bool, String> {
    let Some(after) = through else { return Ok(true) };
    let resident = reader.lock().await.resident.clone();
    match ask(reader, TimelineAsk::Changes { resident, after, max_events: 1, max_bytes: PAGE_BYTES }).await? {
        TimelineTold::Reread => Ok(false),
        TimelineTold::Changes { .. } => Ok(true),
        _ => Err("the timeline could not validate this reading".into()),
    }
}
