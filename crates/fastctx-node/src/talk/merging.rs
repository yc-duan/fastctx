//! The merge of the World's record with the host's (`super::history`), split
//! into a decision on what each event shows of itself (`Key`, a fixed size)
//! and its application to the events (`apply`): a history kept on disk
//! (design §28.51) is merged by the same decision as one held whole.

use fastctx_types::session::{Happened, SessionEvent};
use jiff::Timestamp;
use std::collections::{HashMap, HashSet};

pub(crate) mod sparse;

/// Names given numbers, so that a key is of a fixed size: the same name the
/// same number, whatever it names. Closed (`over`), a name it was not given
/// is no number it gave (`ELSEWHERE`), and it is given none.
#[derive(Clone, Default)]
pub(crate) struct Names {
    numbers: HashMap<String, u32>,
    closed: bool,
}

/// The number a closed `Names` gives every name it was not given: none of
/// the World's, so that a host's event named so meets none of them.
pub(crate) const ELSEWHERE: u32 = u32::MAX - 1;

/// No name, in `HostKeys`.
const NONE: u32 = u32::MAX;

impl Names {
    pub(crate) fn of(&mut self, name: &str) -> u32 {
        if let Some(number) = self.numbers.get(name) {
            return *number;
        }
        if self.closed {
            return ELSEWHERE;
        }
        let number = u32::try_from(self.numbers.len()).ok().filter(|number| *number < ELSEWHERE).expect("fewer names than numbers");
        self.numbers.insert(name.to_string(), number);
        number
    }

    /// The names the World's `recorded` names, numbered, then closed (design
    /// §28.51 ⑤-6): a host's events keyed by it hold no number of their own
    /// for each name the World never names. The merge only ever asks whether
    /// a World event's name is a host event's, so whether two host events'
    /// names are the same, which these no longer tell, is never asked. Only
    /// for merging with `recorded`, or with fewer of its events.
    /// Closed now, as `over` leaves the names it numbered (what numbered the
    /// World's events a page at a time, `timeline::world`).
    pub(crate) fn close(&mut self) {
        self.closed = true;
    }

    pub(crate) fn over(recorded: &[SessionEvent]) -> Self {
        let mut names = Names::default();
        for event in recorded {
            key(event, &mut names);
        }
        names.closed = true;
        names
    }
}

/// The host's events' keys as a merge with the World's record holds them
/// (`sparse::decide`), in little: each one's turn, kind, and whether it
/// stays in the history; the rest of its key — the turn it wrote, its
/// message, its item, its voice — only where the World's record can meet
/// it (`Rest`), apart, and few are; and its time (`Moments`). Their drafts
/// and whether they are native not at all: the merge asks those only of the
/// World's events. Each name a number, `NONE` for none. Held a block at a
/// time (`Column`), so that nothing held grows past what it holds by more
/// than a block (design §28.51 ⑥-3).
#[derive(Default)]
pub(crate) struct HostKeys {
    turns: Column<u32>,
    kinds: Column<u8>,
    /// The rests of those that have one, by their numbers, rising.
    rests: Vec<(u32, Rest)>,
    moments: Moments,
}

/// The rest of a host's event's key (`HostKeys`).
#[derive(Clone, Copy)]
struct Rest {
    written_turn: u32,
    message: u32,
    item: u32,
    realtime: (u32, u32),
    complete: bool,
}

/// That a host's event stays in the history, beside its kind (`HostKeys`).
const STAYS: u8 = 0x80;

const KINDS: [Kind; 7] = [Kind::TurnStarted, Kind::TurnEnded, Kind::Heard, Kind::Finished, Kind::Updated, Kind::Realtime, Kind::Other];

impl HostKeys {
    /// `key` held, the next of the host's events.
    pub(crate) fn push(&mut self, key: &Key) {
        let number = |name: Option<u32>| name.unwrap_or(NONE);
        let named = |name: Option<u32>| name.is_some_and(|name| name != ELSEWHERE);
        // What the merge can meet of a key but its turn: a turn's start or
        // end by the turn it wrote; voice; words by a message the World
        // names; an item the World names, with the turn it wrote. A name the
        // World does not name meets nothing, whether it is there or not.
        let met = match key.kind {
            Kind::TurnStarted | Kind::TurnEnded | Kind::Realtime => true,
            Kind::Heard => named(key.message),
            Kind::Finished | Kind::Updated => named(key.item),
            Kind::Other => false,
        };
        let index = u32::try_from(self.len()).ok().filter(|index| *index < NONE).expect("fewer events than numbers");
        if met {
            let (complete, realtime) = match key.realtime {
                Some((complete, item, session)) => (complete, (item, session)),
                None => (false, (NONE, NONE)),
            };
            self.rests.push((index, Rest { written_turn: number(key.written_turn), message: number(key.message), item: number(key.item), realtime, complete }));
        }
        self.turns.push(number(key.turn));
        let kind = KINDS.iter().position(|kind| *kind == key.kind).expect("a kind") as u8;
        self.kinds.push(kind | if key.in_history { STAYS } else { 0 });
        self.moments.push(key.at);
    }

    pub(crate) fn len(&self) -> usize {
        self.turns.len()
    }

    /// Whether the host's event numbered `index` stays in the history.
    pub(crate) fn stays(&self, index: usize) -> bool {
        self.kinds.get(index) & STAYS != 0
    }

    /// Their times, taken to be weighed apart (`sparse::Times`): what is
    /// left knows each one's key but its time (`key`).
    pub(crate) fn take_moments(&mut self) -> Moments {
        std::mem::take(&mut self.moments)
    }

    /// The key of the host's event numbered `index` as the merge asks of it,
    /// but its time, which the merge weighs apart (`take_moments`): `at` is
    /// the epoch's.
    pub(crate) fn key(&self, index: usize) -> Key {
        let name = |number: u32| (number != NONE).then_some(number);
        let kind = KINDS[(self.kinds.get(index) & !STAYS) as usize];
        let mut key = Key {
            at: Timestamp::UNIX_EPOCH,
            kind,
            turn: name(self.turns.get(index)),
            written_turn: None,
            message: None,
            item: None,
            draft: None,
            realtime: None,
            native: false,
            in_history: self.stays(index),
        };
        let index = index as u32;
        if let Ok(found) = self.rests.binary_search_by_key(&index, |(number, _)| *number) {
            let rest = self.rests[found].1;
            key.written_turn = name(rest.written_turn);
            key.message = name(rest.message);
            key.item = name(rest.item);
            key.realtime = (rest.realtime.0 != NONE).then_some((rest.complete, rest.realtime.0, rest.realtime.1));
        }
        key
    }
}

/// How many a block of a `Column` holds.
const BLOCK: usize = 1 << 16;

/// Values held a block at a time, each block made as large as it will be:
/// never more room than a block beyond what is held, and nothing moved as it
/// grows (design §28.51 ⑥-3).
pub(crate) struct Column<T> {
    blocks: Vec<Vec<T>>,
    len: usize,
}

impl<T> Default for Column<T> {
    fn default() -> Self {
        Column { blocks: Vec::new(), len: 0 }
    }
}

impl<T: Copy> Column<T> {
    pub(crate) fn push(&mut self, value: T) {
        if self.len.is_multiple_of(BLOCK) {
            self.blocks.push(Vec::with_capacity(BLOCK));
        }
        self.blocks.last_mut().expect("a block").push(value);
        self.len += 1;
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn get(&self, index: usize) -> T {
        self.blocks[index / BLOCK][index % BLOCK]
    }

    pub(crate) fn set(&mut self, index: usize, value: T) {
        self.blocks[index / BLOCK][index % BLOCK] = value;
    }
}

impl<T: Copy> FromIterator<T> for Column<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        let mut column = Column::default();
        for value in values {
            column.push(value);
        }
        column
    }
}

/// A time as the merge weighs it: only against another, in order.
pub(crate) trait Moment: Copy + Ord {
    /// Earlier than any (or the earliest, which no time is later than).
    const NONE: Self;
    fn of(at: Timestamp) -> Self;
}

impl Moment for Timestamp {
    const NONE: Self = Timestamp::MIN;
    fn of(at: Timestamp) -> Self {
        at
    }
}

/// Nanoseconds of the epoch, where every time weighed is one
/// (`Moments::Nanos`).
impl Moment for i64 {
    const NONE: Self = i64::MIN;
    fn of(at: Timestamp) -> Self {
        nanos(at).expect("a time weighed in nanoseconds is one")
    }
}

/// `at` as nanoseconds of the epoch, where it is one.
pub(crate) fn nanos(at: Timestamp) -> Option<i64> {
    i64::try_from(at.as_nanosecond()).ok()
}

/// The host's events' times: as nanoseconds of the epoch, eight bytes each,
/// while every one is one (from 1677 to 2262); else, from the first that is
/// not, the times themselves. Weighed only against each other and the
/// World's, the same either way.
pub(crate) enum Moments {
    Nanos(Column<i64>),
    Exact(Column<Timestamp>),
}

impl Default for Moments {
    fn default() -> Self {
        Moments::Nanos(Column::default())
    }
}

impl Moments {
    pub(crate) fn push(&mut self, at: Timestamp) {
        match self {
            Moments::Nanos(held) => match nanos(at) {
                Some(nanos) => held.push(nanos),
                None => {
                    let mut exact: Column<Timestamp> = (0..held.len()).map(|index| from_nanos(held.get(index))).collect();
                    exact.push(at);
                    *self = Moments::Exact(exact);
                }
            },
            Moments::Exact(held) => held.push(at),
        }
    }

    /// The times themselves.
    pub(crate) fn exact(self) -> Column<Timestamp> {
        match self {
            Moments::Nanos(held) => (0..held.len()).map(|index| from_nanos(held.get(index))).collect(),
            Moments::Exact(held) => held,
        }
    }
}

impl FromIterator<Timestamp> for Moments {
    fn from_iter<I: IntoIterator<Item = Timestamp>>(times: I) -> Self {
        let mut moments = Moments::default();
        for at in times {
            moments.push(at);
        }
        moments
    }
}

fn from_nanos(nanos: i64) -> Timestamp {
    Timestamp::from_nanosecond(i128::from(nanos)).expect("nanoseconds of the epoch held from a time")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    TurnStarted,
    TurnEnded,
    Heard,
    Finished,
    Updated,
    Realtime,
    Other,
}

/// What the merge looks at of an event.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Key {
    pub(crate) at: Timestamp,
    pub(crate) kind: Kind,
    /// Its turn as `super::turn_of` gives it.
    pub(crate) turn: Option<u32>,
    /// A turn's start or end, or an item: its turn as written, empty or not.
    pub(crate) written_turn: Option<u32>,
    /// Words with a message: its id.
    pub(crate) message: Option<u32>,
    /// An item: its id, and a finished one's draft.
    pub(crate) item: Option<u32>,
    pub(crate) draft: Option<u32>,
    /// Voice: whether complete, and its item's id and session as JSON.
    pub(crate) realtime: Option<(bool, u32, u32)>,
    /// Words with no delivery and no speaker whose message is a Codex user
    /// message: what a Codex host makes again of its own.
    pub(crate) native: bool,
    /// `Happened::in_history`.
    pub(crate) in_history: bool,
}

/// The key of `event`, its names numbered by `names`.
pub(crate) fn key(event: &SessionEvent, names: &mut Names) -> Key {
    let turn = super::turn_of(&event.happened).map(|turn| names.of(turn));
    let mut key = Key {
        at: event.at,
        kind: Kind::Other,
        turn,
        written_turn: None,
        message: None,
        item: None,
        draft: None,
        realtime: None,
        native: false,
        in_history: event.happened.in_history(),
    };
    match &event.happened {
        Happened::TurnStarted { turn } => {
            key.kind = Kind::TurnStarted;
            key.written_turn = Some(names.of(turn));
        }
        Happened::TurnEnded { turn, .. } => {
            key.kind = Kind::TurnEnded;
            key.written_turn = Some(names.of(turn));
        }
        Happened::Heard { delivery, from, message, .. } => {
            key.kind = Kind::Heard;
            if let Some(message) = message {
                key.message = Some(names.of(&message.id));
                key.native = delivery.is_none() && from.is_none() && message.detail["type"] == "userMessage";
            }
        }
        Happened::Finished { turn, item } => {
            key.kind = Kind::Finished;
            key.written_turn = Some(names.of(turn));
            key.item = Some(names.of(&item.id));
            key.draft = item.draft.as_deref().map(|draft| names.of(draft));
        }
        Happened::Updated { turn, item } => {
            key.kind = Kind::Updated;
            key.written_turn = Some(names.of(turn));
            key.item = Some(names.of(&item.id));
        }
        Happened::Realtime { item, complete } => {
            key.kind = Kind::Realtime;
            key.realtime = Some((*complete, names.of(&canonical(&item["id"])), names.of(&canonical(&item["realtimeSessionId"]))));
        }
        _ => {}
    }
    key
}

/// `value` as text that is the same exactly when the values are equal: as
/// JSON, but a zero written as a float the same whatever its sign, as
/// `Value`'s equality takes them (an object's keys are in their order).
fn canonical(value: &serde_json::Value) -> String {
    use serde_json::Value;
    fn even(value: &Value) -> Value {
        match value {
            Value::Number(number) if number.is_f64() && number.as_f64() == Some(0.0) => Value::from(0.0),
            Value::Array(items) => Value::Array(items.iter().map(even).collect()),
            Value::Object(fields) => Value::Object(fields.iter().map(|(name, field)| (name.clone(), even(field))).collect()),
            other => other.clone(),
        }
    }
    even(value).to_string()
}

/// What stands in a place of the merged history: an event of the host's or
/// of the World's record (`base`), and each of the World's events that took
/// that place after it, in turn (`over`), each taking the pictures of what
/// stood there before it (`crate::user_images::merge`). Kept as a list, not
/// a chain: however many take one place, laying it out or letting it go
/// takes no deeper a stack.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) base: Base,
    pub(crate) over: Vec<usize>,
}

/// What first stood in a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Base {
    Hosted(usize),
    Recorded(usize),
}

impl Slot {
    fn of(base: Base) -> Self {
        Self { base, over: Vec::new() }
    }
}

/// Whether `event` of the World's record is the same as `other` in the
/// history: a turn's start or end, words by their message, voice by its item.
fn same(event: &Key, other: &Key) -> bool {
    match (event.kind, other.kind) {
        (Kind::TurnStarted, Kind::TurnStarted) | (Kind::TurnEnded, Kind::TurnEnded) => event.written_turn == other.written_turn,
        (Kind::Heard, Kind::Heard) => event.message.is_some() && other.message.is_some() && event.turn == other.turn && event.message == other.message,
        (Kind::Realtime, Kind::Realtime) => event.realtime == other.realtime,
        _ => false,
    }
}

/// The merged history: the host's events (`hosted`, as restored and filtered)
/// with the World's record (`recorded`) put among them, as `super::history`
/// has done it — each of the World's events in turn, in the history as it
/// stands then. `codex`: the host is Codex.
pub(crate) fn decide(hosted: &[Key], recorded: &[Key], codex: bool) -> Vec<Slot> {
    let items = |key: &Key| matches!(key.kind, Kind::Finished | Kind::Updated);
    // What the host's record holds is the one shown of anything both tell
    // under one name; a draft recorded before its row was kept is the item
    // that names it as its draft.
    let held: HashSet<(u32, u32)> =
        hosted.iter().filter(|key| items(key)).filter_map(|key| Some((key.written_turn?, key.item?))).collect();
    let drafts: HashSet<(u32, u32)> =
        recorded.iter().filter(|key| key.kind == Kind::Finished).filter_map(|key| Some((key.written_turn?, key.draft?))).collect();
    // When the World saw each turn begin: the first words a turn heard soon
    // after it opened it, whatever the times say.
    let began: HashMap<u32, Timestamp> =
        recorded.iter().filter(|key| key.kind == Kind::TurnStarted).filter_map(|key| Some((key.written_turn?, key.at))).collect();
    let mut opened: HashSet<u32> = HashSet::new();
    // Each place, with the key of what stands in it now.
    let mut history: Vec<(Slot, Key)> = hosted.iter().enumerate().map(|(index, key)| (Slot::of(Base::Hosted(index)), *key)).collect();
    for (at, event) in recorded.iter().enumerate() {
        if items(event)
            && let (Some(turn), Some(item)) = (event.written_turn, event.item)
            && (held.contains(&(turn, item)) || drafts.contains(&(turn, item)))
        {
            continue;
        }
        if codex && event.kind == Kind::Heard && event.native {
            continue;
        }
        if let Some(index) = history.iter().position(|(_, other)| same(event, other)) {
            history[index].0.over.push(at);
            history[index].1 = *event;
            continue;
        }
        let turn = event.turn;
        let opening = match (event.kind, turn) {
            (Kind::Heard, Some(turn)) if !opened.contains(&turn) => began
                .get(&turn)
                .is_some_and(|start| event.at.as_millisecond().saturating_sub(start.as_millisecond()) <= super::OPENING_MS),
            _ => false,
        };
        let first = opening.then(|| history.iter().position(|(_, other)| turn == other.turn && other.kind != Kind::TurnStarted)).flatten();
        if let Some(turn) = turn.filter(|_| opening) {
            opened.insert(turn);
        }
        if let Some(index) = first {
            history.insert(index, (Slot::of(Base::Recorded(at)), *event));
            continue;
        }
        let index = history
            .iter()
            .position(|(_, other)| {
                let same_turn = turn.is_some() && turn == other.turn;
                if same_turn && other.kind == Kind::TurnEnded {
                    return true;
                }
                if same_turn && event.kind == Kind::TurnStarted {
                    return true;
                }
                if other.kind == Kind::TurnStarted && same_turn {
                    return false;
                }
                other.at > event.at
            })
            .unwrap_or(history.len());
        history.insert(index, (Slot::of(Base::Recorded(at)), *event));
    }
    history.into_iter().filter(|(_, key)| key.in_history).map(|(slot, _)| slot).collect()
}

/// The history `slots` lay out, of the events they refer to.
pub(crate) fn apply(slots: Vec<Slot>, hosted: Vec<SessionEvent>, recorded: Vec<SessionEvent>) -> Vec<SessionEvent> {
    let mut hosted: Vec<Option<SessionEvent>> = hosted.into_iter().map(Some).collect();
    let mut history = Vec::with_capacity(slots.len());
    let laid: Result<(), std::convert::Infallible> = apply_each(
        slots,
        &mut |index| Ok(hosted[index].take().expect("each of the host's events in one place")),
        recorded,
        &mut |event| {
            history.push(event);
            Ok(())
        },
    );
    let Ok(()) = laid;
    history
}

/// As `apply`, the host's events taken one at a time by their index
/// (`hosted`, each once, the indices rising), the history handed on one
/// event at a time (`each`): for a reading kept on disk (design §28.51).
pub(crate) fn apply_each<E>(
    slots: impl IntoIterator<Item = Slot>,
    hosted: &mut dyn FnMut(usize) -> Result<SessionEvent, E>,
    recorded: Vec<SessionEvent>,
    each: &mut dyn FnMut(SessionEvent) -> Result<(), E>,
) -> Result<(), E> {
    let mut recorded: Vec<Option<SessionEvent>> = recorded.into_iter().map(Some).collect();
    for slot in slots {
        each(made(slot, hosted, &mut recorded)?)?;
    }
    Ok(())
}

fn made<E>(slot: Slot, hosted: &mut dyn FnMut(usize) -> Result<SessionEvent, E>, recorded: &mut [Option<SessionEvent>]) -> Result<SessionEvent, E> {
    let mut event = match slot.base {
        Base::Hosted(index) => hosted(index)?,
        Base::Recorded(at) => recorded[at].take().expect("each of the World's events in one place"),
    };
    for at in slot.over {
        let mut next = recorded[at].take().expect("each of the World's events in one place");
        crate::user_images::merge(&mut next, &event);
        event = next;
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::session::{Health, HealthKind, Item, TurnEnd, TurnHow, UserMessage, What};

    /// The merge as `super::history` did it before it was split, word for
    /// word: what the split one must equal.
    fn merged_before(mut history: Vec<SessionEvent>, recorded: Vec<SessionEvent>, agent: &str) -> Vec<SessionEvent> {
        let held: HashSet<(String, String)> = history
            .iter()
            .filter_map(|event| match &event.happened {
                Happened::Finished { turn, item } | Happened::Updated { turn, item } => Some((turn.clone(), item.id.clone())),
                _ => None,
            })
            .collect();
        let drafts: HashSet<(String, String)> = recorded
            .iter()
            .filter_map(|event| match &event.happened {
                Happened::Finished { turn, item } => Some((turn.clone(), item.draft.clone()?)),
                _ => None,
            })
            .collect();
        let began: HashMap<String, Timestamp> = recorded
            .iter()
            .filter_map(|event| match &event.happened {
                Happened::TurnStarted { turn } => Some((turn.clone(), event.at)),
                _ => None,
            })
            .collect();
        let mut opened = HashSet::new();
        for mut event in recorded {
            if let Happened::Finished { turn, item } | Happened::Updated { turn, item } = &event.happened
                && (held.contains(&(turn.clone(), item.id.clone())) || drafts.contains(&(turn.clone(), item.id.clone())))
            {
                continue;
            }
            if matches!(&event.happened, Happened::Heard { delivery: None, from: None, message: Some(message), .. }
                if agent == "codex" && message.detail["type"] == "userMessage")
            {
                continue;
            }
            let same = history.iter().position(|other| match (&event.happened, &other.happened) {
                (Happened::TurnStarted { turn: a }, Happened::TurnStarted { turn: b })
                | (Happened::TurnEnded { turn: a, .. }, Happened::TurnEnded { turn: b, .. }) => a == b,
                (Happened::Heard { turn: a, message: Some(a_message), .. }, Happened::Heard { turn: b, message: Some(b_message), .. }) => {
                    a == b && a_message.id == b_message.id
                }
                (Happened::Realtime { item: a, complete: ac }, Happened::Realtime { item: b, complete: bc }) => {
                    ac == bc && a["id"] == b["id"] && a["realtimeSessionId"] == b["realtimeSessionId"]
                }
                _ => false,
            });
            if let Some(index) = same {
                crate::user_images::merge(&mut event, &history[index]);
                history[index] = event;
                continue;
            }
            let turn = super::super::turn_of(&event.happened);
            let opening = match (&event.happened, turn) {
                (Happened::Heard { .. }, Some(turn)) if !opened.contains(turn) => began
                    .get(turn)
                    .is_some_and(|start| event.at.as_millisecond().saturating_sub(start.as_millisecond()) <= super::super::OPENING_MS),
                _ => false,
            };
            let first = opening
                .then(|| history.iter().position(|other| turn == super::super::turn_of(&other.happened) && !matches!(other.happened, Happened::TurnStarted { .. })))
                .flatten();
            if let Some(turn) = turn.filter(|_| opening) {
                opened.insert(turn.to_string());
            }
            if let Some(index) = first {
                history.insert(index, event);
                continue;
            }
            let index = history
                .iter()
                .position(|other| {
                    let same_turn = turn.is_some() && turn == super::super::turn_of(&other.happened);
                    if same_turn && matches!(other.happened, Happened::TurnEnded { .. }) {
                        return true;
                    }
                    if same_turn && matches!(event.happened, Happened::TurnStarted { .. }) {
                        return true;
                    }
                    if matches!(other.happened, Happened::TurnStarted { .. }) && same_turn {
                        return false;
                    }
                    other.at > event.at
                })
                .unwrap_or(history.len());
            history.insert(index, event);
        }
        history.retain(|event| event.happened.in_history());
        history
    }

    /// A small, fixed-seed source of choices.
    struct Choices(u64);
    impl Choices {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn pick(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    const TURNS: [&str; 4] = ["t1", "t2", "t3", ""];
    const IDS: [&str; 3] = ["a", "b", "c"];

    fn message(id: &str, native: bool, picture: bool) -> Box<UserMessage> {
        Box::new(UserMessage {
            attachments: Vec::new(),
            images: Vec::new(),
            id: id.into(),
            client_id: picture.then(|| "client".to_string()),
            content: if picture { vec![serde_json::json!({"type": "image"})] } else { Vec::new() },
            hook_fragments: Vec::new(),
            presentation: serde_json::Value::Null,
            detail: if native { serde_json::json!({"type": "userMessage"}) } else { serde_json::Value::Null },
        })
    }

    fn item(choices: &mut Choices) -> Item {
        Item {
            id: IDS[choices.pick(3) as usize].into(),
            draft: (choices.pick(3) == 0).then(|| IDS[choices.pick(3) as usize].to_string()),
            started_at: None,
            detail: None,
            what: What::Message { text: format!("x{}", choices.pick(4)) },
        }
    }

    /// An event of one of the kinds the merge tells apart, at a moment among
    /// a few, of a turn among a few: alike often, so that the rules meet.
    fn event(choices: &mut Choices) -> SessionEvent {
        let at = Timestamp::from_millisecond(1_800_000_000_000 + choices.pick(12) as i64 * 700).unwrap();
        let turn = TURNS[choices.pick(4) as usize].to_string();
        let maybe_turn = (choices.pick(4) != 0).then(|| turn.clone());
        let happened = match choices.pick(9) {
            0 => Happened::TurnStarted { turn },
            1 => Happened::TurnEnded {
                turn,
                end: TurnEnd { how: TurnHow::Completed, reason: None, error: None, duration_ms: None, facts: None, last_message: None },
            },
            2 | 3 => Happened::Heard {
                delivery: None,
                turn: maybe_turn,
                from: None,
                text: format!("w{}", choices.pick(3)),
                message: (choices.pick(3) != 0).then(|| message(IDS[choices.pick(3) as usize], choices.pick(3) == 0, choices.pick(2) == 0)),
                notice: None,
            },
            4 => Happened::Finished { turn, item: item(choices) },
            5 => Happened::Updated { turn, item: item(choices) },
            // Ids as JSON: a zero of either sign is the same id to `Value`.
            6 => {
                let ids = [serde_json::json!("a"), serde_json::json!("b"), serde_json::json!(0.0), serde_json::json!(-0.0), serde_json::json!(0), serde_json::json!([-0.0])];
                let id = ids[choices.pick(6) as usize].clone();
                Happened::Realtime {
                    item: serde_json::json!({"id": id, "realtimeSessionId": IDS[choices.pick(2) as usize]}),
                    complete: choices.pick(2) == 0,
                }
            }
            7 => Happened::Began { turn, item: item(choices) },
            _ => Happened::Health { turn: maybe_turn, health: Health { kind: HealthKind::Warning, text: "h".into() } },
        };
        SessionEvent { at, happened }
    }

    /// Thousands of small histories, Claude and Codex: the split merge lays
    /// out every one as the merge before it did.
    #[test]
    fn the_split_merge_is_the_merge_it_was() {
        let mut choices = Choices(0x9e37_79b9_7f4a_7c15);
        for round in 0..20_000 {
            let hosted: Vec<SessionEvent> = (0..choices.pick(10)).map(|_| event(&mut choices)).collect();
            let recorded: Vec<SessionEvent> = (0..choices.pick(10)).map(|_| event(&mut choices)).collect();
            let agent = if round % 2 == 0 { "claude" } else { "codex" };
            let before = merged_before(hosted.clone(), recorded.clone(), agent);
            let mut names = Names::default();
            let hosted_keys: Vec<Key> = hosted.iter().map(|event| key(event, &mut names)).collect();
            let recorded_keys: Vec<Key> = recorded.iter().map(|event| key(event, &mut names)).collect();
            let now = apply(decide(&hosted_keys, &recorded_keys, agent == "codex"), hosted.clone(), recorded.clone());
            assert_eq!(now, before, "round {round}: hosted {hosted:?}\nrecorded {recorded:?}");
        }
    }

    /// However many of the World's events take one place in turn, laying the
    /// history out and letting it go take no deeper a stack.
    #[test]
    fn many_taking_one_place_take_no_deeper_a_stack() {
        let laid = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let at = Timestamp::from_second(1_800_000_000).unwrap();
                let words = || SessionEvent {
                    at,
                    happened: Happened::Heard {
                        delivery: None,
                        turn: Some("t".into()),
                        from: None,
                        text: "w".into(),
                        message: Some(message("m", false, false)),
                        notice: None,
                    },
                };
                let hosted = vec![words()];
                let recorded: Vec<SessionEvent> = (0..100_000).map(|_| words()).collect();
                let mut names = Names::default();
                let hosted_keys: Vec<Key> = hosted.iter().map(|event| key(event, &mut names)).collect();
                let recorded_keys: Vec<Key> = recorded.iter().map(|event| key(event, &mut names)).collect();
                let slots = decide(&hosted_keys, &recorded_keys, false);
                assert_eq!(slots.len(), 1);
                assert_eq!(slots[0].over.len(), 100_000);
                apply(slots, hosted, recorded).len()
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(laid, 1);
    }
}
