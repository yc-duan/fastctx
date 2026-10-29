//! A watched conversation as its page holds it (`world-gui.md`, 对话: 很长
//! 的对话). The shell keeps every event of the conversation, as the JSON
//! the page reads; the page holds only the part near its reader, the latest
//! turns at first, and asks for the rest a part at a time. A conversation
//! of tens of thousands of tool calls is then as light in the page as a
//! short one, and nothing the page draws waits for the whole of it.
//!
//! Parts begin and end only between turns, at a cut no later event crosses:
//! every name an event refers to (its turn, its item where it has no turn,
//! the words it delivers, the request it answers) is first named on the same
//! side of the cut, so a page folds each part on its own and two parts never
//! hold the same thing. A late event that names something before a cut
//! takes the cut away. Facts a run tells after its turn are one exception,
//! as a job goes on for as long as it likes: they are named by the run, and
//! a part brings along those told after it. What the host's own agents do
//! is the other, as one goes on in the background: it is named by the call
//! that started the agent, and comes along with that call's part.
//!
//! Finding words reaches the whole conversation: the page hands the shell
//! the displayed text of parts it does not hold, once, and the shell counts
//! the matches in them.

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};

use fastctx_client::Update;
use fastctx_types::session::{Happened, SessionChange, SessionEvent, What};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::Piece;

/// A part is the turns that hold at least this many events or bytes of
/// them, whichever comes first. The page folds a part in one go and draws a
/// few screens of it at once: 400 events are some ten to forty turns of an
/// agent at work, more than the screens it opens on, and fold in about a
/// millisecond; a megabyte bounds the parts whose events are few but large
/// (long outputs, long words).
const PART_EVENTS: usize = 400;
const PART_BYTES: u64 = 1 << 20;

/// The conversations watched, by the watch's id.
#[derive(Default)]
pub(crate) struct Transcripts(Mutex<HashMap<u64, Arc<Mutex<Transcript>>>>);

impl Transcripts {
    pub(crate) fn open(&self, id: u64, transcript: Transcript) -> Arc<Mutex<Transcript>> {
        let shared = Arc::new(Mutex::new(transcript));
        self.0.lock().expect("transcript table").insert(id, shared.clone());
        shared
    }

    fn get(&self, id: u64) -> Option<Arc<Mutex<Transcript>>> {
        self.0.lock().expect("transcript table").get(&id).cloned()
    }

    pub(crate) fn close(&self, id: u64) {
        self.0.lock().expect("transcript table").remove(&id);
    }

    pub(crate) fn close_all(&self) {
        self.0.lock().expect("transcript table").clear();
    }
}

/// The part a page holds, in place of all it held.
#[derive(Clone, Serialize)]
pub(crate) struct Window {
    generation: u64,
    /// Where its events begin, among all the shell holds.
    start: usize,
    /// How many turns come before it.
    turns_before: usize,
    events: Vec<Box<RawValue>>,
    /// What its turns read of what came before them: the conversation's
    /// title, its context, the model that answered last and the host's
    /// agents, as they last stood before `start`.
    carry: Vec<Box<RawValue>>,
}

/// A part a page asked for.
#[derive(Serialize)]
pub(crate) struct Part {
    generation: u64,
    start: usize,
    end: usize,
    turns_before: usize,
    turns: usize,
    events: Vec<Box<RawValue>>,
    /// Facts its runs told after `end`, as their jobs went on.
    late: Vec<Box<RawValue>>,
}

/// Which part a page asks for: the one just before or just after a part it
/// holds, or the one around an event; never reaching into what it holds
/// beyond `floor` and `ceiling`.
#[derive(Deserialize)]
#[serde(tag = "edge", rename_all = "snake_case")]
pub(crate) enum Edge {
    Before { at: usize, floor: usize },
    After { at: usize, ceiling: usize },
    Around { at: usize, floor: usize, ceiling: usize },
}

/// Where a page can let go of what comes before its latest part.
#[derive(Serialize)]
pub(crate) struct Cut {
    generation: u64,
    at: usize,
    turns_before: usize,
}

/// Displayed text of one part of a block, lowercased as the page compares,
/// and the event its block begins at.
#[derive(Deserialize)]
pub(crate) struct Doc {
    block: String,
    part: String,
    text: String,
    at: usize,
}

/// What the shell found of words in the parts the page indexed.
#[derive(Serialize)]
pub(crate) struct Found {
    generation: u64,
    ranges: Vec<FoundRange>,
    /// What neither the page holds nor the shell has the text of yet, a
    /// part at a time.
    unindexed: Vec<(usize, usize)>,
}

#[derive(Serialize)]
pub(crate) struct FoundRange {
    start: usize,
    end: usize,
    /// Each part with matches, in the order the page shows them: its block,
    /// its part, how many, and where its block begins.
    hits: Vec<(String, String, u32, usize)>,
}

struct Indexed {
    end: usize,
    docs: Vec<Doc>,
}

#[derive(Default)]
pub(crate) struct Transcript {
    /// Moves whenever what is held is replaced: an answer about an earlier
    /// reading is no answer.
    generation: u64,
    events: crate::held_events::HeldEvents,
    hashes: Vec<u64>,
    /// Bytes of JSON before each event, and after the last.
    bytes: Vec<u64>,
    /// Where each turn begins.
    units: Vec<usize>,
    /// Each name events refer to, by the first event that did.
    first: HashMap<String, usize>,
    previous_started: bool,
    /// Each tool by its item, at its first event, in order.
    tools: Vec<(usize, String)>,
    seen_tools: std::collections::HashSet<String>,
    /// The run facts told, and what the host's own agents did, by the call
    /// they tell of: the run's, the one that started the agent.
    facts: HashMap<String, Vec<usize>>,
    /// The call that started each of the host's own agents, by the agent.
    agent_calls: HashMap<String, String>,
    titled: Option<usize>,
    context: Option<usize>,
    answered: Option<usize>,
    agents: Option<usize>,
    /// The displayed text of parts a page indexed, by where each begins.
    texts: BTreeMap<usize, Indexed>,
}

impl Transcript {
    /// A conversation whose history is `history`, and the window a page
    /// opens on: its latest part.
    pub(crate) fn new(history: &[SessionEvent]) -> Result<(Self, Window), String> {
        let mut transcript = Self { bytes: vec![0], units: vec![0], ..Self::default() };
        for event in history {
            transcript.admit(event)?;
        }
        let window = transcript.window();
        Ok((transcript, window))
    }

    /// What a page's watch is given for what the client handed over; nothing
    /// when the history handed over is what is held.
    pub(crate) fn take(&mut self, update: Update) -> Result<Option<Piece>, String> {
        Ok(Some(match update {
            Update::Current => Piece::Resumed,
            Update::Interrupted(interruption) => Piece::interrupted(&interruption),
            Update::Moment(moment) => {
                let at = self.events.len();
                self.admit(&moment.event)?;
                Piece::Event { at, event: self.events.get(at) }
            }
            Update::History(history) => return self.history(&history),
            Update::Now { busy, turn, kept } => Piece::Now { busy, turn, kept },
            Update::Extended(history) => {
                let start = self.events.len();
                for event in &history {
                    self.admit(event)?;
                }
                self.extended(start)
            }
        }))
    }

    /// The history as it now is, in place of what is held: more after what
    /// is held, more before it, or a different history.
    pub(crate) fn history(&mut self, history: &[SessionEvent]) -> Result<Option<Piece>, String> {
        let fresh = history.iter().map(json).collect::<Result<Vec<_>, _>>()?;
        let held = self.hashes.len();
        let same =
            |offset: usize| fresh[offset..offset + held].iter().map(|(_, hash)| *hash).eq(self.hashes.iter().copied());
        if fresh.len() >= held && same(0) {
            if fresh.len() == held {
                return Ok(None);
            }
            for (event, (text, hash)) in history[held..].iter().zip(fresh.into_iter().skip(held)) {
                self.keep(event, text, hash)?;
            }
            return Ok(Some(self.extended(held)));
        }
        // What came before the page's part arrived, as the rest of what this
        // device keeps does after its latest part: the page keeps all it
        // holds, moved along by what came before it.
        let offset = (held > 0 && fresh.len() > held && same(fresh.len() - held)).then(|| fresh.len() - held);
        let texts = std::mem::take(&mut self.texts);
        let generation = self.generation + 1;
        *self = Self { bytes: vec![0], units: vec![0], generation, ..Self::default() };
        for (event, (text, hash)) in history.iter().zip(fresh) {
            self.keep(event, text, hash)?;
        }
        if let Some(offset) = offset {
            self.texts = texts
                .into_iter()
                .map(|(start, Indexed { end, mut docs })| {
                    for doc in &mut docs {
                        doc.at += offset;
                    }
                    (start + offset, Indexed { end: end + offset, docs })
                })
                .collect();
            return Ok(Some(Piece::Rebased { generation, offset, turns_added: self.turns_before(offset) }));
        }
        Ok(Some(Piece::Window { window: self.window() }))
    }

    /// Events kept after `start` go to the page as they are, when they are
    /// about a part's worth; more than that, and the page opens again on the
    /// latest part.
    fn extended(&mut self, start: usize) -> Piece {
        let end = self.events.len();
        if end - start >= PART_EVENTS || self.bytes[end] - self.bytes[start] >= PART_BYTES {
            self.generation += 1;
            self.texts.clear();
            return Piece::Window { window: self.window() };
        }
        Piece::Extended { start, events: self.events.range(start, end) }
    }

    /// Replaces the page after a delivery gap even if the stored history matches:
    /// it may have missed a piece already admitted to this transcript.
    pub(crate) fn recovered(&mut self, history: &[SessionEvent]) -> Result<Piece, String> {
        self.history(history)?;
        self.generation += 1;
        self.texts.clear();
        Ok(Piece::Window { window: self.window() })
    }

    fn window(&self) -> Window {
        let start = self.latest(PART_EVENTS, PART_BYTES);
        let carry = [self.titled, self.context, self.answered, self.agents]
            .into_iter()
            .flatten()
            .filter(|&at| at < start)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|at| self.events.get(at))
            .collect();
        Window {
            generation: self.generation,
            start,
            turns_before: self.turns_before(start),
            events: self.events.range(start, self.events.len()),
            carry,
        }
    }

    fn admit(&mut self, event: &SessionEvent) -> Result<(), String> {
        let (text, hash) = json(event)?;
        self.keep(event, text, hash)
    }

    fn keep(&mut self, event: &SessionEvent, text: String, hash: u64) -> Result<(), String> {
        let index = self.events.len();
        self.bytes.push(self.bytes[index] + text.len() as u64);
        self.hashes.push(hash);
        self.events.push(RawValue::from_string(text).map_err(|error| error.to_string())?)?;
        let mut fresh = true;
        let mut earliest: Option<usize> = None;
        for name in names(&event.happened) {
            match self.first.get(&name) {
                Some(&at) => {
                    fresh = false;
                    earliest = Some(earliest.map_or(at, |earliest| earliest.min(at)));
                }
                None => {
                    self.first.insert(name, index);
                }
            }
        }
        // A name from before a cut takes the cut away.
        if let Some(at) = earliest {
            while self.units.len() > 1 && self.units.last().is_some_and(|&unit| unit > at) {
                self.units.pop();
            }
        }
        // A turn begins where it starts, or with the words that start it.
        let starts = match &event.happened {
            Happened::TurnStarted { .. } => true,
            Happened::Heard { .. } => !self.previous_started,
            _ => false,
        };
        if starts && fresh && self.units.last().is_some_and(|&unit| index > unit) {
            self.units.push(index);
        }
        self.previous_started = matches!(event.happened, Happened::TurnStarted { .. });
        match &event.happened {
            Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. }
                if matches!(item.what, What::Tool(_) | What::AgentActivity { .. })
                    && self.seen_tools.insert(item.id.clone()) =>
            {
                self.tools.push((index, item.id.clone()));
            }
            Happened::Changed { change: SessionChange::RunFactsUpdated { update, .. }, .. } => {
                self.facts.entry(update.call_id.clone()).or_default().push(index);
            }
            Happened::Changed { change: SessionChange::AgentStarted { agent }, .. } => {
                if let Some(call) = &agent.call {
                    self.agent_calls.insert(agent.id.clone(), call.clone());
                }
                if let Some(call) = self.agent_calls.get(&agent.id) {
                    self.facts.entry(call.clone()).or_default().push(index);
                }
            }
            Happened::Changed {
                change: SessionChange::AgentEvent { agent_id, .. } | SessionChange::AgentEnded { agent_id, .. },
                ..
            } => {
                if let Some(call) = self.agent_calls.get(agent_id) {
                    self.facts.entry(call.clone()).or_default().push(index);
                }
            }
            Happened::Titled { .. } => self.titled = Some(index),
            Happened::Context { .. } => self.context = Some(index),
            Happened::Changed {
                change: SessionChange::ModelResponse { usage: Some(_), .. } | SessionChange::MessageUsage { .. },
                ..
            } => self.answered = Some(index),
            Happened::Changed { change: SessionChange::Agents { .. }, .. } => self.agents = Some(index),
            _ => {}
        }
        Ok(())
    }

    /// Where the latest turns holding `events` events or `bytes` bytes
    /// begin; the latest turn whatever it holds.
    fn latest(&self, events: usize, bytes: u64) -> usize {
        let end = self.events.len();
        for &start in self.units.iter().rev() {
            if end - start >= events || self.bytes[end] - self.bytes[start] >= bytes {
                return start;
            }
        }
        0
    }

    fn turns_before(&self, at: usize) -> usize {
        self.units.partition_point(|&start| start < at)
    }

    /// Where the turn holding event `at` begins.
    fn turn_of(&self, at: usize) -> usize {
        self.units[self.units.partition_point(|&start| start <= at).max(1) - 1]
    }

    /// The part ending at `end`, from no earlier than `floor`.
    fn before(&self, end: usize, floor: usize, share: usize) -> usize {
        let mut at = self.units.partition_point(|&start| start < end);
        while at > 0 {
            at -= 1;
            let start = self.units[at];
            if start <= floor {
                return floor;
            }
            if end - start >= PART_EVENTS / share || self.bytes[end] - self.bytes[start] >= PART_BYTES / share as u64 {
                return start;
            }
        }
        floor
    }

    /// The part beginning at `start`, ending no later than `ceiling`.
    fn after(&self, start: usize, ceiling: usize, share: usize) -> usize {
        let mut at = self.units.partition_point(|&unit| unit <= start);
        while at < self.units.len() {
            let end = self.units[at];
            if end >= ceiling {
                return ceiling;
            }
            if end - start >= PART_EVENTS / share || self.bytes[end] - self.bytes[start] >= PART_BYTES / share as u64 {
                return end;
            }
            at += 1;
        }
        ceiling
    }

    pub(crate) fn part(&self, generation: u64, edge: Edge) -> Option<Part> {
        if generation != self.generation {
            return None;
        }
        let len = self.events.len();
        let (start, end) = match edge {
            Edge::Before { at, floor } => {
                let end = at.min(len);
                (self.before(end, floor.min(end), 1), end)
            }
            Edge::After { at, ceiling } => {
                let ceiling = ceiling.min(len);
                let start = at.min(ceiling);
                (start, self.after(start, ceiling, 1))
            }
            Edge::Around { at, floor, ceiling } => self.around(at, floor, ceiling),
        };
        if start >= end {
            return None;
        }
        Some(Part {
            generation,
            start,
            end,
            turns_before: self.turns_before(start),
            turns: self.turns_before(end) - self.turns_before(start),
            events: self.events.range(start, end),
            late: self.late(start, end),
        })
    }

    /// The turns around event `at`, half a part on each side of its own.
    fn around(&self, at: usize, floor: usize, ceiling: usize) -> (usize, usize) {
        let ceiling = ceiling.min(self.events.len());
        let floor = floor.min(ceiling);
        let own = self.turn_of(at.min(ceiling.saturating_sub(1))).clamp(floor, ceiling);
        (self.before(own, floor, 2), self.after(own, ceiling, 2))
    }

    /// Facts told after `end` by the runs that began in `start..end`, and
    /// what the agents its calls started did after it.
    fn late(&self, start: usize, end: usize) -> Vec<Box<RawValue>> {
        let first = self.tools.partition_point(|(at, _)| *at < start);
        let mut late: Vec<usize> = self.tools[first..]
            .iter()
            .take_while(|(at, _)| *at < end)
            .filter_map(|(_, item)| self.facts.get(item))
            .flat_map(|told| told.iter().copied().filter(|&at| at >= end))
            .collect();
        late.sort_unstable();
        late.into_iter().map(|at| self.events.get(at)).collect()
    }

    /// Where a turn first appears.
    pub(crate) fn locate(&self, generation: u64, turn: &str) -> Option<usize> {
        if generation != self.generation {
            return None;
        }
        self.first.get(&format!("t:{turn}")).copied()
    }

    /// Where a page whose latest part has grown can let go of what comes
    /// before its latest `events` events.
    pub(crate) fn cut(&self, generation: u64, events: usize) -> Option<Cut> {
        if generation != self.generation {
            return None;
        }
        let at = self.latest(events, u64::MAX);
        Some(Cut { generation, at, turns_before: self.turns_before(at) })
    }

    /// Keeps the displayed text of `start..end` for finding.
    pub(crate) fn index(&mut self, generation: u64, start: usize, end: usize, docs: Vec<Doc>) -> bool {
        if generation != self.generation || start >= end || end > self.events.len() {
            return false;
        }
        // A part indexed again in other bounds replaces what it overlaps.
        let overlapping: Vec<usize> =
            self.texts.range(..end).filter(|(_, indexed)| indexed.end > start).map(|(from, _)| *from).collect();
        for from in overlapping {
            self.texts.remove(&from);
        }
        self.texts.insert(start, Indexed { end, docs });
        true
    }

    /// Counts `query` (lowercased as the page compares) in the parts indexed
    /// and not held by the page, and says what is not indexed yet.
    pub(crate) fn find(&self, generation: u64, query: &str, held: &[(usize, usize)]) -> Option<Found> {
        if generation != self.generation {
            return None;
        }
        // What the page holds it finds itself: a block it holds is left out
        // here, wherever the shell's text of it came from.
        let holds = |at: usize| held.iter().any(|&(from, to)| from <= at && at < to);
        let mut ranges = Vec::new();
        if !query.is_empty() {
            for (&start, indexed) in &self.texts {
                let hits: Vec<(String, String, u32, usize)> = indexed
                    .docs
                    .iter()
                    .filter(|doc| !holds(doc.at))
                    .filter_map(|doc| {
                        let count = doc.text.matches(query).count() as u32;
                        (count > 0).then(|| (doc.block.clone(), doc.part.clone(), count, doc.at))
                    })
                    .collect();
                if !hits.is_empty() {
                    ranges.push(FoundRange { start, end: indexed.end, hits });
                }
            }
        }
        // What is covered: what the page holds and what is indexed.
        let mut covered: Vec<(usize, usize)> = held.to_vec();
        covered.extend(self.texts.iter().map(|(&start, indexed)| (start, indexed.end)));
        covered.sort_unstable();
        let mut unindexed = Vec::new();
        let mut at = 0;
        let len = self.events.len();
        for (start, end) in covered.into_iter().chain([(len, len)]) {
            while at < start.min(len) {
                let end = self.after(at, start.min(len), 1);
                unindexed.push((at, end));
                at = end;
            }
            at = at.max(end);
        }
        Some(Found { generation, ranges, unindexed })
    }
}

/// The names an event refers to that must stay in its part.
fn names(happened: &Happened) -> Vec<String> {
    let mut names = Vec::new();
    let mut turn = |turn: &str| {
        if !turn.is_empty() {
            names.push(format!("t:{turn}"));
        }
    };
    match happened {
        Happened::TurnStarted { turn: at }
        | Happened::TurnEnded { turn: at, .. }
        | Happened::Steps { turn: at, .. } => turn(at),
        Happened::Began { turn: at, item }
        | Happened::Finished { turn: at, item }
        | Happened::Updated { turn: at, item } => {
            turn(at);
            // Items in no turn the World saw are their own, together with
            // the draft they replace.
            if at.is_empty() {
                names.push(format!("i:{}", item.id));
                if let Some(draft) = &item.draft {
                    names.push(format!("i:{draft}"));
                }
            }
        }
        Happened::Grew { turn: at, item, .. } => {
            turn(at);
            if at.is_empty() {
                names.push(format!("i:{item}"));
            }
        }
        Happened::Heard { turn: at, delivery, message, .. } => {
            if let Some(at) = at {
                turn(at);
            }
            if let Some(delivery) = delivery {
                names.push(format!("d:{}", delivery.as_str()));
            }
            if let Some(message) = message {
                names.push(format!("m:{}", message.id));
            }
        }
        Happened::Changed { turn: at, change } => match change {
            SessionChange::MessageUsage { .. } | SessionChange::Usage { .. } => {
                if let Some(at) = at {
                    turn(at);
                }
            }
            SessionChange::Request { id, .. } | SessionChange::Resolved { id, .. } => {
                if let Some(at) = at {
                    turn(at);
                }
                names.push(format!("a:{id}"));
            }
            _ => {}
        },
        Happened::DeliveryUncertain { delivery, .. } => names.push(format!("d:{}", delivery.as_str())),
        _ => {}
    }
    names
}

fn json(event: &SessionEvent) -> Result<(String, u64), String> {
    let text = serde_json::to_string(event).map_err(|error| error.to_string())?;
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    Ok((text, hasher.finish()))
}

fn shared(shell: &crate::Shell, id: u64) -> Result<Arc<Mutex<Transcript>>, String> {
    shell.transcripts.get(id).ok_or_else(|| "the conversation is no longer watched".to_string())
}

/// A part of a watched conversation; none when the page asked about an
/// earlier reading or there is nothing there.
#[tauri::command]
pub(crate) fn conversation_part(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    edge: Edge,
) -> Result<Option<Part>, String> {
    Ok(shared(&shell, id)?.lock().expect("transcript").part(generation, edge))
}

#[tauri::command]
pub(crate) fn conversation_locate(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    turn: String,
) -> Result<Option<usize>, String> {
    Ok(shared(&shell, id)?.lock().expect("transcript").locate(generation, &turn))
}

#[tauri::command]
pub(crate) fn conversation_cut(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    events: usize,
) -> Result<Option<Cut>, String> {
    Ok(shared(&shell, id)?.lock().expect("transcript").cut(generation, events))
}

#[tauri::command]
pub(crate) fn conversation_index(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    start: usize,
    end: usize,
    docs: Vec<Doc>,
) -> Result<bool, String> {
    Ok(shared(&shell, id)?.lock().expect("transcript").index(generation, start, end, docs))
}

#[tauri::command]
pub(crate) fn conversation_find(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    query: String,
    held: Vec<(usize, usize)>,
) -> Result<Option<Found>, String> {
    Ok(shared(&shell, id)?.lock().expect("transcript").find(generation, &query, &held))
}
