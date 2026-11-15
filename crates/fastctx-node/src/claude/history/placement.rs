//! Places host events in recorded turns without scanning all turns per event,
//! as a decision on what each event shows of itself (`Spot`) and its
//! application to each event alone (`finish`): a history kept on disk
//! (design §28.51) is placed by the same decision as one held whole.

use super::hidden;
use fastctx_hosts::claude;
use fastctx_types::session::{Happened, SessionEvent};
use jiff::Timestamp;
use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};

/// What placing looks at of an event.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Spot {
    pub(crate) at: Timestamp,
    /// What the host keeps out of its conversation (`super::hidden`).
    pub(crate) hidden: bool,
    pub(crate) heard: bool,
}

pub(crate) fn spot(event: &SessionEvent) -> Spot {
    Spot { at: event.at, hidden: hidden(event), heard: matches!(event.happened, Happened::Heard { .. }) }
}

/// The World's turns, by when they started, each with when it ended; and the
/// words it heard in each: borrowed from the World's record, not copied —
/// or, of a record kept on disk, its turns held and its words asked of it
/// (`timeline::world`).
pub(crate) struct Turns<'a> {
    turns: Vec<(Timestamp, Option<Timestamp>, Cow<'a, str>)>,
    heard: Heard<'a>,
}

/// The words the World heard in each turn: borrowed, or asked of where they
/// are kept (a turn and words: whether they were heard in it).
enum Heard<'a> {
    Held(HashMap<&'a str, Vec<&'a str>>),
    Kept(&'a dyn Fn(&str, &str) -> bool),
}

impl<'a> Turns<'a> {
    /// As `of` makes them, of the starts of turns (`turns`, in the order the
    /// record tells them, each with its end) and the words asked of `heard`.
    pub(crate) fn kept(turns: Vec<(Timestamp, Option<Timestamp>, String)>, heard: &'a dyn Fn(&str, &str) -> bool) -> Self {
        let mut turns: Vec<_> = turns.into_iter().map(|(start, end, turn)| (start, end, Cow::Owned(turn))).collect();
        turns.sort_by_key(|(start, ..)| *start);
        Self { turns, heard: Heard::Kept(heard) }
    }

    /// Each turn by when it started: when, when it ended, its name.
    #[cfg(test)]
    pub(crate) fn spans(&self) -> Vec<(Timestamp, Option<Timestamp>, String)> {
        self.turns.iter().map(|(start, end, turn)| (*start, *end, turn.to_string())).collect()
    }

    /// Whether the World heard `text` in `turn`.
    pub(crate) fn heard(&self, turn: &str, text: &str) -> bool {
        match &self.heard {
            Heard::Held(heard) => heard.get(turn).is_some_and(|said| said.contains(&text)),
            Heard::Kept(heard) => heard(turn, text),
        }
    }

    pub(crate) fn of(recorded: &'a [SessionEvent]) -> Self {
        let mut ends: HashMap<&str, Timestamp> = HashMap::new();
        let mut heard: HashMap<&str, Vec<&str>> = HashMap::new();
        for event in recorded {
            match &event.happened {
                Happened::TurnEnded { turn, .. } => {
                    ends.insert(turn, event.at);
                }
                Happened::Heard { turn: Some(turn), text, .. } => heard.entry(turn.as_str()).or_default().push(text.as_str()),
                _ => {}
            }
        }
        let mut turns = Vec::new();
        for event in recorded {
            if let Happened::TurnStarted { turn } = &event.happened {
                turns.push((event.at, ends.get(turn.as_str()).copied(), Cow::Borrowed(turn.as_str())));
            }
        }
        turns.sort_by_key(|(start, ..)| *start);
        Self { turns, heard: Heard::Held(heard) }
    }

    /// The name of the turn at `index`.
    pub(crate) fn name(&self, index: usize) -> &str {
        &self.turns[index].2
    }
}

/// What placing looks at of many events, in their order (`Spot`), held in
/// little: each one's time, and whether it is kept out of the conversation
/// or is words.
#[derive(Default)]
pub(crate) struct Spots {
    times: Vec<Timestamp>,
    flags: Vec<u8>,
}

const HIDDEN: u8 = 1;
const HEARD: u8 = 2;

impl Spots {
    pub(crate) fn push(&mut self, spot: Spot) {
        self.times.push(spot.at);
        self.flags.push(Self::flags(spot));
    }

    /// Puts `spot` in the place of the one numbered `index`.
    pub(crate) fn set(&mut self, index: usize, spot: Spot) {
        self.times[index] = spot.at;
        self.flags[index] = Self::flags(spot);
    }

    pub(crate) fn remove(&mut self, index: usize) {
        self.times.remove(index);
        self.flags.remove(index);
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.times.truncate(len);
        self.flags.truncate(len);
    }

    pub(crate) fn len(&self) -> usize {
        self.times.len()
    }

    fn flags(spot: Spot) -> u8 {
        (u8::from(spot.hidden) * HIDDEN) | (u8::from(spot.heard) * HEARD)
    }

    fn hidden(&self, index: usize) -> bool {
        self.flags[index] & HIDDEN != 0
    }

    fn heard(&self, index: usize) -> bool {
        self.flags[index] & HEARD != 0
    }
}

/// Where each event goes: the indices of `spots` in the order placed (by
/// time, those alike in time as they came), each with the index of the
/// World's turn (`Turns`) it is placed in: the turn running at its time, and
/// for words heard just before a turn started with nothing else shown
/// between, that turn, as they are what started it. Given one at a time, of
/// no more than the order of the events and the turns running.
pub(crate) fn decide<'a>(spots: &'a Spots, turns: &'a Turns<'_>) -> impl Iterator<Item = (usize, Option<usize>)> + 'a {
    let turns = &turns.turns;
    let count = spots.len();
    let mut order: Vec<u32> = (0..u32::try_from(count).expect("fewer events than numbers")).collect();
    order.sort_by_key(|index| spots.times[*index as usize]);
    let mut started = 0;
    let mut active = BTreeSet::new();
    let mut ending = BinaryHeap::new();
    // The first place after the one being placed whose event is not kept
    // out of the conversation: only ever moving on.
    let mut visible = 0usize;
    let mut position = 0usize;
    std::iter::from_fn(move || {
        let index = *order.get(position)? as usize;
        let at = spots.times[index];
        while started < turns.len() && turns[started].0 <= at {
            active.insert(started);
            if let Some(end) = turns[started].1 {
                ending.push(Reverse((end, started)));
            }
            started += 1;
        }
        while ending.peek().is_some_and(|Reverse((end, _))| *end < at) {
            let Reverse((_, ended)) = ending.pop().expect("ended turn");
            active.remove(&ended);
        }
        visible = visible.max(position + 1);
        while visible < count && spots.hidden(order[visible] as usize) {
            visible += 1;
        }
        let next_visible = (visible < count).then(|| spots.times[order[visible] as usize]);
        let turn = active.last().copied().or_else(|| {
            if !spots.heard(index) {
                return None;
            }
            let following = turns.partition_point(|(start, ..)| *start < at);
            let (start, ..) = turns.get(following)?;
            // What the host keeps out of its conversation, such as the
            // context it gives the model with the words, is not between.
            next_visible.is_none_or(|next| next >= *start).then_some(following)
        });
        position += 1;
        Some((index, turn))
    })
}

/// `event` placed in `turn`: it takes the turn; words the World's telling
/// went in beside are the speaker's alone; words the World itself handed
/// over are told by the World's record, which knows who said them, and are
/// left out here.
pub(crate) fn finish(mut event: SessionEvent, turn: Option<&str>, turns: &Turns<'_>) -> Option<SessionEvent> {
    if let Some(turn) = turn {
        match &mut event.happened {
            Happened::Heard { turn: slot, .. } | Happened::Health { turn: slot, .. } | Happened::Changed { turn: slot, .. } => {
                *slot = Some(turn.to_string())
            }
            Happened::Began { turn: slot, .. }
            | Happened::Grew { turn: slot, .. }
            | Happened::Finished { turn: slot, .. }
            | Happened::Steps { turn: slot, .. } => *slot = turn.to_string(),
            _ => {}
        }
    }
    if let Happened::Heard { text, .. } = &mut event.happened
        && let Some(words) = claude::beside_framed(text)
    {
        *text = words.to_string();
    }
    if let Happened::Heard { turn, text, message, .. } = &event.happened {
        let told = claude::is_framed(text) || turn.as_deref().is_some_and(|turn| turns.heard(turn, text));
        if told
            && fastctx_types::session::attachments::Envelope::read(text).is_none()
            && message.as_ref().is_none_or(|message| message.images.is_empty())
        {
            return None;
        }
    }
    Some(event)
}

/// Puts each event of Claude Code's record in the World's turn it happened
/// in (`decide`, `finish`). What the World did not see stays where it is, in
/// no turn.
pub(crate) fn place(events: Vec<SessionEvent>, recorded: &[SessionEvent]) -> Vec<SessionEvent> {
    let turns = Turns::of(recorded);
    let mut spots = Spots::default();
    for event in &events {
        spots.push(spot(event));
    }
    let mut events: Vec<Option<SessionEvent>> = events.into_iter().map(Some).collect();
    decide(&spots, &turns)
        .filter_map(|(index, turn)| finish(events[index].take().expect("each event placed once"), turn.map(|turn| turns.name(turn)), &turns))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::session::{Health, HealthKind, Item, TurnEnd, TurnHow, UserMessage, What};

    /// `place` as it was before it was split, word for word.
    fn placed_before(mut events: Vec<SessionEvent>, recorded: &[SessionEvent]) -> Vec<SessionEvent> {
        let mut turns: Vec<(Timestamp, Option<Timestamp>, String)> = Vec::new();
        let mut ends: HashMap<&str, Timestamp> = HashMap::new();
        let mut heard: HashMap<&str, Vec<&str>> = HashMap::new();
        for event in recorded {
            match &event.happened {
                Happened::TurnEnded { turn, .. } => {
                    ends.insert(turn, event.at);
                }
                Happened::Heard { turn: Some(turn), text, .. } => heard.entry(turn).or_default().push(text),
                _ => {}
            }
        }
        for event in recorded {
            if let Happened::TurnStarted { turn } = &event.happened {
                turns.push((event.at, ends.get(turn.as_str()).copied(), turn.clone()));
            }
        }
        turns.sort_by_key(|(start, ..)| *start);
        events.sort_by_key(|event| event.at);
        let mut next_visible = vec![None; events.len()];
        let mut next = None;
        for (index, event) in events.iter().enumerate().rev() {
            next_visible[index] = next;
            if !hidden(event) {
                next = Some(event.at);
            }
        }
        let mut started = 0;
        let mut active = BTreeSet::new();
        let mut ending = BinaryHeap::new();
        let mut placed = Vec::with_capacity(events.len());
        for (index, mut event) in events.into_iter().enumerate() {
            while started < turns.len() && turns[started].0 <= event.at {
                active.insert(started);
                if let Some(end) = turns[started].1 {
                    ending.push(Reverse((end, started)));
                }
                started += 1;
            }
            while ending.peek().is_some_and(|Reverse((end, _))| *end < event.at) {
                let Reverse((_, index)) = ending.pop().expect("ended turn");
                active.remove(&index);
            }
            let turn = active.last().map(|index| turns[*index].2.clone()).or_else(|| match event.happened {
                Happened::Heard { .. } => {
                    let (start, _, turn) = turns.get(turns.partition_point(|(start, ..)| *start < event.at))?;
                    next_visible[index].is_none_or(|next| next >= *start).then(|| turn.clone())
                }
                _ => None,
            });
            if let Some(turn) = turn {
                match &mut event.happened {
                    Happened::Heard { turn: slot, .. } | Happened::Health { turn: slot, .. } | Happened::Changed { turn: slot, .. } => *slot = Some(turn),
                    Happened::Began { turn: slot, .. }
                    | Happened::Grew { turn: slot, .. }
                    | Happened::Finished { turn: slot, .. }
                    | Happened::Steps { turn: slot, .. } => *slot = turn,
                    _ => {}
                }
            }
            if let Happened::Heard { text, .. } = &mut event.happened
                && let Some(words) = claude::beside_framed(text)
            {
                *text = words.to_string();
            }
            if let Happened::Heard { turn, text, message, .. } = &event.happened {
                let told = claude::is_framed(text)
                    || turn.as_deref().and_then(|turn| heard.get(turn)).is_some_and(|said| said.contains(&text.as_str()));
                if told
                    && fastctx_types::session::attachments::Envelope::read(text).is_none()
                    && message.as_ref().is_none_or(|message| message.images.is_empty())
                {
                    continue;
                }
            }
            placed.push(event);
        }
        placed
    }

    struct Choices(u64);
    impl Choices {
        fn pick(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    const TURNS: [&str; 3] = ["t1", "t2", "t3"];
    const WORDS: [&str; 5] = [
        "w0",
        "w1",
        "A message from fastctx, not typed by the user:\nsomething",
        "my words\nThe user sent a new message while you were working:\nmore",
        "The fastctx plugin sent a message from fastctx:\nA message from fastctx, not typed by the user:\nx",
    ];

    fn at(choices: &mut Choices) -> Timestamp {
        Timestamp::from_second(1_800_000_000 + choices.pick(16) as i64).unwrap()
    }

    fn item(choices: &mut Choices, hidden: bool) -> Item {
        Item {
            id: format!("i{}", choices.pick(3)),
            draft: None,
            started_at: None,
            detail: None,
            what: if hidden { What::Hidden { host_kind: "attachment".into() } } else { What::Message { text: "m".into() } },
        }
    }

    fn hosted(choices: &mut Choices) -> SessionEvent {
        let at = at(choices);
        let turn = TURNS[choices.pick(3) as usize].to_string();
        let happened = match choices.pick(6) {
            0 | 1 => Happened::Heard {
                delivery: None,
                turn: (choices.pick(3) == 0).then(|| turn.clone()),
                from: None,
                text: WORDS[choices.pick(5) as usize].into(),
                message: (choices.pick(2) == 0).then(|| {
                    Box::new(UserMessage {
                        attachments: Vec::new(),
                        images: Vec::new(),
                        id: "m".into(),
                        client_id: None,
                        content: Vec::new(),
                        hook_fragments: Vec::new(),
                        presentation: serde_json::Value::Null,
                        detail: serde_json::Value::Null,
                    })
                }),
                notice: None,
            },
            2 => {
                let hidden = choices.pick(2) == 0;
                Happened::Finished { turn: String::new(), item: item(choices, hidden) }
            }
            3 => Happened::Began { turn: String::new(), item: item(choices, false) },
            4 => Happened::Health { turn: None, health: Health { kind: HealthKind::Warning, text: "h".into() } },
            _ => Happened::Titled { title: "t".into() },
        };
        SessionEvent { at, happened }
    }

    fn recorded(choices: &mut Choices) -> SessionEvent {
        let at = at(choices);
        let turn = TURNS[choices.pick(3) as usize].to_string();
        let happened = match choices.pick(3) {
            0 => Happened::TurnStarted { turn },
            1 => Happened::TurnEnded {
                turn,
                end: TurnEnd { how: TurnHow::Completed, reason: None, error: None, duration_ms: None, facts: None, last_message: None },
            },
            _ => Happened::Heard { delivery: None, turn: Some(turn), from: None, text: WORDS[choices.pick(2) as usize].into(), message: None, notice: None },
        };
        SessionEvent { at, happened }
    }

    /// Thousands of small histories: the split placing places every one as
    /// the placing before it did.
    #[test]
    fn the_split_placing_is_the_placing_it_was() {
        let mut choices = Choices(0x2545_f491_4f6c_dd1d);
        for round in 0..20_000 {
            let events: Vec<SessionEvent> = (0..choices.pick(12)).map(|_| hosted(&mut choices)).collect();
            let world: Vec<SessionEvent> = (0..choices.pick(8)).map(|_| recorded(&mut choices)).collect();
            assert_eq!(place(events.clone(), &world), placed_before(events.clone(), &world), "round {round}: {events:?}\n{world:?}");
        }
    }
}
