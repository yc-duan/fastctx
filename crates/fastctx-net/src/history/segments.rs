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
    let mut cutter = Cutter::default();
    for event in events {
        cutter.push(event);
    }
    cutter.finish()
}

/// `cut` an event at a time, for a history read a page at a time rather
/// than held (design §28.52 52-1): where each segment ends depends only on
/// what comes before it, so the segments are the same.
#[derive(Default)]
pub struct Cutter {
    segmenter: Segmenter,
    segments: Vec<Segment>,
    sizes: Vec<usize>,
}

impl Cutter {
    /// The next event of the history.
    pub fn push(&mut self, event: SessionEvent) {
        for (segment, size) in self.segmenter.push(event).into_iter().flatten() {
            self.segments.push(segment);
            self.sizes.push(size);
        }
    }

    /// The history cut, its last segment ended.
    pub fn finish(mut self) -> Cut {
        if let Some((segment, size)) = self.segmenter.finish() {
            self.segments.push(segment);
            self.sizes.push(size);
        }
        Cut { segments: self.segments, sizes: self.sizes }
    }
}

/// `cut` an event at a time, each segment handed back as it ends with its
/// bytes as JSON, for it to be kept elsewhere than in memory.
#[derive(Default)]
pub struct Segmenter {
    events_here: Vec<SessionEvent>,
    digests: Sha256,
    size: usize,
    previous_started: bool,
}

impl Segmenter {
    /// The next event of the history: the segments it ends, before it and
    /// with it.
    pub fn push(&mut self, event: SessionEvent) -> [Option<(Segment, usize)>; 2] {
        let starts = match &event.happened {
            Happened::TurnStarted { .. } => true,
            // Words with no turn the World saw before them begin one of the
            // host's own.
            Happened::Heard { .. } => !self.previous_started,
            _ => false,
        };
        self.previous_started = matches!(event.happened, Happened::TurnStarted { .. });
        let before = if starts { self.close() } else { None };
        let bytes = serde_json::to_vec(&event).expect("session events serialize");
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        self.digests.update(digest);
        self.size += bytes.len();
        self.events_here.push(event);
        let with = if self.size >= LARGEST || (self.size >= SMALLEST && u16::from(digest[0]) * u16::from(SPREAD) < 256) {
            self.close()
        } else {
            None
        };
        [before, with]
    }

    /// The last segment, ended.
    pub fn finish(&mut self) -> Option<(Segment, usize)> {
        self.close()
    }

    fn close(&mut self) -> Option<(Segment, usize)> {
        if self.events_here.is_empty() {
            return None;
        }
        let digest: [u8; 32] = std::mem::take(&mut self.digests).finalize().into();
        let segment = Segment { id: SegmentId::of(&digest), events: std::mem::take(&mut self.events_here) };
        Some((segment, std::mem::take(&mut self.size)))
    }
}

/// A cut in outline: what `pieces` needs of it but its events — each
/// segment's name, its bytes as JSON, how many events it holds, what it
/// takes in a watch's queue, and whether it begins a turn none before it
/// began (`turn_boundaries`, over every segment from the first).
#[derive(Clone, Debug, Default)]
pub struct Outline {
    pub ids: Vec<SegmentId>,
    pub sizes: Vec<usize>,
    pub counts: Vec<usize>,
    pub queued: Vec<usize>,
    pub boundaries: Vec<bool>,
    seen: HashSet<String>,
}

impl Outline {
    /// The next segment of the cut, of `size` bytes as JSON.
    pub fn add(&mut self, segment: &Segment, size: usize) {
        let mut boundary = false;
        for (at, event) in segment.events.iter().enumerate() {
            if let Some(turn) = turn_of(event) {
                let first = self.seen.insert(turn);
                if at == 0 {
                    boundary = first;
                }
            }
        }
        self.ids.push(segment.id.clone());
        self.sizes.push(size);
        self.counts.push(segment.events.len());
        self.queued.push(crate::watch_queue::size(segment));
        self.boundaries.push(boundary);
    }

    /// The outline of `cut`.
    pub fn of(cut: &Cut) -> Self {
        let mut outline = Self::default();
        for (segment, size) in cut.segments.iter().zip(&cut.sizes) {
            outline.add(segment, *size);
        }
        outline
    }

    /// The cut whole: what only its making needed goes.
    pub fn seal(&mut self) {
        self.seen = HashSet::new();
    }

    /// The bytes it holds in memory, about.
    pub fn bytes(&self) -> usize {
        self.ids.len() * (std::mem::size_of::<SegmentId>() + 64 + 4 * std::mem::size_of::<usize>())
            + self.seen.iter().map(|turn| turn.len() + 32).sum::<usize>()
    }
}

/// A piece of what a viewer is sent (`pieces`), its segments named by where
/// they are in the cut, to be read as it is sent.
#[derive(Clone, Debug)]
pub enum Planned {
    Made(Watched),
    Segments { version: String, layout: std::sync::Arc<Vec<SegmentId>>, segments: Vec<usize>, complete: bool },
}

/// `pieces` of a cut in outline: the same pieces, in the same order, each
/// segment named by where it is in the cut.
pub fn plan(outline: &Outline, held: &Held, version: String) -> Vec<Planned> {
    if held.segments == outline.ids {
        return vec![Planned::Made(Watched::Unchanged { version })];
    }
    let layout = std::sync::Arc::new(outline.ids.clone());
    let have: HashSet<&SegmentId> = held.segments.iter().collect();
    let mut missing: Vec<usize> = (0..outline.ids.len()).filter(|&at| !have.contains(&outline.ids[at])).collect();
    let phases = if !(held.segments.is_empty() && held.latest_first) {
        vec![(missing, true)]
    } else {
        // Holding none, every segment is missing, in order: the boundaries
        // over them are the outline's.
        let (mut events, mut bytes, mut first) = (0, 0, missing.len());
        while first > 0 && events < LATEST_EVENTS && bytes < LATEST_BYTES {
            first -= 1;
            events += outline.counts[missing[first]];
            bytes += outline.sizes[missing[first]];
        }
        while first > 0 && !outline.boundaries[missing[first]] {
            first -= 1;
        }
        if first == 0 {
            vec![(missing, true)]
        } else {
            let latest = missing.split_off(first);
            vec![(latest, false), (missing, true)]
        }
    };
    let mut planned = Vec::new();
    for (segments, complete) in phases {
        let mut groups = Vec::new();
        let mut group = Vec::new();
        let mut size = 0_usize;
        for at in segments {
            let bytes = outline.queued[at];
            if !group.is_empty() && size.saturating_add(bytes) > PIECE_BYTES {
                groups.push(std::mem::take(&mut group));
                size = 0;
            }
            size = size.saturating_add(bytes);
            group.push(at);
        }
        if !group.is_empty() || groups.is_empty() {
            groups.push(group);
        }
        if !complete {
            groups.reverse();
        }
        let last = groups.len() - 1;
        for (index, segments) in groups.into_iter().enumerate() {
            planned.push(Planned::Segments {
                version: version.clone(),
                layout: layout.clone(),
                segments,
                complete: complete && index == last,
            });
        }
    }
    planned
}

/// The turn an event tells of, as `turn_boundaries` names it.
fn turn_of(event: &SessionEvent) -> Option<String> {
    match &event.happened {
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
    }
}

/// The most bytes of segments one piece takes (`bounded`).
const PIECE_BYTES: usize = 4 * 1024 * 1024;

/// What a viewer that holds `held` is sent of the history `cut`, named
/// `version`: nothing when it holds every segment in order; otherwise the
/// segments it lacks, the latest first when it holds none and asks for that.
pub fn pieces(cut: &Cut, held: &Held, version: String) -> Vec<Watched> {
    bounded(collected(cut, held, version))
}

fn collected(cut: &Cut, held: &Held, version: String) -> Vec<Watched> {
    let layout = cut.layout();
    if held.segments == layout {
        return vec![Watched::Unchanged { version }];
    }
    let have: HashSet<&SegmentId> = held.segments.iter().collect();
    let mut missing = Vec::new();
    let mut sizes = Vec::new();
    for (segment, size) in cut.segments.iter().zip(&cut.sizes) {
        if !have.contains(&segment.id) {
            missing.push(segment.clone());
            sizes.push(*size);
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
    const BYTES: usize = PIECE_BYTES;
    let mut result = Vec::new();
    for piece in pieces {
        let Watched::Segments { version, layout, segments, complete } = piece else {
            result.push(piece);
            continue;
        };
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
        if !group.is_empty() || groups.is_empty() {
            groups.push(group);
        }
        if !complete {
            groups.reverse();
        }
        let last = groups.len() - 1;
        for (index, segments) in groups.into_iter().enumerate() {
            result.push(Watched::Segments {
                version: version.clone(),
                layout: layout.clone(),
                segments,
                complete: complete && index == last,
            });
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
                if let Some(turn) = turn_of(event) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::session::{Health, HealthKind};

    struct Choices(u64);
    impl Choices {
        fn pick(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    fn event(choices: &mut Choices, at: i64) -> SessionEvent {
        let at = jiff::Timestamp::from_second(1_800_000_000 + at).unwrap();
        let turn = format!("t{}", choices.pick(6));
        let happened = match choices.pick(4) {
            0 => Happened::TurnStarted { turn },
            1 => Happened::Heard {
                delivery: None,
                turn: (choices.pick(2) == 0).then_some(turn),
                from: None,
                text: "w".repeat(choices.pick(3) as usize * 40_000),
                message: None,
                notice: None,
            },
            _ => Happened::Health { turn: Some(turn), health: Health { kind: HealthKind::Warning, text: "n".repeat(choices.pick(2) as usize * 30_000) } },
        };
        SessionEvent { at, happened }
    }

    /// A cut planned in outline and read as it is sent is the pieces of the
    /// cut held, piece for piece, for viewers that hold none, some or all
    /// of it, latest first or not; and the cutter handing segments back cuts
    /// as the cut does.
    #[test]
    fn pieces_planned_in_outline_are_the_pieces_of_the_cut() {
        let mut choices = Choices(0x2545_f491_4f6c_dd1d);
        for round in 0..60 {
            let count = choices.pick(300) as usize;
            let events: Vec<SessionEvent> = (0..count).map(|at| event(&mut choices, at as i64)).collect();
            let cut = cut(events.clone());
            let mut segmenter = Segmenter::default();
            let mut handed = Vec::new();
            for event in events {
                handed.extend(segmenter.push(event).into_iter().flatten());
            }
            handed.extend(segmenter.finish());
            assert_eq!(handed.iter().map(|(segment, size)| (segment.clone(), *size)).collect::<Vec<_>>(),
                cut.segments.iter().cloned().zip(cut.sizes.iter().copied()).collect::<Vec<_>>(), "round {round}");
            let outline = Outline::of(&cut);
            assert_eq!(outline.boundaries, turn_boundaries(&cut.segments));
            let layout = cut.layout();
            let mut helds = vec![
                Held { segments: Vec::new(), latest_first: true, ..Held::default() },
                Held { segments: Vec::new(), latest_first: false, ..Held::default() },
                Held { segments: layout.clone(), ..Held::default() },
            ];
            let some: Vec<SegmentId> = layout.iter().filter(|_| choices.pick(2) == 0).cloned().collect();
            helds.push(Held { segments: some, latest_first: choices.pick(2) == 0, ..Held::default() });
            for held in helds {
                let pieces = pieces(&cut, &held, "v".into());
                let planned: Vec<Watched> = plan(&outline, &held, "v".into())
                    .into_iter()
                    .map(|planned| match planned {
                        Planned::Made(watched) => watched,
                        Planned::Segments { version, layout, segments, complete } => Watched::Segments {
                            version,
                            layout: (*layout).clone(),
                            segments: segments.into_iter().map(|at| cut.segments[at].clone()).collect(),
                            complete,
                        },
                    })
                    .collect();
                assert_eq!(serde_json::to_string(&planned).unwrap(), serde_json::to_string(&pieces).unwrap(), "round {round}");
            }
        }
    }
}
