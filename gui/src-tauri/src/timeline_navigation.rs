//! Bounded navigation and activity edges, independent of live delivery.

use super::{Page, Reader, event_turn};
use crate::timeline_window;
use fastctx_types::timeline_pages::{PAGE_EVENTS, TimelineEvent, TimelineFrom, TimelinePlace};

impl Reader {
    pub(super) async fn complete_edge(&self, page: &mut Page, backwards: bool) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let bytes =
                page.events.iter().map(|event| serde_json::to_vec(event).map_or(0, |bytes| bytes.len())).sum::<usize>();
            if page.events.len() >= timeline_window::HELD_EVENTS
                || bytes >= timeline_window::HELD_BYTES
                || page
                    .events
                    .iter()
                    .any(|event| crate::timeline_groups::boundary(timeline_window::decoded(event).as_ref()))
            {
                return Ok(());
            }
            let next = if backwards {
                page.older.as_ref().and(page.events.first())
            } else {
                page.newer.as_ref().and(page.events.last())
            };
            let Some(next) = next else { return Ok(()) };
            let from = if backwards {
                TimelineFrom::Before { place: next.place.clone() }
            } else {
                TimelineFrom::After { place: next.place.clone() }
            };
            let mut additional = tokio::time::timeout_at(
                deadline,
                self.fetch_count(
                    from,
                    (timeline_window::HELD_EVENTS - page.events.len()).min(PAGE_EVENTS as usize) as u32,
                ),
            )
            .await
            .map_err(|_| "reading the activity boundary exceeded 30 s")??;
            if additional.revision != page.revision {
                return Err("the timeline changed while its activity boundary was being read".into());
            }
            if additional.events.is_empty() {
                return Ok(());
            }
            if backwards {
                if let Some(at) = additional
                    .events
                    .iter()
                    .rposition(|event| crate::timeline_groups::boundary(timeline_window::decoded(event).as_ref()))
                    && at > 0
                {
                    additional.older = Some(additional.events[at - 1].place.clone());
                    additional.events.drain(..at);
                }
                page.older = additional.older;
                additional.events.append(&mut page.events);
                page.events = additional.events;
            } else {
                page.newer = additional.newer;
                page.events.append(&mut additional.events);
            }
        }
    }
    pub(super) async fn read_turn(&mut self, turn: &str) -> Result<bool, String> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut from = TimelineFrom::Latest;
        loop {
            let page = tokio::time::timeout_at(deadline, self.fetch(from))
                .await
                .map_err(|_| "finding the requested turn exceeded 30 s; try again")??;
            if let Some(event) = page
                .events
                .iter()
                .rev()
                .find(|event| timeline_window::decoded(event).as_ref().and_then(event_turn) == Some(turn))
            {
                return self.read_position(event.place.clone()).await;
            }
            if page.older.is_none() {
                return Err("the requested turn is not in the currently readable history".into());
            }
            let Some(first) = page.events.first() else {
                return Err("the timeline did not advance while finding the requested turn".into());
            };
            from = TimelineFrom::Before { place: first.place.clone() };
        }
    }
    pub(super) async fn read_position(&mut self, place: TimelinePlace) -> Result<bool, String> {
        let before = self.fetch(TimelineFrom::Before { place: place.clone() }).await?;
        let after = match before.events.last() {
            Some(previous) => self.fetch(TimelineFrom::After { place: previous.place.clone() }).await?,
            None => {
                let following = self.fetch_count(TimelineFrom::After { place: place.clone() }, 1).await?;
                match following.events.first() {
                    Some(next) => self.fetch(TimelineFrom::Before { place: next.place.clone() }).await?,
                    None => self.fetch(TimelineFrom::Latest).await?,
                }
            }
        };
        if before.revision != after.revision {
            return Err("the timeline moved while the requested position was being read; search it again".into());
        }
        let Some(revision) = after.revision else {
            return Err("the conversation's history is still being prepared".into());
        };
        let mut page = Page {
            revision: after.revision,
            initializing: after.initializing,
            through: after.through,
            events: before.events,
            older: before.older,
            newer: after.newer,
        };
        page.events.extend(after.events);
        self.focus_edges(&mut page, &place).await?;
        self.state.preparing = page.initializing.unwrap_or(false);
        let events = page.events;
        if !events.iter().any(|event| event.place.item == place.item) {
            return Err("the requested position is not in this reading; search it again".into());
        }
        self.following = false;
        self.through = page.through;
        self.catch_up = false;
        self.state.interval_start = None;
        self.state.interval_end = None;
        self.state.right = None;
        let at = events.iter().position(|event| event.place.item == place.item).expect("target checked");
        let partial_left = page.older.is_some()
            && !events[..=at]
                .iter()
                .any(|event| crate::timeline_groups::boundary(timeline_window::decoded(event).as_ref()));
        let partial_right = page.newer.is_some()
            && !events[at + 1..]
                .iter()
                .any(|event| crate::timeline_groups::boundary(timeline_window::decoded(event).as_ref()));
        self.state.replace(events, revision, page.older.is_some(), page.newer.is_some());
        self.state.oversized_left = partial_left;
        self.state.oversized_right = partial_right;
        if self.state.newer
            && let Some(last) = self.state.last()
        {
            let right = self.fetch_count(TimelineFrom::After { place: last }, 1).await?;
            if right.revision != Some(revision) {
                return Err("the timeline moved while the requested edge was being read; search it again".into());
            }
            self.state.right_complete = right
                .events
                .first()
                .is_some_and(|event| crate::timeline_groups::boundary(timeline_window::decoded(event).as_ref()));
            self.state.right = right.events.first().map(|event| event.place.clone());
        }
        Ok(true)
    }
    pub(super) async fn focus_edges(&self, page: &mut Page, target: &TimelinePlace) -> Result<(), String> {
        for _ in 0..4 {
            let Some(at) = page.events.iter().position(|event| event.place.item == target.item) else { return Ok(()) };
            let boundary =
                |event: &TimelineEvent| crate::timeline_groups::boundary(timeline_window::decoded(event).as_ref());
            let left = page.events[..=at].iter().rposition(&boundary);
            let right = page.events[at + 1..].iter().position(&boundary).map(|index| index + at + 1);
            let backwards = left.is_none() && page.older.is_some() && right.is_some();
            let forwards = right.is_none() && page.newer.is_some() && left.is_some();
            if !backwards && !forwards {
                return Ok(());
            }
            if backwards {
                let keep = right.expect("right edge") + 1;
                if keep < page.events.len() {
                    page.newer = Some(page.events[keep].place.clone());
                    page.events.truncate(keep);
                }
            } else {
                let discard = left.expect("left edge");
                if discard > 0 {
                    page.older = Some(page.events[discard - 1].place.clone());
                    page.events.drain(..discard);
                }
            }
            let room = timeline_window::HELD_EVENTS.saturating_sub(page.events.len()).min(PAGE_EVENTS as usize);
            if room == 0 {
                return Ok(());
            }
            let anchor = if backwards { &page.events[0] } else { page.events.last().expect("target held") };
            let from = if backwards {
                TimelineFrom::Before { place: anchor.place.clone() }
            } else {
                TimelineFrom::After { place: anchor.place.clone() }
            };
            let mut additional = self.fetch_count(from, room as u32).await?;
            if additional.revision != page.revision {
                return Err("the timeline moved while the requested group was being read; search it again".into());
            }
            if additional.events.is_empty() {
                return Ok(());
            }
            if backwards {
                if let Some(at) = additional
                    .events
                    .iter()
                    .rposition(|event| crate::timeline_groups::boundary(timeline_window::decoded(event).as_ref()))
                    && at > 0
                {
                    additional.older = Some(additional.events[at - 1].place.clone());
                    additional.events.drain(..at);
                }
                page.older = additional.older;
                additional.events.append(&mut page.events);
                page.events = additional.events;
            } else {
                page.newer = additional.newer;
                page.events.append(&mut additional.events);
            }
        }
        Ok(())
    }
}
