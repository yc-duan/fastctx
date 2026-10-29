//! A history cut into segments named by their content.
//!
//! Where a segment ends depends only on what comes before that point, so a
//! history that grows keeps its earlier segments, and a change in the middle
//! makes new segments only around it: a segment ends before each turn
//! begins; after an event whose own
//! digest falls in one part in `SPREAD`, once it holds that much; and when it
//! reaches `LARGEST`. The second rule puts ends in the same places again
//! after a change, where a history has no turns the World saw.

use std::collections::HashSet;

use fastctx_types::session::held::{Held, Segment, SegmentId};
use fastctx_types::session::{Happened, SessionEvent, Watched};
use sha2::{Digest as _, Sha256};

/// Below this a segment does not end at a content-selected boundary.
const SMALLEST: usize = 64 * 1024;
/// Past this a segment ends, so that a change rarely sends much again.
const LARGEST: usize = 1024 * 1024;
/// One event in this many ends a segment that holds `SMALLEST` bytes.
const SPREAD: u8 = 32;
/// What a viewer that holds nothing gets first: the latest segments, until
/// they hold this many events or bytes, about a few screens of a
/// conversation.
const LATEST_EVENTS: usize = 200;
const LATEST_BYTES: usize = 256 * 1024;

/// A history cut into segments, in order.
pub struct Cut {
    pub segments: Vec<Segment>,
    /// The bytes of each segment's events as JSON.
    pub sizes: Vec<usize>,
}

impl Cut {
    pub fn layout(&self) -> Vec<SegmentId> {
        self.segments.iter().map(|segment| segment.id.clone()).collect()
    }
}

/// Cuts `events` into segments.
pub fn cut(events: Vec<SessionEvent>) -> Cut {
    let mut segments = Vec::new();
    let mut sizes = Vec::new();
    let mut events_here: Vec<SessionEvent> = Vec::new();
    let mut digests = Sha256::new();
    let mut size = 0usize;
    let mut previous_started = false;
    let mut close = |events_here: &mut Vec<SessionEvent>, digests: &mut Sha256, size: &mut usize| {
        if events_here.is_empty() {
            return;
        }
        let digest: [u8; 32] = std::mem::take(digests).finalize().into();
        segments.push(Segment { id: SegmentId::of(&digest), events: std::mem::take(events_here) });
        sizes.push(*size);
        *size = 0;
    };
    for event in events {
        let starts = match &event.happened {
            Happened::TurnStarted { .. } => true,
            // Words with no turn the World saw before them begin one of the
            // host's own.
            Happened::Heard { .. } => !previous_started,
            _ => false,
        };
        previous_started = matches!(event.happened, Happened::TurnStarted { .. });
        if starts {
            close(&mut events_here, &mut digests, &mut size);
        }
        let bytes = serde_json::to_vec(&event).expect("session events serialize");
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        digests.update(digest);
        size += bytes.len();
        events_here.push(event);
        if size >= LARGEST || (size >= SMALLEST && u16::from(digest[0]) * u16::from(SPREAD) < 256) {
            close(&mut events_here, &mut digests, &mut size);
        }
    }
    close(&mut events_here, &mut digests, &mut size);
    Cut { segments, sizes }
}

/// What a viewer that holds `held` is sent of the history `cut`, named
/// `version`: nothing when it holds every segment in order; otherwise the
/// segments it lacks, the latest first when it holds none and asks for that.
pub fn pieces(cut: Cut, held: &Held, version: String) -> Vec<Watched> {
    bounded(collected(cut, held, version))
}

fn collected(cut: Cut, held: &Held, version: String) -> Vec<Watched> {
    let layout = cut.layout();
    if held.segments == layout {
        return vec![Watched::Unchanged { version }];
    }
    let have: HashSet<&SegmentId> = held.segments.iter().collect();
    let mut missing = Vec::new();
    let mut sizes = Vec::new();
    for (segment, size) in cut.segments.into_iter().zip(cut.sizes) {
        if !have.contains(&segment.id) {
            missing.push(segment);
            sizes.push(size);
        }
    }
    if !(held.segments.is_empty() && held.latest_first) {
        return vec![Watched::Segments { version, layout, segments: missing, complete: true }];
    }
    let (mut events, mut bytes, mut first) = (0, 0, missing.len());
    while first > 0 && events < LATEST_EVENTS && bytes < LATEST_BYTES {
        first -= 1;
        events += missing[first].events.len();
        bytes += sizes[first];
    }
    let boundaries = turn_boundaries(&missing);
    while first > 0 && !boundaries[first] {
        first -= 1;
    }
    if first == 0 {
        return vec![Watched::Segments { version, layout, segments: missing, complete: true }];
    }
    let latest = missing.split_off(first);
    vec![
        Watched::Segments { version: version.clone(), layout: layout.clone(), segments: latest, complete: false },
        Watched::Segments { version, layout, segments: missing, complete: true },
    ]
}

/// Splits a peer's older bundled segment response without changing its layout,
/// segment identities or completion boundary. Other watch pieces stay whole.
pub fn split(piece: Watched) -> Vec<Watched> {
    bounded(vec![piece])
}

/// A history is transferred incrementally even when nearly every segment is
/// missing. Complete remains false until the final piece of the original phase;
/// the newest part of a large initial tail is delivered first.
fn bounded(pieces: Vec<Watched>) -> Vec<Watched> {
    const BYTES: usize = 4 * 1024 * 1024;
    let mut result = Vec::new();
    for piece in pieces {
        let Watched::Segments { version, layout, segments, complete } = piece else { result.push(piece); continue; };
        let mut groups = Vec::new();
        let mut group = Vec::new();
        let mut size = 0_usize;
        for segment in segments {
            let bytes = crate::watch_queue::size(&segment);
            if !group.is_empty() && size.saturating_add(bytes) > BYTES {
                groups.push(std::mem::take(&mut group));
                size = 0;
            }
            size = size.saturating_add(bytes);
            group.push(segment);
        }
        if !group.is_empty() || groups.is_empty() { groups.push(group); }
        if !complete { groups.reverse(); }
        let last = groups.len() - 1;
        for (index, segments) in groups.into_iter().enumerate() {
            result.push(Watched::Segments { version: version.clone(), layout: layout.clone(), segments, complete: complete && index == last });
        }
    }
    result
}

/// Whether a segment begins before any words or items of its first turn.
/// Turnless words without a delivery or message identity share one ordinal
/// sequence, so their first occurrence is the only usable boundary.
pub fn turn_boundaries(segments: &[Segment]) -> Vec<bool> {
    let mut seen = HashSet::new();
    segments
        .iter()
        .map(|segment| {
            let mut boundary = false;
            for (at, event) in segment.events.iter().enumerate() {
                let turn = match &event.happened {
                    Happened::TurnStarted { turn }
                    | Happened::Began { turn, .. }
                    | Happened::Grew { turn, .. }
                    | Happened::Finished { turn, .. }
                    | Happened::Updated { turn, .. }
                    | Happened::TurnEnded { turn, .. }
                    | Happened::Steps { turn, .. } => Some(turn.clone()),
                    Happened::Heard { turn: None, delivery: Some(delivery), .. } => Some(format!("heard:{delivery}")),
                    Happened::Heard { turn: None, message: Some(message), .. } => Some(format!("heard:{}", message.id)),
                    Happened::Heard { turn, .. } => Some(turn.clone().unwrap_or_default()),
                    _ => None,
                };
                if let Some(turn) = turn {
                    let first = seen.insert(turn);
                    if at == 0 {
                        boundary = first;
                    }
                }
            }
            boundary
        })
        .collect()
}
