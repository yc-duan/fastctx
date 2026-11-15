//! The merge decided as `super::decide` decides it, laid out sparsely
//! (design §28.51 ⑤-6): the host's events stand in their order, and only
//! what the World's record does among them is kept — which of them it took
//! the places of, and where it put its own, in what order. Each of the
//! World's events finds its place through indices of what it can meet (a
//! turn, a word's message, voice) and of the host's times, not by reading
//! the whole history again; so what grows with the host's events is their
//! times and a few positions, and a World event's place is found in about
//! the logarithm of their number and the number of the World's own.

mod puts;

use super::{Base, Column, HostKeys, Key, Kind, Moment, Moments, Slot, nanos, same};
use puts::Puts;
use jiff::Timestamp;
use std::collections::{HashMap, HashSet};

/// What `same` matches an event by, for the kinds it matches at all.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Signature {
    Started(Option<u32>),
    Ended(Option<u32>),
    Heard(Option<u32>, u32),
    Realtime(Option<(bool, u32, u32)>),
}

fn signature(key: &Key) -> Option<Signature> {
    match key.kind {
        Kind::TurnStarted => Some(Signature::Started(key.written_turn)),
        Kind::TurnEnded => Some(Signature::Ended(key.written_turn)),
        Kind::Heard => key.message.map(|message| Signature::Heard(key.turn, message)),
        Kind::Realtime => Some(Signature::Realtime(key.realtime)),
        _ => None,
    }
}

/// How many times each level of `Times` gathers of the one below.
const FAN: usize = 64;

/// The host's events' times as they stand now, with the latest of each
/// `FAN` of them, and of each `FAN` of those, up to one block: the first
/// event later than a time is found without reading all before it.
struct Times<T> {
    /// Each host's event's time (`HostKeys::take_moments`).
    base: Column<T>,
    /// The latest of each `FAN` of the level below, from the base up.
    levels: Vec<Vec<T>>,
}

impl<T: Moment> Times<T> {
    fn new(base: Column<T>) -> Self {
        let mut times = Times { base, levels: Vec::new() };
        while times.len(times.levels.len()) > FAN {
            let level = times.levels.len();
            let below = times.len(level);
            let above = (0..below).step_by(FAN).map(|start| times.latest(level, start, (start + FAN).min(below))).collect();
            times.levels.push(above);
        }
        times
    }

    /// How many `level` holds (the base, 0).
    fn len(&self, level: usize) -> usize {
        if level == 0 { self.base.len() } else { self.levels[level - 1].len() }
    }

    fn value(&self, level: usize, index: usize) -> T {
        if level == 0 { self.base.get(index) } else { self.levels[level - 1][index] }
    }

    /// The latest of `level` from `start` to `end`, one or more.
    fn latest(&self, level: usize, start: usize, end: usize) -> T {
        (start..end).map(|index| self.value(level, index)).max().expect("a block of one or more")
    }

    fn set(&mut self, index: usize, at: T) {
        self.base.set(index, at);
        let mut index = index;
        for level in 1..=self.levels.len() {
            index /= FAN;
            let start = index * FAN;
            let latest = self.latest(level - 1, start, (start + FAN).min(self.len(level - 1)));
            self.levels[level - 1][index] = latest;
        }
    }

    /// The first event at `from` or after whose time is later than `at`.
    fn first_later(&self, from: usize, at: T) -> Option<usize> {
        self.find(0, from, at)
    }

    /// The first index at `level`, `from` or after, whose time is later than
    /// `at`, as an index of the events.
    fn find(&self, level: usize, from: usize, at: T) -> Option<usize> {
        let len = self.len(level);
        if from >= len {
            return None;
        }
        let end = ((from / FAN + 1) * FAN).min(len);
        if let Some(found) = (from..end).find(|index| self.value(level, *index) > at) {
            return Some(self.descend(level, found, at));
        }
        // Past the last of this level, or at the top: none. Else the blocks
        // after this one, from the level above (their first is `end`).
        if end == len || level == self.levels.len() {
            return None;
        }
        self.find(level + 1, end / FAN, at)
    }

    /// The first event under `index` at `level` whose time is later than
    /// `at`, where one is.
    fn descend(&self, mut level: usize, mut index: usize, at: T) -> usize {
        while level > 0 {
            level -= 1;
            let start = index * FAN;
            index = (start..(start + FAN).min(self.len(level))).find(|below| self.value(level, *below) > at).expect("the latest of a block is in it");
        }
        index
    }
}

/// A place of the merged history: a host's event's, or one of the World's
/// events the merge put in, by its number among the World's events.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    Hosted(usize),
    Put(usize),
}

/// The merged history as `decide` leaves it, laid out sparsely, its times
/// weighed as `T`.
pub struct Layout<T> {
    hosted: HostKeys,
    /// The host's events' times, as they stand now where a World event took
    /// a place.
    times: Times<T>,
    /// The host's events the World's took the places of: those that took
    /// it, in turn, and the key of the last.
    taken: HashMap<usize, (Vec<usize>, Key)>,
    /// The World's events put among them, in the merged order (design
    /// §28.51 ⑥-2).
    puts: Puts<T>,
    /// Those put whose keys have a signature — the only ones a later event
    /// can take the place of, and the only ones that start or end a turn:
    /// what stands there now.
    signed: HashMap<usize, Signed>,
    /// What took the places of those taken, in turn.
    over: HashMap<usize, Vec<usize>>,
    /// Those signed by their signature: one each, as an event whose
    /// signature one put has takes a place, and taking keeps it.
    by_signature: HashMap<Signature, usize>,
    /// Those signed by the turn of what stands there now, and the ends of
    /// each turn among them apart (one each, as a second takes the first's
    /// place or the host's).
    signed_of_turn: HashMap<u32, Vec<usize>>,
    ends_of_turn: HashMap<u32, Vec<usize>>,
    /// Of those put with no signature, whose keys never change: the first in
    /// the merged order of each turn, and whether each stays in the history.
    first_unsigned: HashMap<u32, usize>,
    stays: Vec<u64>,
}

/// What `Layout` holds of a signed put's key as it stands now.
#[derive(Clone, Copy)]
struct Signed {
    kind: Kind,
    turn: Option<u32>,
    in_history: bool,
}

impl Signed {
    fn of(key: &Key) -> Self {
        Signed { kind: key.kind, turn: key.turn, in_history: key.in_history }
    }
}

impl<T: Moment + 'static> Layout<T> {
    /// The key of the host's event `index` as it stands now, but its time,
    /// which `times` holds.
    fn hosted_key(&self, index: usize) -> Key {
        self.taken.get(&index).map_or_else(|| self.hosted.key(index), |(_, key)| *key)
    }

    /// Whether `one` stands before `other` (or is it): a World event put
    /// before a host's event stands before it.
    fn first(&self, one: Place, other: Place) -> bool {
        match (one, other) {
            (Place::Hosted(one), Place::Hosted(other)) => one <= other,
            (Place::Hosted(one), Place::Put(other)) => self.puts.gap(other) > one,
            (Place::Put(one), Place::Hosted(other)) => self.puts.gap(one) <= other,
            (Place::Put(one), Place::Put(other)) => one == other || self.puts.before(one, other),
        }
    }

    fn earliest(&self, one: Option<Place>, other: Option<Place>) -> Option<Place> {
        match (one, other) {
            (Some(one), Some(other)) => Some(if self.first(one, other) { one } else { other }),
            (one, other) => one.or(other),
        }
    }

    /// The put whose key is `same` as `event`'s.
    fn put_same(&self, event: &Key) -> Option<usize> {
        signature(event).and_then(|signature| self.by_signature.get(&signature)).copied()
    }

    /// The first of those put of `turn` but its start: for the words that
    /// open it, once a turn.
    fn first_in_turn(&self, turn: u32) -> Option<usize> {
        let signed = self.signed_of_turn.get(&turn).into_iter().flatten().copied().filter(|put| self.signed[put].kind != Kind::TurnStarted);
        self.puts.first_of(signed.chain(self.first_unsigned.get(&turn).copied()))
    }

    /// The first of those put of `turn`: for its start, once a turn, as a
    /// second start takes the first's place or the host's.
    fn first_of_turn(&self, turn: u32) -> Option<usize> {
        let signed = self.signed_of_turn.get(&turn).into_iter().flatten().copied();
        self.puts.first_of(signed.chain(self.first_unsigned.get(&turn).copied()))
    }

    /// The World's event `at`, of key `key`, put right before `place` (after
    /// all, with none).
    fn insert(&mut self, place: Option<Place>, at: usize, key: Key) {
        match place {
            Some(Place::Hosted(hosted)) => self.puts.insert_in_gap(at, hosted),
            Some(Place::Put(put)) => self.puts.insert_before(at, put),
            None => self.puts.push(at, self.hosted.len()),
        }
        if let Some(signature) = signature(&key) {
            self.signed.insert(at, Signed::of(&key));
            let first = self.by_signature.insert(signature, at);
            debug_assert!(first.is_none(), "one put of a signature");
            if let Some(turn) = key.turn {
                self.signed_of_turn.entry(turn).or_default().push(at);
                if key.kind == Kind::TurnEnded {
                    self.ends_of_turn.entry(turn).or_default().push(at);
                }
            }
            return;
        }
        if key.in_history {
            self.stays[at / 64] |= 1 << (at % 64);
        }
        if let Some(turn) = key.turn {
            let first = self.first_unsigned.entry(turn).or_insert(at);
            if *first != at && self.puts.before(at, *first) {
                *first = at;
            }
        }
    }

    /// The World's event `at`, of key `event`, takes the place of `put`.
    fn take_put(&mut self, put: usize, at: usize, event: &Key) {
        let signed = self.signed.get_mut(&put).expect("a place taken is signed");
        let was = *signed;
        *signed = Signed::of(event);
        self.over.entry(put).or_default().push(at);
        self.puts.set_moment(put, self.puts.moment(at));
        // Of the same kind (`same`): only its turn may be another now.
        if was.turn != event.turn {
            for of_turn in [&mut self.signed_of_turn, &mut self.ends_of_turn] {
                if let Some(places) = was.turn.and_then(|turn| of_turn.get_mut(&turn)) {
                    places.retain(|place| *place != put);
                }
            }
            if let Some(turn) = event.turn {
                self.signed_of_turn.entry(turn).or_default().push(put);
                if event.kind == Kind::TurnEnded {
                    self.ends_of_turn.entry(turn).or_default().push(put);
                }
            }
        }
    }

    /// The places of the history in its order, of those that stay in it
    /// (`Happened::in_history` of what stands there last): the World's put
    /// in a gap, then the host's event there — each handed on as it comes.
    pub fn slots(self) -> impl Iterator<Item = Slot> {
        let Layout { hosted, mut taken, puts, signed, mut over, stays, .. } = self;
        let mut put = puts.into_order().peekable();
        let mut gap = 0usize;
        std::iter::from_fn(move || {
            loop {
                if let Some((at, _)) = put.next_if(|(_, of)| *of == gap) {
                    let in_history = signed.get(&at).map_or(stays[at / 64] & (1 << (at % 64)) != 0, |signed| signed.in_history);
                    if in_history {
                        return Some(Slot { base: Base::Recorded(at), over: over.remove(&at).unwrap_or_default() });
                    }
                    continue;
                }
                if gap >= hosted.len() {
                    return None;
                }
                let here = gap;
                gap += 1;
                let (over, in_history) = match taken.remove(&here) {
                    Some((over, key)) => (over, key.in_history),
                    None => (Vec::new(), hosted.stays(here)),
                };
                if in_history {
                    return Some(Slot { base: Base::Hosted(here), over });
                }
            }
        })
    }
}

/// The merged history laid out (`decide`), its times weighed as nanoseconds
/// where every one is one, else as they are.
pub enum Laid {
    Nanos(Layout<i64>),
    Exact(Layout<Timestamp>),
}

impl Laid {
    /// The places of the history in its order (`Layout::slots`).
    pub fn slots(self) -> Box<dyn Iterator<Item = Slot>> {
        match self {
            Laid::Nanos(layout) => Box::new(layout.slots()),
            Laid::Exact(layout) => Box::new(layout.slots()),
        }
    }
}

/// `decide` of the host's events (`hosted`, at the times it holds) and
/// `recorded`, laid out sparsely: the same history. The host's keys
/// numbered by names closed over `recorded` (`Names::over`) do as well as
/// their own. Their times and the World's are only weighed against each
/// other: as nanoseconds of the epoch where every one is one (§28.51 ⑥-3).
pub fn decide(mut hosted: HostKeys, recorded: &dyn Keys, codex: bool) -> Result<Laid, String> {
    let asked = Asked::of(recorded)?;
    Ok(match hosted.take_moments() {
        Moments::Nanos(times) if asked.nanos => Laid::Nanos(decide_in(Times::new(times), hosted, recorded, asked, codex)?),
        moments => Laid::Exact(decide_in(Times::new(moments.exact()), hosted, recorded, asked, codex)?),
    })
}

/// The World's events' keys as a merge reads them (`decide`): how many, and
/// each in turn, as often as it asks — held, or read back from where they
/// are kept (`timeline::world`).
pub trait Keys {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn each(&self, each: &mut dyn FnMut(usize, &Key) -> Result<(), String>) -> Result<(), String>;
}

/// Keys held.
pub struct Held<'a>(pub &'a [Key]);

impl Keys for Held<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn each(&self, each: &mut dyn FnMut(usize, &Key) -> Result<(), String>) -> Result<(), String> {
        self.0.iter().enumerate().try_for_each(|(at, key)| each(at, key))
    }
}

/// What the World's record can ask of the host's events, gathered of its
/// keys before they are laid out: the items it names with their turns, the
/// drafts its finished items name, when each turn began, its signatures and
/// its turns; and whether each of its times is nanoseconds of the epoch.
struct Asked {
    items: HashSet<(u32, u32)>,
    drafts: HashSet<(u32, u32)>,
    began: HashMap<u32, Timestamp>,
    signatures: HashSet<Signature>,
    turns: HashSet<u32>,
    nanos: bool,
}

impl Asked {
    fn of(recorded: &dyn Keys) -> Result<Self, String> {
        let mut asked =
            Asked { items: HashSet::new(), drafts: HashSet::new(), began: HashMap::new(), signatures: HashSet::new(), turns: HashSet::new(), nanos: true };
        recorded.each(&mut |_, key| {
            if matches!(key.kind, Kind::Finished | Kind::Updated)
                && let (Some(turn), Some(item)) = (key.written_turn, key.item)
            {
                asked.items.insert((turn, item));
            }
            if key.kind == Kind::Finished
                && let (Some(turn), Some(draft)) = (key.written_turn, key.draft)
            {
                asked.drafts.insert((turn, draft));
            }
            // The last start told of a turn, as a map collected in order keeps.
            if key.kind == Kind::TurnStarted
                && let Some(turn) = key.written_turn
            {
                asked.began.insert(turn, key.at);
            }
            if let Some(signature) = signature(key) {
                asked.signatures.insert(signature);
            }
            if let Some(turn) = key.turn {
                asked.turns.insert(turn);
            }
            asked.nanos &= nanos(key.at).is_some();
            Ok(())
        })?;
        Ok(asked)
    }
}

/// Each turn's host events by their numbers, rising: those counted at the
/// start, each turn's held in as much room as it needs, and those a World
/// event's taking a place brought into a turn since, apart (§28.51 ⑥-3).
#[derive(Default)]
struct OfTurn {
    held: HashMap<u32, Vec<u32>>,
    added: HashMap<u32, Vec<u32>>,
}

impl OfTurn {
    /// Of `places`, a turn's each and how many, counted first.
    fn of(places: impl Fn(&mut dyn FnMut(u32, usize))) -> Self {
        let mut counts: HashMap<u32, usize> = HashMap::new();
        places(&mut |turn, _| *counts.entry(turn).or_default() += 1);
        let mut held: HashMap<u32, Vec<u32>> = counts.into_iter().map(|(turn, count)| (turn, Vec::with_capacity(count))).collect();
        places(&mut |turn, index| held.get_mut(&turn).expect("counted").push(u32::try_from(index).expect("fewer events than numbers")));
        OfTurn { held, added: HashMap::new() }
    }

    /// `turn`'s, rising.
    fn of_turn(&self, turn: u32) -> impl Iterator<Item = usize> + '_ {
        let held = self.held.get(&turn).map_or(&[][..], Vec::as_slice);
        let added = self.added.get(&turn).map_or(&[][..], Vec::as_slice);
        let (mut held, mut added) = (held.iter().copied().peekable(), added.iter().copied().peekable());
        std::iter::from_fn(move || match (held.peek(), added.peek()) {
            (Some(one), Some(other)) if other < one => added.next(),
            (Some(_), _) => held.next(),
            (None, _) => added.next(),
        })
        .map(|index| index as usize)
    }

    /// `index` among `turn`'s, where it is not.
    fn insert(&mut self, turn: u32, index: usize) {
        let index = u32::try_from(index).expect("fewer events than numbers");
        if self.held.get(&turn).is_some_and(|held| held.binary_search(&index).is_ok()) {
            return;
        }
        let added = self.added.entry(turn).or_default();
        if let Err(slot) = added.binary_search(&index) {
            added.insert(slot, index);
        }
    }
}

fn decide_in<T: Moment + 'static>(times: Times<T>, hosted: HostKeys, recorded: &dyn Keys, asked: Asked, codex: bool) -> Result<Layout<T>, String> {
    let Asked { items: asked, drafts, began, signatures, turns, .. } = asked;
    let items = |key: &Key| matches!(key.kind, Kind::Finished | Kind::Updated);
    // Only what the World's record can ask of the host's events is indexed.
    let held: HashSet<(u32, u32)> = (0..hosted.len())
        .map(|index| hosted.key(index))
        .filter(|key| items(key))
        .filter_map(|key| Some((key.written_turn?, key.item?)))
        .filter(|pair| asked.contains(pair))
        .collect();
    let mut by_signature: HashMap<Signature, Vec<usize>> = HashMap::new();
    for (index, key) in (0..hosted.len()).map(|index| (index, hosted.key(index))) {
        if let Some(signature) = signature(&key).filter(|signature| signatures.contains(signature)) {
            by_signature.entry(signature).or_default().push(index);
        }
    }
    let of_turn = |ends: bool| {
        let (hosted, turns) = (&hosted, &turns);
        move |each: &mut dyn FnMut(u32, usize)| {
            for index in 0..hosted.len() {
                let key = hosted.key(index);
                if let Some(turn) = key.turn.filter(|turn| turns.contains(turn) && (!ends || key.kind == Kind::TurnEnded)) {
                    each(turn, index);
                }
            }
        }
    };
    let mut by_turn = OfTurn::of(of_turn(false));
    // The ends of each turn, apart: found without reading all of the turn.
    let mut ends_of = OfTurn::of(of_turn(true));
    let mut moments = Vec::with_capacity(recorded.len());
    recorded.each(&mut |_, key| {
        moments.push(T::of(key.at));
        Ok(())
    })?;
    let mut layout = Layout {
        hosted,
        times,
        taken: HashMap::new(),
        puts: Puts::new(moments),
        signed: HashMap::new(),
        over: HashMap::new(),
        by_signature: HashMap::new(),
        signed_of_turn: HashMap::new(),
        ends_of_turn: HashMap::new(),
        first_unsigned: HashMap::new(),
        stays: vec![0; recorded.len().div_ceil(64)],
    };
    let mut opened: HashSet<u32> = HashSet::new();
    recorded.each(&mut |at, event| {
        if items(event)
            && let (Some(turn), Some(item)) = (event.written_turn, event.item)
            && (held.contains(&(turn, item)) || drafts.contains(&(turn, item)))
        {
            return Ok(());
        }
        if codex && event.kind == Kind::Heard && event.native {
            return Ok(());
        }
        // The first place of the same event: taken by this one. What takes a
        // place is the same as what stood there (`same`), so what it is
        // indexed by stays.
        let hosted_same = signature(event)
            .and_then(|signature| by_signature.get(&signature))
            .and_then(|places| places.iter().copied().find(|index| same(event, &layout.hosted_key(*index))))
            .map(Place::Hosted);
        let put_same = layout.put_same(event).map(Place::Put);
        match layout.earliest(hosted_same, put_same) {
            Some(Place::Hosted(index)) => {
                let stood = layout.hosted_key(index);
                let entry = layout.taken.entry(index).or_insert_with(|| (Vec::new(), stood));
                let was = entry.1.turn;
                entry.0.push(at);
                entry.1 = *event;
                layout.times.set(index, T::of(event.at));
                // Its turn may be one now that it was not (voice is matched
                // by its item): it is found among that turn's too.
                if let Some(turn) = event.turn.filter(|turn| was != Some(*turn) && turns.contains(turn)) {
                    by_turn.insert(turn, index);
                    if event.kind == Kind::TurnEnded {
                        ends_of.insert(turn, index);
                    }
                }
                return Ok(());
            }
            Some(Place::Put(put)) => {
                layout.take_put(put, at, event);
                return Ok(());
            }
            None => {}
        }
        let turn = event.turn;
        let opening = match (event.kind, turn) {
            (Kind::Heard, Some(turn)) if !opened.contains(&turn) => began
                .get(&turn)
                .is_some_and(|start| event.at.as_millisecond().saturating_sub(start.as_millisecond()) <= super::super::OPENING_MS),
            _ => false,
        };
        if let Some(turn) = turn.filter(|_| opening) {
            opened.insert(turn);
            // What opened the turn goes before the first of it but its start.
            let in_turn = |key: &Key| key.turn == Some(turn) && key.kind != Kind::TurnStarted;
            let hosted_first = by_turn.of_turn(turn).find(|index| in_turn(&layout.hosted_key(*index))).map(Place::Hosted);
            let put_first = layout.first_in_turn(turn).map(Place::Put);
            if let Some(first) = layout.earliest(hosted_first, put_first) {
                layout.insert(Some(first), at, *event);
                return Ok(());
            }
        }
        // The first place before which it goes: the end of its turn; with
        // its turn's start, anything of the turn; else what is later, but
        // not its turn's start.
        let same_turn = |key: &Key| turn.is_some() && turn == key.turn;
        // With its turn's start, the first of the turn; else the first end
        // of it — each from its own list.
        let of_turn = turn.and_then(|turn| {
            let mut places = if event.kind == Kind::TurnStarted { by_turn.of_turn(turn) } else { ends_of.of_turn(turn) };
            places.find(|index| {
                let key = layout.hosted_key(*index);
                same_turn(&key) && (key.kind == Kind::TurnEnded || event.kind == Kind::TurnStarted)
            })
        });
        let mut from = 0;
        let later = loop {
            let Some(index) = layout.times.first_later(from, T::of(event.at)) else { break None };
            let key = layout.hosted_key(index);
            if key.kind == Kind::TurnStarted && same_turn(&key) {
                from = index + 1;
                continue;
            }
            break Some(index);
        };
        let hosted_before = match (of_turn, later) {
            (Some(one), Some(other)) => Some(one.min(other)),
            (one, other) => one.or(other),
        }
        .map(Place::Hosted);
        // Of those put: the first end of its turn, or with its turn's start
        // the first of the turn (those signed and the first unsigned); and
        // the first later than it but its turn's start — a start is signed.
        let put_of_turn = turn.and_then(|turn| {
            if event.kind == Kind::TurnStarted {
                layout.first_of_turn(turn)
            } else {
                layout.puts.first_of(layout.ends_of_turn.get(&turn).into_iter().flatten().copied())
            }
        });
        let moment = layout.puts.moment(at);
        let mut after = None;
        let put_later = loop {
            let Some(put) = layout.puts.first_later(after, moment) else { break None };
            if layout.signed.get(&put).is_some_and(|signed| signed.kind == Kind::TurnStarted && turn.is_some() && signed.turn == turn) {
                after = Some(put);
                continue;
            }
            break Some(put);
        };
        let put_before = layout.puts.first_of(put_of_turn.into_iter().chain(put_later)).map(Place::Put);
        let place = layout.earliest(hosted_before, put_before);
        layout.insert(place, at, *event);
        Ok(())
    })?;
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Choices(u64);
    impl Choices {
        fn pick(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    fn at(second: i64, nanosecond: i32) -> Timestamp {
        Timestamp::new(1_800_000_000 + second, nanosecond).unwrap()
    }

    /// A key of any kind, its fields drawn each on its own from a few values
    /// — so that a place taken may take another turn or time, as no event
    /// the merge is given need keep them alike.
    fn key(choices: &mut Choices, spread: i64) -> Key {
        let kind = match choices.pick(8) {
            0 => Kind::TurnStarted,
            1 => Kind::TurnEnded,
            2 | 3 => Kind::Heard,
            4 => Kind::Finished,
            5 => Kind::Updated,
            6 => Kind::Realtime,
            _ => Kind::Other,
        };
        let mut small = |n: u64| if choices.pick(4) == 0 { None } else { Some(choices.pick(n) as u32) };
        let (turn, written_turn, message, item, draft) = (small(3), small(3), small(3), small(3), small(3));
        let realtime = (kind == Kind::Realtime || choices.pick(6) == 0).then(|| (choices.pick(2) == 0, choices.pick(2) as u32, choices.pick(2) as u32));
        Key {
            at: at(choices.pick(spread as u64) as i64, if choices.pick(3) == 0 { choices.pick(1_000) as i32 } else { 0 }),
            kind,
            turn,
            written_turn,
            message,
            item,
            draft,
            realtime,
            native: choices.pick(4) == 0,
            in_history: choices.pick(6) != 0,
        }
    }

    /// The merge as it was and the sparse one, of the host's keys as they
    /// are held for it (`HostKeys`: no draft, nor whether native).
    fn both(hosted: &[Key], recorded: &[Key], codex: bool) -> (Vec<Slot>, Vec<Slot>) {
        let mut held = HostKeys::default();
        for key in hosted {
            held.push(key);
        }
        (super::super::decide(hosted, recorded, codex), decide(held, &Held(recorded), codex).expect("held keys").slots().collect())
    }

    /// The sparse merge lays out the history the merge it stands for does:
    /// over many small histories of every kind, and a few long ones whose
    /// times span levels of `Times`.
    #[test]
    fn the_sparse_merge_is_the_merge() {
        let mut choices = Choices(0x9e37_79b9_7f4a_7c15);
        for round in 0..4_000 {
            let long = round % 400 == 0;
            let hosted_count = if long { 5_000 } else { choices.pick(150) as usize };
            let recorded_count = if long { 120 } else { choices.pick(30) as usize };
            let spread = if long { 20_000 } else { 60 };
            let hosted: Vec<Key> = (0..hosted_count).map(|_| key(&mut choices, spread)).collect();
            let recorded: Vec<Key> = (0..recorded_count).map(|_| key(&mut choices, spread)).collect();
            let codex = choices.pick(2) == 0;
            let (was, now) = both(&hosted, &recorded, codex);
            assert_eq!(now, was, "round {round}");
        }
    }

    /// The host's names the World's record never names, all one number
    /// (`ELSEWHERE`), as names closed over the World's record give them
    /// (`Names::over`): the merge lays out the history it lays out with each
    /// its own.
    #[test]
    fn names_the_world_never_names_merge_as_their_own() {
        let mut choices = Choices(0x2545_f491_4f6c_dd1d);
        for round in 0..2_000 {
            let hosted: Vec<Key> = (0..choices.pick(120) as usize).map(|_| key(&mut choices, 60)).collect();
            let recorded: Vec<Key> = (0..choices.pick(25) as usize).map(|_| key(&mut choices, 60)).collect();
            // Every name the World's keys hold, whichever field holds it.
            let named: HashSet<u32> = recorded
                .iter()
                .flat_map(|key| {
                    let realtime = key.realtime.map(|(_, item, session)| [item, session]).unwrap_or_default();
                    [key.turn, key.written_turn, key.message, key.item, key.draft].into_iter().flatten().chain(realtime)
                })
                .collect();
            let closed = |name: Option<u32>| name.map(|name| if named.contains(&name) { name } else { super::super::ELSEWHERE });
            let mut collapsed = HostKeys::default();
            for key in &hosted {
                collapsed.push(&Key {
                    turn: closed(key.turn),
                    written_turn: closed(key.written_turn),
                    message: closed(key.message),
                    item: closed(key.item),
                    realtime: key.realtime.map(|(complete, item, session)| {
                        (complete, closed(Some(item)).expect("named"), closed(Some(session)).expect("named"))
                    }),
                    ..*key
                });
            }
            let codex = choices.pick(2) == 0;
            let was = super::super::decide(&hosted, &recorded, codex);
            let now: Vec<Slot> = decide(collapsed, &Held(&recorded), codex).expect("held keys").slots().collect();
            assert_eq!(now, was, "round {round}");
        }
    }

    /// A World's record shaped as a hosted conversation's is — per turn its
    /// start, its opening words, many model requests with their answers and
    /// usage, some usage late after its end, its end — with times a little
    /// out of order, turns started or ended twice, words and voice the host
    /// has too, among the host's events of each turn: the sparse merge lays
    /// out what the merge does, with many of the World's events put among
    /// the host's (design §28.51 ⑥-2).
    #[test]
    fn a_long_world_record_merges_as_the_merge() {
        let mut choices = Choices(0x51ed_270b_c4a1_9e03);
        for round in 0..60 {
            let turns = 1 + choices.pick(if round % 10 == 0 { 40 } else { 8 }) as u32;
            let (mut hosted, mut recorded) = (Vec::new(), Vec::new());
            let jitter = |choices: &mut Choices, millis: i64| if choices.pick(6) == 0 { millis - choices.pick(40_000) as i64 } else { millis };
            for turn in 0..turns {
                let start = i64::from(turn) * 1_000_000;
                let plain = |kind: Kind, turn: Option<u32>, millis: i64| Key {
                    at: Timestamp::from_millisecond(1_800_000_000_000 + millis).unwrap(),
                    kind,
                    turn,
                    written_turn: matches!(kind, Kind::TurnStarted | Kind::TurnEnded).then_some(turn).flatten(),
                    message: None,
                    item: None,
                    draft: None,
                    realtime: None,
                    native: false,
                    in_history: true,
                };
                let some = Some(turn);
                if choices.pick(4) == 0 {
                    hosted.push(plain(Kind::TurnStarted, some, start + 3));
                }
                recorded.push(plain(Kind::TurnStarted, some, start));
                if choices.pick(8) == 0 {
                    recorded.push(plain(Kind::TurnStarted, some, start + 2));
                }
                let words = Key { message: Some(1_000 + turn), ..plain(Kind::Heard, some, start + choices.pick(3_000) as i64) };
                recorded.push(words);
                if choices.pick(3) == 0 {
                    hosted.push(Key { at: Timestamp::from_millisecond(1_800_000_000_000 + start + 7).unwrap(), ..words });
                }
                if choices.pick(4) == 0 {
                    // Words with no message, and usage before the turn's words.
                    recorded.push(plain(Kind::Heard, some, start + 1));
                    recorded.push(plain(Kind::Other, some, start + 1));
                }
                let steps = choices.pick(30) as i64;
                for step in 0..steps {
                    let at = start + 10 + step * 20_000;
                    hosted.push(plain(if choices.pick(5) == 0 { Kind::Finished } else { Kind::Other }, some, at + 5));
                    if choices.pick(2) == 0 {
                        hosted.push(plain(Kind::Other, some, at + 9_000));
                    }
                    if choices.pick(9) == 0 {
                        hosted.push(plain(Kind::Other, None, at + 9_500));
                    }
                    recorded.push(plain(Kind::Other, some, jitter(&mut choices, at)));
                    recorded.push(plain(Kind::Other, some, jitter(&mut choices, at + 10_000)));
                    for usage in 0..choices.pick(4) as i64 {
                        recorded.push(Key { in_history: choices.pick(7) != 0, ..plain(Kind::Other, if choices.pick(10) == 0 { None } else { some }, at + 10_001 + usage) });
                    }
                    if choices.pick(12) == 0 {
                        let realtime = Some((choices.pick(2) == 0, choices.pick(3) as u32, 7));
                        let voice = Key { realtime, ..plain(Kind::Realtime, if choices.pick(2) == 0 { some } else { Some(choices.pick(u64::from(turns)) as u32) }, at + 11_000) };
                        recorded.push(voice);
                        if choices.pick(2) == 0 {
                            hosted.push(Key { turn: some, ..voice });
                        }
                    }
                }
                let end = start + 800_000;
                if choices.pick(3) == 0 {
                    hosted.push(plain(Kind::TurnEnded, some, end + 1));
                }
                recorded.push(plain(Kind::TurnEnded, some, end));
                // Usage told late, after its turn's end, and an end told twice.
                for late in 0..choices.pick(4) as i64 {
                    recorded.push(plain(Kind::Other, some, jitter(&mut choices, end + 10 + late)));
                }
                if choices.pick(10) == 0 {
                    recorded.push(plain(Kind::TurnEnded, some, end + 50));
                }
            }
            let codex = choices.pick(2) == 0;
            let (was, now) = both(&hosted, &recorded, codex);
            assert_eq!(now, was, "round {round}: hosted {} recorded {}", hosted.len(), recorded.len());
        }
    }

    /// A World's record shaped as a hosted Claude conversation's is (per
    /// turn its start, the words, many model requests with their answers
    /// and usage, its end), among the host's events of each turn: how long
    /// the sparse merge takes as it grows. Timing only.
    #[test]
    #[ignore]
    fn timing_a_long_world_record() {
        let plain = |kind: Kind, turn: u32, millis: i64| Key {
            at: Timestamp::from_millisecond(1_800_000_000_000 + millis).unwrap(),
            kind,
            turn: Some(turn),
            written_turn: matches!(kind, Kind::TurnStarted | Kind::TurnEnded).then_some(turn),
            message: (kind == Kind::Heard).then_some(turn),
            item: None,
            draft: None,
            realtime: None,
            native: false,
            in_history: true,
        };
        for turns in [120u32, 480, 1_920, 7_680, 30_720] {
            let (mut hosted, mut recorded) = (Vec::new(), Vec::new());
            for turn in 0..turns {
                let start = i64::from(turn) * 1_000_000;
                recorded.push(plain(Kind::TurnStarted, turn, start));
                recorded.push(plain(Kind::Heard, turn, start + 1));
                for step in 0..35 {
                    let at = start + 10 + step * 20_000;
                    hosted.push(plain(Kind::Other, turn, at + 5));
                    hosted.push(plain(Kind::Other, turn, at + 9_000));
                    recorded.push(plain(Kind::Other, turn, at));
                    recorded.push(plain(Kind::Other, turn, at + 10_000));
                    for usage in 0..3 {
                        recorded.push(plain(Kind::Other, turn, at + 10_001 + usage));
                    }
                }
                recorded.push(plain(Kind::Other, turn, start + 800_000));
                recorded.push(plain(Kind::TurnEnded, turn, start + 900_000));
            }
            let mut held = HostKeys::default();
            for key in &hosted {
                held.push(key);
            }
            let began = std::time::Instant::now();
            let slots = decide(held, &Held(&recorded), false).expect("held keys").slots().count();
            println!("turns {turns}: hosted {} recorded {} slots {slots}: {:.2} s", hosted.len(), recorded.len(), began.elapsed().as_secs_f64());
        }
    }

    /// Times no nanoseconds of the epoch can hold — the host's or the
    /// World's, before 1677 or after 2262 — weighed as they are: the same
    /// history as the merge lays out (design §28.51 ⑥-3).
    #[test]
    fn times_beyond_nanoseconds_merge_as_the_merge() {
        let mut choices = Choices(0x7f4a_7c15_9e37_79b9);
        let far = |key: Key, choices: &mut Choices| match choices.pick(40) {
            0 => Key { at: Timestamp::new(10_000_000_000 + key.at.as_second() % 1_000, 0).unwrap(), ..key },
            1 => Key { at: Timestamp::new(-10_000_000_000 + key.at.as_second() % 1_000, 0).unwrap(), ..key },
            _ => key,
        };
        for round in 0..600 {
            let hosted: Vec<Key> = (0..choices.pick(120) as usize).map(|_| { let key = key(&mut choices, 60); far(key, &mut choices) }).collect();
            let recorded: Vec<Key> = (0..choices.pick(30) as usize).map(|_| { let key = key(&mut choices, 60); far(key, &mut choices) }).collect();
            let codex = choices.pick(2) == 0;
            let (was, now) = both(&hosted, &recorded, codex);
            assert_eq!(now, was, "round {round}");
        }
    }

    /// The places its review found a merge by indices could miss.
    #[test]
    fn the_places_a_merge_by_indices_could_miss() {
        let plain = |kind: Kind, turn: Option<u32>, second: i64| Key {
            at: at(second, 0),
            kind,
            turn,
            written_turn: matches!(kind, Kind::TurnStarted | Kind::TurnEnded).then_some(turn).flatten(),
            message: None,
            item: None,
            draft: None,
            realtime: None,
            native: false,
            in_history: true,
        };
        let heard = |turn: u32, message: u32| Key { message: Some(message), ..plain(Kind::Heard, Some(turn), 0) };
        // Words of one message in two turns: the second taken, not the first.
        let (was, now) = both(&[heard(1, 7), heard(2, 7)], &[heard(2, 7)], false);
        assert_eq!(now, was);
        assert_eq!(now[1].over, vec![0]);
        // A start taken earlier than it stood: what comes later is the next.
        let hosted = [plain(Kind::TurnStarted, Some(1), 100), plain(Kind::Other, None, 200)];
        let (was, now) = both(&hosted, &[plain(Kind::TurnStarted, Some(1), 10), plain(Kind::Other, None, 50)], false);
        assert_eq!(now, was);
        // Its turn's start passed over.
        let (was, now) = both(&hosted, &[plain(Kind::Other, Some(1), 50)], false);
        assert_eq!(now, was);
        // With no host's events, the World's own taken by its own.
        let (was, now) = both(&[], &[heard(1, 3), heard(1, 3)], false);
        assert_eq!(now, was);
        assert_eq!(now.len(), 1);
    }
}
