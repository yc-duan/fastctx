//! Live-only readers pair original events with their later materialized previews.

use fastctx_types::{
    Address,
    session::{Mark, SessionEvent, Watched},
};

pub(super) fn watched(
    event: SessionEvent,
    mark: Option<Mark>,
    slim: Option<&Address>,
    live_only: bool,
) -> Option<Watched> {
    let likeness = if live_only {
        serde_json::to_vec(&event).ok().map(|encoded| crate::timeline::likeness(&event, &encoded))
    } else {
        None
    };
    super::live(event, slim).map(|event| Watched::Live { event, mark, likeness: likeness.map(Box::new) })
}
