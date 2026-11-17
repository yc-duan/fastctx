//! A cancellable full-history search streams one bounded batch to its renderer.

use crate::{
    timeline_search_index::{self as index, Count, Index},
    timeline_walk as walk,
    timeline_watch::Shared,
};
use fastctx_types::timeline_pages::{
    PAGE_BYTES, PAGE_EVENTS, TimelineAsk, TimelineChange, TimelineEvent, TimelineFrom, TimelinePlace, TimelineShown,
    TimelineTold,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Default)]
pub(crate) struct Searches {
    next: AtomicU64,
    current: Mutex<Option<Arc<Search>>>,
}
struct Search {
    token: u64,
    /// The events whose text the page holds already, by item and sequence:
    /// told by place alone, their bodies neither read nor sent again.
    known: HashSet<(String, u64)>,
    cancelled: AtomicBool,
    cancelling: tokio::sync::Notify,
    state: tokio::sync::Mutex<State>,
    index: Arc<Mutex<Index>>,
}
struct State {
    from: Option<TimelineFrom>,
    through: Option<u64>,
    complete: bool,
    queued: VecDeque<TimelineEvent>,
    pending: Vec<(TimelinePlace, u64, bool)>,
}
#[derive(Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub(crate) enum Request {
    /// A search begun; `known`, the events whose text the page kept from
    /// one before, as they were then.
    Start {
        #[serde(default)]
        known: Vec<Known>,
    },
    Next { token: String, counts: Vec<u32> },
    Hit { token: String, ordinal: String, cursor: Option<index::Seeking> },
    Rank { token: String, item: String, ordinal: u32, cursor: Option<index::Ranking> },
    Close { token: String },
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Reply {
    Batch { token: String, events: Vec<Event>, total: String, examined: String, complete: bool, previews: String },
    Hit { hit: index::Hit },
    Rank { rank: index::Rank },
    Closed,
}
#[derive(Serialize)]
pub(crate) struct Event {
    place: TimelinePlace,
    /// Which version of the event this is: the page keeps its text by it.
    sequence: String,
    /// None for an event the page said it holds as it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<fastctx_types::session::SessionEvent>,
}

/// An event whose text the page holds, as of its sequence.
#[derive(Deserialize)]
pub(crate) struct Known {
    item: String,
    sequence: String,
}

impl Searches {
    pub fn close(&self) {
        if let Some(search) = self.current.lock().expect("search table").take() {
            search.cancelled.store(true, Ordering::Release);
            search.cancelling.notify_waiters();
        }
    }
    fn get(&self, token: &str) -> Result<Arc<Search>, String> {
        self.current
            .lock()
            .expect("search table")
            .as_ref()
            .filter(|search| search.token.to_string() == token && !search.cancelled.load(Ordering::Acquire))
            .cloned()
            .ok_or_else(|| "this search was cancelled".into())
    }
    async fn start(&self, known: Vec<Known>) -> Result<Arc<Search>, String> {
        self.close();
        let token = self.next.fetch_add(1, Ordering::AcqRel) + 1;
        let index = tokio::task::spawn_blocking(Index::new).await.map_err(|e| e.to_string())??;
        let known = known
            .into_iter()
            .filter_map(|known| Some((known.item, known.sequence.parse().ok()?)))
            .collect();
        let search = Arc::new(Search {
            token,
            known,
            cancelled: AtomicBool::new(false),
            cancelling: tokio::sync::Notify::new(),
            index: Arc::new(Mutex::new(index)),
            state: tokio::sync::Mutex::new(State {
                from: Some(TimelineFrom::Latest),
                through: None,
                complete: false,
                queued: VecDeque::new(),
                pending: Vec::new(),
            }),
        });
        let mut current = self.current.lock().expect("search table");
        if self.next.load(Ordering::Acquire) != token {
            return Err("this search was cancelled".into());
        }
        current.replace(search.clone());
        Ok(search)
    }
    pub async fn request(&self, reader: Shared, request: Request) -> Result<Reply, String> {
        match request {
            Request::Start { known } => {
                let search = self.start(known).await?;
                Ok(Reply::Batch {
                    token: search.token.to_string(),
                    events: Vec::new(),
                    total: "0".into(),
                    examined: "0".into(),
                    complete: false,
                    previews: "0".into(),
                })
            }
            Request::Next { token, counts } => self.get(&token)?.deliver(reader, counts).await,
            Request::Hit { token, ordinal, cursor } => {
                let search = self.get(&token)?;
                let ordinal = ordinal.parse().map_err(|_| "invalid search ordinal")?;
                let index = search.index.clone();
                let hit = tokio::task::spawn_blocking(move || index.lock().expect("search index").hit(ordinal, cursor))
                    .await
                    .map_err(|e| e.to_string())??;
                Ok(Reply::Hit { hit })
            }
            Request::Rank { token, item, ordinal, cursor } => {
                let search = self.get(&token)?;
                let index = search.index.clone();
                let rank = tokio::task::spawn_blocking(move || {
                    index.lock().expect("search index").rank(&item, ordinal, cursor)
                })
                .await
                .map_err(|e| e.to_string())??;
                Ok(Reply::Rank { rank })
            }
            Request::Close { token } => {
                if self.get(&token).is_ok() {
                    self.close();
                }
                Ok(Reply::Closed)
            }
        }
    }
}
impl Search {
    async fn until_cancelled(&self) {
        let notified = self.cancelling.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.cancelled.load(Ordering::Acquire) {
            notified.await;
        }
    }
    async fn deliver(&self, reader: Shared, counts: Vec<u32>) -> Result<Reply, String> {
        tokio::select! { reply=self.batch(reader,counts)=>reply, ()=self.until_cancelled()=>Err("this search was cancelled".into()) }
    }
    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) { Err("this search was cancelled".into()) } else { Ok(()) }
    }
    async fn apply(&self, events: Vec<Count>) -> Result<(), String> {
        let index = self.index.clone();
        let result = tokio::task::spawn_blocking(move || index.lock().expect("search index").apply(events))
            .await
            .map_err(|e| e.to_string())?;
        if result.is_err() {
            self.cancelled.store(true, Ordering::Release);
        }
        result
    }
    async fn batch(&self, reader: Shared, counts: Vec<u32>) -> Result<Reply, String> {
        self.check()?;
        let mut state = self.state.lock().await;
        if counts.len() != state.pending.len() {
            return Err("the search counts do not match its pending batch".into());
        }
        let counts = state
            .pending
            .iter()
            .cloned()
            .zip(counts)
            .map(|((place, sequence, preview), count)| Count { place, sequence, count, preview })
            .collect();
        self.apply(counts).await?;
        state.pending.clear();
        self.check()?;
        if state.queued.is_empty() {
            if let Some(from) = state.from.clone() {
                let page = walk::page(&reader, from).await?;
                if page.initializing == Some(true) || page.revision.is_none() {
                    return Err("the history is still being prepared".into());
                }
                if state.through.is_none() {
                    state.through = page.through;
                }
                state.from = if page.older {
                    page.events.first().map(|event| TimelineFrom::Before { place: event.place.clone() })
                } else {
                    None
                };
                state.queued.extend(page.events);
                state.complete = false;
            } else {
                let resident = reader.lock().await.resident.clone();
                let after = state.through.ok_or("this timeline cannot resume a full-history search")?;
                match walk::ask(
                    &reader,
                    TimelineAsk::Changes { resident, after, max_events: PAGE_EVENTS, max_bytes: PAGE_BYTES },
                )
                .await?
                {
                    TimelineTold::Changes { changes, through, more, .. } => {
                        let mut gone = Vec::new();
                        for (offset, change) in changes.into_iter().enumerate() {
                            match change {
                                TimelineChange::Is { event } => state.queued.push_back(event),
                                TimelineChange::Gone { item } => gone.push(Count {
                                    place: TimelinePlace { order: String::new(), item },
                                    sequence: after + offset as u64,
                                    count: 0,
                                    preview: false,
                                }),
                            }
                        }
                        self.apply(gone).await?;
                        state.through = Some(through);
                        state.complete = !more;
                    }
                    TimelineTold::Reread => return Err("the history was rebuilt; restart this search".into()),
                    _ => return Err("the timeline changes are not available for search".into()),
                }
            }
        }
        let mut events = Vec::new();
        let mut bytes = 0;
        let mut pending = Vec::new();
        while let Some(event) = state.queued.get(events.len()).cloned() {
            self.check()?;
            let sequence = event.sequence.to_string();
            if self.known.contains(&(event.place.item.clone(), event.sequence)) {
                // Its text is the page's already: told by place, as it would
                // be counted from a preview or a whole body alike.
                let preview = matches!(&event.shown, TimelineShown::Oversized { bytes, .. } if *bytes > crate::timeline_body::EXPANDED_BYTES);
                bytes += event.place.item.len() + event.place.order.len() + 64;
                pending.push((event.place.clone(), event.sequence, preview));
                events.push(Event { place: event.place, sequence, event: None });
            } else {
                let (body, preview) = crate::timeline_scan::full(&reader, &event).await?;
                bytes += serde_json::to_vec(&body).map_or(0, |bytes| bytes.len());
                pending.push((event.place.clone(), event.sequence, preview.is_some()));
                events.push(Event { place: event.place, sequence, event: Some(body) });
            }
            if bytes >= PAGE_BYTES as usize {
                break;
            }
        }
        state.queued.drain(..events.len());
        state.pending = pending;
        let (total, examined, previews) = self.index.lock().expect("search index").stats();
        Ok(Reply::Batch {
            token: self.token.to_string(),
            events,
            total: total.to_string(),
            examined: examined.to_string(),
            complete: state.complete && state.queued.is_empty() && state.pending.is_empty(),
            previews: previews.to_string(),
        })
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn conversation_search(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    request: Request,
) -> Result<Reply, String> {
    let reader = shell.timeline_readers.get(id).ok_or("the conversation is no longer watched")?;
    reader.searches.request(reader.clone(), request).await
}
