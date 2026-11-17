//! Recover a missing running prefix once, without reopening the conversation.

use crate::{Piece, timeline_watch::Shared};
use fastctx_types::{
    session::{Happened, Item, Mark, SessionEvent, What},
    timeline_pages::{TimelineAsk, TimelineTold},
};

pub(crate) fn pending(event: &SessionEvent) -> bool {
    matches!(&event.happened, Happened::Began { item, .. } | Happened::Updated { item, .. }
        if matches!(&item.what, What::Other { host_kind } if host_kind == "fastctx:running-content-pending"))
}
pub(crate) fn active(event: &SessionEvent) -> bool {
    matches!(event.happened, Happened::Began { .. } | Happened::Updated { .. })
}

/// The snapshot's mark covers only this item; it must not skip other items' events.
pub(crate) async fn recover(reader: &Shared, event: &SessionEvent, mark: Option<Mark>) -> Option<Piece> {
    let Happened::Grew { turn, item, .. } = &event.happened else { return None };
    let key = (turn.clone(), item.clone());
    let request = {
        let mut current = reader.lock().await;
        if let Some(covered) = current.running_reads.get(&key) {
            if covered.is_some_and(|covered| {
                mark.is_some_and(|mark| mark.epoch == covered.epoch && mark.sequence <= covered.sequence)
            }) {
                return None;
            }
            None
        } else if current.state.running(event).is_none() {
            current.running_reads.insert(key.clone(), None);
            Some((current.world.clone(), current.machine.clone(), current.resident.clone()))
        } else {
            None
        }
    };
    let (world, machine, resident) = request?;
    let response = tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(30), world.timeline(machine, TimelineAsk::Running { resident, turn: turn.clone(), item: item.clone() })) => result.ok().and_then(Result::ok),
        () = reader.closed() => return None,
    };
    let (snapshot, covered) = match response {
        Some(TimelineTold::Running { event, mark }) => (event.map(|event| *event).filter(|snapshot| matches!(&snapshot.happened, Happened::Began { turn: t, item: i } | Happened::Updated { turn: t, item: i } if t == turn && i.id == *item)), mark),
        _ => (None, None),
    };
    let mut current = reader.lock().await;
    current.running_reads.insert(key, covered);
    let mut snapshot = snapshot.unwrap_or_else(|| SessionEvent {
        at: event.at,
        happened: Happened::Began {
            turn: turn.clone(),
            item: Item {
                id: item.clone(),
                draft: None,
                started_at: Some(event.at),
                detail: None,
                what: What::Other { host_kind: "fastctx:running-content-pending".into() },
            },
        },
    });
    if let Happened::Began { turn, item } = snapshot.happened {
        snapshot.happened = Happened::Updated { turn, item };
    }
    current.live(snapshot, None, None).ok().flatten()
}

pub(crate) fn covered(reader: &crate::timeline_watch::Reader, event: &SessionEvent, mark: Option<Mark>) -> bool {
    let Happened::Grew { turn, item, .. } = &event.happened else { return false };
    reader.running_reads.get(&(turn.clone(), item.clone())).is_some_and(|covered| {
        covered.is_some_and(|covered| {
            mark.is_some_and(|mark| mark.epoch == covered.epoch && mark.sequence <= covered.sequence)
        })
    }) || reader.state.running(event).is_some_and(|event| pending(&event))
}
