//! Read-only access to Claude Code's versioned, internal transcript format.
//! Each read starts from the file, so a previous read cannot retain rows the
//! host subsequently removed. Turn attribution belongs to the caller.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use fastctx_types::session::{
    Does, Happened, HostAgentStart, Item, SessionChange, SessionEvent, ToolStatus, UserMessage, What,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{content, subagents, tools};

#[cfg(test)]
mod before;
pub mod follow;

/// What the host still stores, with unreadable records reported separately.
#[derive(Debug)]
pub struct Transcript {
    pub title: Option<String>,
    pub pending_cuts: bool,
    pub retained_tail: Option<String>,
    pub tail_uuid: Option<String>,
    pub context_uuids: Vec<String>,
    pub context_parent: Option<String>,
    pub context_tail: Option<String>,
    pub context_times: Vec<Timestamp>,
    pub context_messages: Vec<Value>,
    /// An idle native fork may close a trailing user summary without a model request.
    pub synthetic_tail: bool,
    pub events: Vec<SessionEvent>,
    /// What could not be made out, the first `PROBLEMS_KEPT` word for word.
    pub problems: Vec<String>,
    /// How many more there were past those, counted, not kept: a record of
    /// many rows it cannot make out holds a reading to no more than a few.
    pub problems_omitted: usize,
    /// Why the reading stopped before a line larger than World reads
    /// (`crate::bounded`), the rows before it read: what it shows is not the
    /// record whole, and nothing past that line may be judged gone by it.
    pub refused: Option<String>,
    /// Every record's UUID the file holds, shown or not: whether a record
    /// the session is known to have kept has reached the file yet. A
    /// message queued while a turn ran is kept in a row of its own, which
    /// names the message's UUID as its source: that UUID counts too.
    pub uuids: HashSet<String>,
    /// The agents the session started, as the files beside its record tell
    /// them.
    beside: subagents::Beside,
    /// The agents its Agent calls started, by their ids.
    agents: HashMap<String, HostAgentStart>,
}

/// The reply Claude Code adds as it loads a context that ends on the user's
/// side, as a fork's does: no model said it.
fn synthetic_reply(row: &Value) -> bool {
    row["type"] == "assistant"
        && row.pointer("/message/model").and_then(Value::as_str) == Some("<synthetic>")
        && row.pointer("/message/content") == Some(&json!([{"type":"text","text":"No response requested."}]))
}

pub fn same_projection(left: &Value, right: &Value) -> bool {
    super::context::same(left, right)
}

/// Whether a message projection is an assistant's without words, calls or
/// results. Claude Code leaves such messages out as it loads a transcript, as
/// one a stop cut off while the model was still thinking (2.1.303), so a
/// session resumed or forked from a file holds fewer messages than the file.
pub fn says_nothing(message: &Value) -> bool {
    message["role"] == "assistant"
        && message["text"].as_str().is_some_and(|text| text.trim().is_empty())
        && message["toolUses"].as_array().is_some_and(Vec::is_empty)
        && message["toolResults"].as_array().is_some_and(Vec::is_empty)
}

impl Transcript {
    /// The agents its Agent calls started, by their ids.
    pub fn agents(&self) -> &HashMap<String, HostAgentStart> {
        &self.agents
    }

    /// Confirms the mod's public tail before assigning native row UUIDs to it.
    pub fn message_tail_matches(&self, tail: &[Value]) -> bool {
        tail.len() <= self.context_messages.len()
            && tail
                .iter()
                .zip(self.context_messages.iter().skip(self.context_messages.len().saturating_sub(tail.len())))
                .all(|(expected, actual)| super::context::same(expected, actual))
    }

    /// Lines the context up with `held`, the message projections a session
    /// holds, oldest first, passing over the rows that say nothing and that
    /// the session does not hold. With `whole`, `held` is all the session's
    /// context; without it the file may hold only its latest messages, as it
    /// does after a compaction. On success the context keeps only the rows
    /// lined up; otherwise it is left as it was.
    pub fn line_up(&mut self, held: &[Value], whole: bool) -> bool {
        let mut kept = vec![false; self.context_messages.len()];
        let mut held = held.iter().rev().peekable();
        for (index, message) in self.context_messages.iter().enumerate().rev() {
            match held.peek() {
                Some(expected) if super::context::same(message, expected) => {
                    kept[index] = true;
                    held.next();
                }
                _ if says_nothing(message) => {}
                _ => return false,
            }
        }
        if whole && held.peek().is_some() {
            return false;
        }
        self.keep_context(&kept);
        true
    }

    /// Drops the context rows that say nothing and whose UUIDs `held` lacks.
    pub fn pass_over(&mut self, held: impl Fn(&str) -> bool) {
        let kept: Vec<bool> = self
            .context_uuids
            .iter()
            .zip(&self.context_messages)
            .map(|(uuid, message)| held(uuid) || !says_nothing(message))
            .collect();
        self.keep_context(&kept);
    }

    fn keep_context(&mut self, kept: &[bool]) {
        fn keep<T>(values: &mut Vec<T>, kept: &[bool]) {
            if values.len() == kept.len() {
                let mut index = 0;
                values.retain(|_| {
                    index += 1;
                    kept[index - 1]
                });
            }
        }
        keep(&mut self.context_uuids, kept);
        keep(&mut self.context_times, kept);
        keep(&mut self.context_messages, kept);
    }
}

/// A World context replacement, identified by observed message UUIDs and the
/// native compact boundary's logical parent. The host file is never edited.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryCut {
    pub until: String,
    pub parent: String,
    pub messages: usize,
    pub retained: Vec<Value>,
}

#[derive(Clone)]
pub(crate) struct Row {
    line: usize,
    /// Where its line starts in the file.
    start: u64,
    value: Value,
}

#[derive(Clone, Copy)]
struct Block {
    row: usize,
    index: usize,
}

#[derive(Clone, Default)]
struct Pair {
    calls: Vec<Block>,
    results: Vec<Block>,
}

impl Pair {
    fn readable(&self, rows: &dyn Rows) -> io::Result<bool> {
        if self.calls.len() != 1 || self.results.len() > 1 {
            return Ok(false);
        }
        let call = self.calls[0];
        let caller = rows.row(call.row)?;
        if !valid_call(&caller.blocks().expect("indexed blocks")[call.index]) {
            return Ok(false);
        }
        let Some(result) = self.results.first() else { return Ok(true) };
        let result_row = rows.row(result.row)?;
        if !valid_result(&result_row.blocks().expect("indexed blocks")[result.index]) {
            return Ok(false);
        }
        let Some(source) = result_row.value.get("sourceToolAssistantUUID").filter(|source| !source.is_null()) else {
            return Ok(true);
        };
        let Some(source) = source.as_str() else { return Ok(false) };
        let Some(source_index) = rows.index_of(source)? else { return Ok(true) };
        if source_index == call.row {
            return Ok(true);
        }
        // A response can be stored as several UUID-bearing rows. A source
        // UUID may name another block of that same API response.
        let response = caller.value.pointer("/message/id").and_then(Value::as_str).filter(|id| !id.is_empty());
        Ok(response.is_some() && response == rows.row(source_index)?.value.pointer("/message/id").and_then(Value::as_str))
    }
}

/// Reads the session's own record, and of what is beside it only what tells
/// of the agents it started (`subagents`): never their records, which are
/// read one at a time as the user opens one (`read_agent`).
///
/// JSONL records are read one line at a time, including a complete final JSON
/// value without a newline. An incomplete tail is left for a subsequent read. Duplicate
/// UUIDs use the last stored form at the original presentation position.
/// Missing times and malformed records produce problems, never invented times.
/// Unknown records without UUIDs are identified by their file line only.
/// Detected size or modification-time changes return `Interrupted`, without a retry.
pub fn read(path: &Path) -> io::Result<Transcript> {
    read_with_cuts(path, &[])
}

/// A record holding a line larger than World reads (`crate::bounded`) fails
/// here: what reads it needs it whole.
pub fn read_with_cuts(path: &Path, cuts: &[HistoryCut]) -> io::Result<Transcript> {
    read_rows(path, cuts, None, &HashSet::new(), false, Output::Context)
}

/// What a projection makes (design §28.51 ⑤). `Context`: all of it, the
/// context the session holds too. `History`: what a history shows — its
/// events handed to the sink one row at a time as they are made, none kept,
/// and no context messages, though what tells of the context (its problems,
/// tail, cuts) is made as ever. `SeamOnly`: no events at all, for a reading
/// that takes only what a seam keeps (titles, problems, agents).
pub enum Output<'a> {
    Context,
    History(&'a mut dyn FnMut(SessionEvent) -> io::Result<()>),
    SeamOnly,
}

impl Output<'_> {
    /// The events made so far handed on, or let go.
    fn hand(&mut self, events: &mut Vec<SessionEvent>) -> io::Result<()> {
        match self {
            Output::Context => Ok(()),
            Output::History(sink) => events.drain(..).try_for_each(sink),
            Output::SeamOnly => {
                events.clear();
                Ok(())
            }
        }
    }
}

/// As `read_continuing`, the events handed to `sink` as they are made rather
/// than kept (`Output::History`): a history read without its events held
/// whole, nor the context's messages.
pub fn read_history(
    path: &Path,
    cuts: &[HistoryCut],
    earlier: &HashSet<String>,
    sink: &mut dyn FnMut(SessionEvent) -> io::Result<()>,
) -> io::Result<Transcript> {
    read_rows(path, cuts, None, earlier, true, Output::History(sink))
}

/// Whether a UUID is one the sessions before a continuing one hold
/// (`read_continuing`'s `earlier`), asked one at a time.
pub type Earlier<'e> = &'e dyn Fn(&str) -> io::Result<bool>;

/// As `read_history`, the rows scanned kept in `store` rather than held in
/// memory (design §28.51 ⑤-3) and read back from it as the projection asks;
/// `earlier` asked of each UUID rather than held. The result's `uuids`
/// holds only the queued messages' sources it holds in memory: the rows'
/// own UUIDs, and the rest, are the store's (`RowStore::index_of`,
/// `RowStore::is_member` of `SET_QUEUED`).
pub fn read_history_kept(
    path: &Path,
    cuts: &[HistoryCut],
    earlier: Earlier<'_>,
    store: &mut dyn RowStore,
    sink: &mut dyn FnMut(SessionEvent) -> io::Result<()>,
) -> io::Result<Transcript> {
    read_rows_in(Kept::new(store), path, cuts, None, earlier, true, Output::History(sink))
}

/// Reads a session that continues its conversation (`background::continue_args`):
/// the records Claude Code copied there, which keep their ids, give no
/// events when one of `earlier`, the ids the conversation's sessions before
/// it hold, names them, as that session's own record shows them already.
/// They are still the session's context. A history shown: a line larger
/// than World reads (`crate::bounded`) ends it, the rows before it shown and
/// a problem saying why it goes no further.
pub fn read_continuing(path: &Path, cuts: &[HistoryCut], earlier: &HashSet<String>) -> io::Result<Transcript> {
    read_rows(path, cuts, None, earlier, true, Output::Context)
}

/// Reads the record of one agent the session started (`subagents::record`):
/// the agent's own rows, as a conversation of its own whose first words are
/// what it was asked.
pub fn read_agent(path: &Path, agent: &str) -> io::Result<Transcript> {
    read_rows(path, &[], Some(agent), &HashSet::new(), false, Output::Context)
}

/// How many problems a reading keeps word for word; past them it counts
/// how many more (`Transcript::problems_omitted`).
pub const PROBLEMS_KEPT: usize = 100;

/// `problems` as a note lists them: each kept, then how many more there
/// were past those (`PROBLEMS_KEPT`).
pub fn listed(problems: &[String], omitted: usize) -> String {
    let mut text = problems.join("; ");
    if omitted > 0 {
        text.push_str(&format!("; and {omitted} more"));
    }
    text
}

/// `problem` kept among `problems` while they are fewer than
/// `PROBLEMS_KEPT`, else counted in `omitted`: the first met are kept.
fn note_problem(problems: &mut Vec<String>, omitted: &mut usize, problem: String) {
    if problems.len() < PROBLEMS_KEPT {
        problems.push(problem);
    } else {
        *omitted += 1;
    }
}

/// What a reading of a record holds so far: each row once — a duplicate
/// UUID's in the place of the first — the queued messages' source UUIDs, the
/// titles, what could not be made out, and how far it read. The rows held in
/// memory (`Held`), or kept in a store (`Kept`).
pub(crate) struct Scanned<L = Held> {
    rows: L,
    queued: Members,
    titles: super::titles::Titles,
    problems: Vec<String>,
    problems_omitted: usize,
    /// The last line counted, from the file's start.
    number: usize,
    /// The bytes taken, through the last line read whole.
    consumed: u64,
    /// Why the reading stopped at a line larger than World reads
    /// (`crate::bounded`), the lines before it read as ever.
    refused: Option<String>,
}

impl Scanned {
    /// A reading that takes up after `number` lines and `consumed` bytes, with
    /// the titles set and the problems met before, its rows held in memory.
    pub(crate) fn new(number: usize, consumed: u64, titles: super::titles::Titles, problems: Vec<String>, omitted: usize) -> Self {
        Scanned::with(Held::default(), number, consumed, titles, problems, omitted)
    }

    /// The rows read, in their order.
    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows.rows
    }
}

impl<L> Scanned<L> {
    /// As `new`, the rows kept in `rows`.
    fn with(rows: L, number: usize, consumed: u64, titles: super::titles::Titles, mut problems: Vec<String>, omitted: usize) -> Self {
        // Those of a seam kept before a reading kept no more than a few:
        // the first kept, the rest counted.
        let past = problems.len().saturating_sub(PROBLEMS_KEPT);
        problems.truncate(PROBLEMS_KEPT);
        Self {
            rows,
            queued: Members::new(SET_QUEUED),
            titles,
            problems,
            problems_omitted: omitted + past,
            number,
            consumed,
            refused: None,
        }
    }

    /// Stops the reading before a line larger than World reads: the record
    /// is read up to it, and says why it goes no further.
    fn refuse(&mut self, why: io::Error) {
        let why = why.to_string();
        note_problem(&mut self.problems, &mut self.problems_omitted, why.clone());
        self.refused = Some(why);
    }

    /// Why the reading stopped before a line larger than World reads.
    pub(crate) fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    fn problem(&mut self, row: &Row, reason: &str) {
        note_problem(&mut self.problems, &mut self.problems_omitted, format!("line {}: {reason}", row.line));
    }

    /// The lines counted and the bytes taken so far.
    pub(crate) fn reached(&self) -> (usize, u64) {
        (self.number, self.consumed)
    }

    pub(crate) fn titles(&self) -> &super::titles::Titles {
        &self.titles
    }

}

/// Reads the rows `reader` gives into `scanned`. A last line without its
/// newline is taken only when `whole`, a file read at once, and then only a
/// complete value; otherwise it is left for a later reading.
pub(crate) fn scan<L: Keep>(
    reader: &mut impl BufRead,
    agent: Option<&str>,
    whole: bool,
    scanned: &mut Scanned<L>,
    mut taken: Option<&mut xxhash_rust::xxh3::Xxh3>,
) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        // A line past the bound is not read: the reading stops before it,
        // and says so (design §28.49). What needs the record whole fails
        // there (`read_rows`).
        let read = match crate::bounded::read_line(reader, &mut line, scanned.number + 1) {
            Ok(read) => read,
            Err(error) if crate::bounded::is_refused(&error) => {
                scanned.refuse(error);
                break;
            }
            Err(error) => return Err(error),
        };
        if read == 0 {
            break;
        }
        if !whole && line.last() != Some(&b'\n') {
            break;
        }
        scanned.number += 1;
        let number = scanned.number;
        if line.iter().all(u8::is_ascii_whitespace) {
            scanned.consumed += read as u64;
            if let Some(taken) = taken.as_deref_mut() {
                taken.update(&line);
            }
            continue;
        }
        if crate::bounded::too_many_values(&line) {
            scanned.number -= 1;
            scanned.refuse(crate::bounded::refused(number, &format!("holds more than {} values", crate::bounded::VALUES_MOST)));
            break;
        }
        let value: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(error) if line.last() != Some(&b'\n') && error.is_eof() => break,
            Err(error) => {
                note_problem(&mut scanned.problems, &mut scanned.problems_omitted, format!("line {number}: invalid JSON at column {}", error.column()));
                scanned.consumed += read as u64;
                if let Some(taken) = taken.as_deref_mut() {
                    taken.update(&line);
                }
                continue;
            }
        };
        let start = scanned.consumed;
        scanned.consumed += read as u64;
        if let Some(taken) = taken.as_deref_mut() {
            taken.update(&line);
        }
        let row = Row { line: number, start, value };
        if matches!(row.kind(), Some("custom-title" | "ai-title")) {
            if let Err(error) = scanned.titles.row(&row.value) {
                scanned.problem(&row, &error.to_string());
            }
            continue;
        }
        if !row.value.is_object() || row.kind().is_none() {
            scanned.problem(&row, "expected an object with a string type");
            continue;
        }
        if let Some(sidechain) = row.value.get("isSidechain")
            && !sidechain.is_boolean()
        {
            scanned.problem(&row, "isSidechain is not a boolean");
            continue;
        }
        if row.value.get("agentId").is_some_and(|id| !id.is_null() && !id.is_string()) {
            scanned.problem(&row, "agentId is not a string or null");
            continue;
        }
        let own = match agent {
            None => row.value["isSidechain"] != true && row.value.get("agentId").is_none_or(Value::is_null),
            Some(agent) => row.value.get("agentId").and_then(Value::as_str) == Some(agent),
        };
        if !own {
            continue;
        }
        if row.kind() == Some("progress")
            && matches!(
                row.value.pointer("/data/type").and_then(Value::as_str),
                Some("agent_progress" | "skill_progress")
            )
        {
            continue;
        }
        let kind = row.kind().expect("type checked");
        if bookkeeping(kind) {
            continue;
        }
        // Claude Code adds bookkeeping rows of new kinds with most releases. A
        // row outside the conversation's tree, with no UUID, of a kind this
        // reader does not take is one of them: it is left out, not reported as
        // a damaged record that would leave the whole history unread.
        if row.value.get("uuid").is_none()
            && !matches!(kind, "user" | "assistant" | "attachment" | "system" | "queue-operation")
        {
            continue;
        }
        // A message queued while a turn ran (a background task's notice, words
        // sent meanwhile) reaches the file as an attachment of its own UUID
        // that names the message's as its source (2.1.303).
        if row.kind() == Some("attachment")
            && row.value.pointer("/attachment/type").and_then(Value::as_str) == Some("queued_command")
            && let Some(source) = row.value.pointer("/attachment/source_uuid").and_then(Value::as_str)
            && !source.is_empty()
        {
            scanned.queued.insert(source.to_string(), scanned.rows.blocks_store())?;
        }
        let uuid = match row.value.get("uuid") {
            None => None,
            Some(uuid) => match uuid.as_str().filter(|uuid| !uuid.is_empty()) {
                Some(uuid) => Some(uuid.to_owned()),
                None => {
                    scanned.problem(&row, "uuid is not a nonempty string");
                    continue;
                }
            },
        };
        scanned.rows.keep(uuid, row, &line)?;
    }
    Ok(())
}

/// Reads the record at `path` whole; `partial`: up to a line larger than
/// World reads, else such a line fails the reading.
fn read_rows(
    path: &Path,
    cuts: &[HistoryCut],
    agent: Option<&str>,
    earlier: &HashSet<String>,
    partial: bool,
    output: Output<'_>,
) -> io::Result<Transcript> {
    read_rows_in(Held::default(), path, cuts, agent, &|uuid| Ok(earlier.contains(uuid)), partial, output)
}

/// As `read_rows`, the rows scanned kept in `rows`.
fn read_rows_in<L: Keep>(
    rows: L,
    path: &Path,
    cuts: &[HistoryCut],
    agent: Option<&str>,
    earlier: Earlier<'_>,
    partial: bool,
    output: Output<'_>,
) -> io::Result<Transcript> {
    let file = File::open(path)?;
    let before = file.metadata()?;
    let mut reader = BufReader::new(file);
    let mut scanned = Scanned::with(rows, 0, 0, super::titles::Titles::default(), Vec::new(), 0);
    scan(&mut reader, agent, true, &mut scanned, None)?;
    scanned.rows.scanned()?;
    if !partial && let Some(why) = scanned.refused() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, why.to_string()));
    }
    // Claude Code can truncate and rewrite in place (verified 2026-10-04).
    // Reject a mixed read instead of presenting it as a coherent transcript.
    let after = reader.get_ref().metadata()?;
    let named = path.metadata()?;
    if before.len() != after.len()
        || before.modified()? != after.modified()?
        || after.len() != named.len()
        || after.modified()? != named.modified()?
    {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Claude transcript changed while it was being read"));
    }
    let beside = if agent.is_none() { subagents::beside(path) } else { subagents::Beside::default() };
    project_asking(scanned, cuts, earlier, beside, HashMap::new(), output)
}

/// The logical rows a projection reads (design §28.51 ⑤-3): each row once,
/// a duplicate UUID's in the place of the first in its last form; taken in
/// their order, by their number, or by the UUID they bear. Held in memory,
/// or kept on disk and read back as asked, a row at a time.
pub(crate) trait Rows {
    /// How many rows there are.
    fn count(&self) -> usize;
    /// The row numbered `index`.
    fn row(&self, index: usize) -> io::Result<RowRef<'_>>;
    /// The row bearing `uuid`.
    fn index_of(&self, uuid: &str) -> io::Result<Option<usize>>;
    /// Each row in its order, with its number.
    fn each(&self, visit: &mut dyn FnMut(usize, &Row) -> io::Result<()>) -> io::Result<()>;
    /// Every UUID the rows bear, where they are held to be given whole: rows
    /// kept in a store give none, the store answering for them
    /// (`RowStore::index_of`).
    fn uuids(&self) -> io::Result<Vec<String>>;
    /// The store the rows are kept in, where the tool blocks a projection
    /// pairs are noted too (`Pairs`); none where the rows are held.
    fn blocks_store(&self) -> Option<&dyn RowStore> {
        None
    }
}

/// How many keys a set of a reading holds in memory (`Members`), and how
/// many bytes of them, before all of it goes to the store its rows are kept
/// in, where they are.
const MEMBERS_HELD: usize = if cfg!(test) { 64 } else { 1 << 16 };
const MEMBERS_BYTES: usize = if cfg!(test) { 4 * 1024 } else { 4 * 1024 * 1024 };

/// The sets a reading notes in a store (`RowStore::note_member`): the UUIDs
/// its rows take back, and the queued messages' sources.
pub const SET_REMOVED: u8 = 0;
pub const SET_QUEUED: u8 = 1;

/// A set of keys a reading holds: in memory; or, past `MEMBERS_HELD` of
/// them where the rows are kept in a store, all noted there and asked of it
/// from then on.
pub(crate) struct Members {
    held: HashSet<String>,
    /// About what `held` takes: its keys' bytes, and the place of each.
    held_bytes: usize,
    set: u8,
    noted: bool,
}

impl Members {
    fn new(set: u8) -> Self {
        Members { held: HashSet::new(), held_bytes: 0, set, noted: false }
    }

    fn insert(&mut self, key: String, store: Option<&dyn RowStore>) -> io::Result<()> {
        match store {
            Some(store) if self.noted => store.note_member(self.set, &key),
            _ => {
                let bytes = key.len() + std::mem::size_of::<String>() + 8;
                if self.held.insert(key) {
                    self.held_bytes += bytes;
                }
                if let Some(store) = store
                    && (self.held.len() > MEMBERS_HELD || self.held_bytes > MEMBERS_BYTES)
                {
                    for key in self.held.drain() {
                        store.note_member(self.set, &key)?;
                    }
                    self.held_bytes = 0;
                    self.noted = true;
                }
                Ok(())
            }
        }
    }

    fn contains(&self, key: &str, store: Option<&dyn RowStore>) -> io::Result<bool> {
        match store {
            Some(store) if self.noted => store.is_member(self.set, key),
            _ => Ok(self.held.contains(key)),
        }
    }
}

/// The tool blocks a projection pairs, by the tool use's id (`Pair`): held
/// in memory beside rows held there, or noted in the store rows are kept
/// in. A pair is there once a block of its id is noted, its calls and its
/// results each in the order noted — from a store, the first two of each
/// (`RowStore::blocks`), which judge it as all of them do.
enum Pairs<'s> {
    Held(HashMap<String, Pair>),
    Stored(&'s dyn RowStore),
}

impl<'s> Pairs<'s> {
    fn of(store: Option<&'s dyn RowStore>) -> Self {
        match store {
            Some(store) => Pairs::Stored(store),
            None => Pairs::Held(HashMap::new()),
        }
    }

    /// Notes block `block`, a call or (`result`) a result of the tool use `id`.
    fn note(&mut self, id: &str, result: bool, block: Block) -> io::Result<()> {
        match self {
            Pairs::Held(pairs) => {
                let pair = pairs.entry(id.to_owned()).or_default();
                if result { pair.results.push(block) } else { pair.calls.push(block) }
                Ok(())
            }
            Pairs::Stored(store) => store.note_block(id, result, block.row, block.index),
        }
    }

    /// The pair of the tool use `id`, where a block of it was noted.
    fn get(&self, id: &str) -> io::Result<Option<std::borrow::Cow<'_, Pair>>> {
        match self {
            Pairs::Held(pairs) => Ok(pairs.get(id).map(std::borrow::Cow::Borrowed)),
            Pairs::Stored(store) => {
                let blocks = store.blocks(id)?;
                if blocks.is_empty() {
                    return Ok(None);
                }
                let mut pair = Pair::default();
                for (result, row, index) in blocks {
                    let block = Block { row, index };
                    if result { pair.results.push(block) } else { pair.calls.push(block) }
                }
                Ok(Some(std::borrow::Cow::Owned(pair)))
            }
        }
    }
}

/// Where a scan puts the rows it takes: a row bearing a UUID already taken
/// in the place of that one, else after the others; `line` as the file
/// holds it.
pub(crate) trait Keep: Rows {
    fn keep(&mut self, uuid: Option<String>, row: Row, line: &[u8]) -> io::Result<()>;
    /// The scan is done: the rows are read back from here on.
    fn scanned(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Rows held in memory, as a reading scans them (`Scanned`).
#[derive(Default)]
pub(crate) struct Held {
    rows: Vec<Row>,
    by_uuid: HashMap<String, usize>,
}

impl Keep for Held {
    fn keep(&mut self, uuid: Option<String>, row: Row, _line: &[u8]) -> io::Result<()> {
        if let Some(uuid) = uuid {
            if let Some(&index) = self.by_uuid.get(&uuid) {
                self.rows[index] = row;
                return Ok(());
            }
            self.by_uuid.insert(uuid, self.rows.len());
        }
        self.rows.push(row);
        Ok(())
    }
}

/// Where a whole reading keeps the rows it scans rather than hold them in
/// memory (design §28.51 ⑤-3): each row's line as the file holds it, the
/// line's number and where it starts, and the UUID the row bears, one row
/// to a UUID; read back in their order, by number, or by UUID. The node
/// keeps them in a reading's work store. What reads them back parses each
/// line again as the scan did, so it reads the values the scan read.
pub trait RowStore {
    /// How many rows are kept.
    fn count(&self) -> usize;
    /// Keeps a row after the others, bearing `uuid` when it bears one (no
    /// row kept bears it yet).
    fn push(&mut self, uuid: Option<&str>, line: usize, start: u64, bytes: &[u8]) -> io::Result<()>;
    /// Puts a later form of row `index` (the same UUID) in its place.
    fn replace(&mut self, index: usize, line: usize, start: u64, bytes: &[u8]) -> io::Result<()>;
    /// The scan is done: what it kept is written before any row is read
    /// back in order or by number (`each`, `row`). Until then the store is
    /// asked only `index_of`, which answers for the rows not written yet too.
    fn flush(&mut self) -> io::Result<()>;
    /// Notes a tool block a projection pairs: block `index` of row `row`, a
    /// call or (`result`) a result of the tool use `id`. Noted as the rows
    /// are read in order (`each`), each block once, while nothing is asked
    /// of them (`blocks`) yet.
    fn note_block(&self, id: &str, result: bool, row: usize, index: usize) -> io::Result<()>;
    /// The first blocks noted of the tool use `id`: at most two calls and
    /// two results, each in the order noted — all a pair is judged by
    /// (`Pair::readable`: one call, one result at most, which they are),
    /// however many share the id. Each whether it is a result, its row and
    /// its place in the row.
    fn blocks(&self, id: &str) -> io::Result<Vec<(bool, usize, usize)>>;
    /// Notes `key` in the set numbered `set` (`SET_REMOVED`, `SET_QUEUED`):
    /// what a reading holds too many of to keep in memory (`Members`).
    fn note_member(&self, set: u8, key: &str) -> io::Result<()>;
    /// Whether `key` was noted in the set numbered `set`.
    fn is_member(&self, set: u8, key: &str) -> io::Result<bool>;
    /// The row bearing `uuid`.
    fn index_of(&self, uuid: &str) -> io::Result<Option<usize>>;
    /// Row `index`: its line's number, where it starts, the line.
    fn row(&self, index: usize) -> io::Result<(usize, u64, Vec<u8>)>;
    /// Each row in its order: its number, its line's number, where it
    /// starts, the line. `visit` may read the store again (`row`,
    /// `index_of`) while it runs.
    fn each(&self, visit: VisitLine<'_>) -> io::Result<()>;
    /// The UUID of each row that starts before `to`, in their order — a row
    /// a later one of its UUID took the place of starts where that one does
    /// — handed to `each` (`follow::tail_kept`: the rows a seam moved past).
    fn ids_before(&self, to: u64, each: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        let _ = (to, each);
        Err(io::Error::new(io::ErrorKind::Unsupported, "this store gives no rows by where they start"))
    }
    /// Lets go every row, tool block and key it keeps: a reading of other
    /// rows begins (`follow::tail_kept`).
    fn clear(&mut self) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "this store is not cleared"))
    }
}

/// What looks at each row a store gives (`RowStore::each`): its number, its
/// line's number, where it starts, the line.
pub type VisitLine<'v> = &'v mut dyn FnMut(usize, usize, u64, &[u8]) -> io::Result<()>;

/// Rows kept in a store (`RowStore`), each parsed as it is read.
pub(crate) struct Kept<'s> {
    store: &'s mut dyn RowStore,
    /// The row read in order now (`each`), however long: a row of many
    /// tool calls is read again for each as it is paired.
    current: RefCell<Option<(usize, std::rc::Rc<Row>)>>,
    /// The rows parsed last, the latest first, each with what it takes
    /// parsed (`parsed_bytes`), at most `RECENT` of them and `RECENT_BYTES`
    /// in all: a tool's result is read a few times over as the projection
    /// pairs it, close by.
    recent: RefCell<VecDeque<(usize, std::rc::Rc<Row>, usize)>>,
}

/// How many rows a store's reading keeps parsed beside the one it reads in
/// order (`Kept::recent`), and how many bytes they may take parsed at most
/// (`parsed_bytes`): a few rows, never much.
const RECENT: usize = 8;
const RECENT_BYTES: usize = 2 * 1024 * 1024;

/// About what the line `bytes` takes parsed, no less: its bytes, a map's
/// node with room for eleven fields for each `{` (`serde_json::Map` is a
/// B-tree), and a value's place for each `,` or `[`.
fn parsed_bytes(bytes: &[u8]) -> usize {
    const NODE: usize = 16 + 11 * (std::mem::size_of::<String>() + std::mem::size_of::<Value>());
    let (maps, values) = bytes.iter().fold((0usize, 0usize), |(maps, values), byte| match byte {
        b'{' => (maps + 1, values + 1),
        b',' | b'[' => (maps, values + 1),
        _ => (maps, values),
    });
    bytes.len() + maps * NODE + values * std::mem::size_of::<Value>()
}

/// A row read through the row source (`Rows::row`): borrowed where the rows
/// are held, shared where a store's row was parsed.
pub(crate) enum RowRef<'a> {
    Held(&'a Row),
    Parsed(std::rc::Rc<Row>),
}

impl std::ops::Deref for RowRef<'_> {
    type Target = Row;
    fn deref(&self) -> &Row {
        match self {
            RowRef::Held(row) => row,
            RowRef::Parsed(row) => row,
        }
    }
}

impl<'s> Kept<'s> {
    fn new(store: &'s mut dyn RowStore) -> Self {
        Kept { store, current: RefCell::new(None), recent: RefCell::new(VecDeque::with_capacity(RECENT)) }
    }

    /// Row `index` as it was parsed lately, if it was.
    fn lately(&self, index: usize) -> Option<std::rc::Rc<Row>> {
        if let Some((current, row)) = &*self.current.borrow()
            && *current == index
        {
            return Some(row.clone());
        }
        self.recent.borrow().iter().find(|(kept, ..)| *kept == index).map(|(_, row, _)| row.clone())
    }

    /// Row `index` parsed of its line, or as it was parsed lately; kept
    /// among the recent where its line leaves room.
    fn parsed(&self, index: usize, line: usize, start: u64, bytes: &[u8]) -> io::Result<std::rc::Rc<Row>> {
        if let Some(row) = self.lately(index) {
            return Ok(row);
        }
        let row = std::rc::Rc::new(parsed(line, start, bytes)?);
        let held = parsed_bytes(bytes);
        if held <= RECENT_BYTES {
            let mut recent = self.recent.borrow_mut();
            recent.push_front((index, row.clone(), held));
            while recent.len() > RECENT || recent.iter().map(|(.., length)| length).sum::<usize>() > RECENT_BYTES {
                recent.pop_back();
            }
        }
        Ok(row)
    }
}

/// A row's line as the scan parsed it, parsed again.
fn parsed(line: usize, start: u64, bytes: &[u8]) -> io::Result<Row> {
    let value = serde_json::from_slice(bytes).map_err(|error| io::Error::other(format!("a kept row did not read again: {error}")))?;
    Ok(Row { line, start, value })
}

impl Rows for Kept<'_> {
    fn count(&self) -> usize {
        self.store.count()
    }
    fn row(&self, index: usize) -> io::Result<RowRef<'_>> {
        if let Some(row) = self.lately(index) {
            return Ok(RowRef::Parsed(row));
        }
        let (line, start, bytes) = self.store.row(index)?;
        self.parsed(index, line, start, &bytes).map(RowRef::Parsed)
    }
    fn index_of(&self, uuid: &str) -> io::Result<Option<usize>> {
        self.store.index_of(uuid)
    }
    fn each(&self, visit: &mut dyn FnMut(usize, &Row) -> io::Result<()>) -> io::Result<()> {
        let visited = self.store.each(&mut |index, line, start, bytes| {
            let row = self.parsed(index, line, start, bytes)?;
            *self.current.borrow_mut() = Some((index, row.clone()));
            visit(index, &row)
        });
        *self.current.borrow_mut() = None;
        visited
    }
    fn uuids(&self) -> io::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn blocks_store(&self) -> Option<&dyn RowStore> {
        Some(&*self.store)
    }
}

impl Keep for Kept<'_> {
    fn keep(&mut self, uuid: Option<String>, row: Row, line: &[u8]) -> io::Result<()> {
        // A row put again is parsed again.
        *self.current.get_mut() = None;
        self.recent.get_mut().clear();
        if let Some(uuid) = &uuid
            && let Some(index) = self.store.index_of(uuid)?
        {
            return self.store.replace(index, row.line, row.start, line);
        }
        self.store.push(uuid.as_deref(), row.line, row.start, line)
    }
    fn scanned(&mut self) -> io::Result<()> {
        self.store.flush()
    }
}

impl Rows for Held {
    fn count(&self) -> usize {
        self.rows.len()
    }
    fn row(&self, index: usize) -> io::Result<RowRef<'_>> {
        Ok(RowRef::Held(&self.rows[index]))
    }
    fn index_of(&self, uuid: &str) -> io::Result<Option<usize>> {
        Ok(self.by_uuid.get(uuid).copied())
    }
    fn each(&self, visit: &mut dyn FnMut(usize, &Row) -> io::Result<()>) -> io::Result<()> {
        self.rows.iter().enumerate().try_for_each(|(index, row)| visit(index, row))
    }
    fn uuids(&self) -> io::Result<Vec<String>> {
        Ok(self.by_uuid.keys().cloned().collect())
    }
}

/// What a scan leaves beside its rows: the queued messages' sources, the
/// titles, what could not be made out, and why it stopped short.
pub(crate) struct Scan {
    pub(crate) queued: Members,
    pub(crate) titles: super::titles::Titles,
    pub(crate) problems: Vec<String>,
    pub(crate) problems_omitted: usize,
    pub(crate) refused: Option<String>,
}

/// When `row` says it was written, where it says so readably.
fn time(row: &Row) -> Option<Timestamp> {
    row.value.get("timestamp").and_then(Value::as_str).and_then(|text| text.parse::<Timestamp>().ok())
}

/// What the rows `scanned` holds show, the agents named in `agents` started
/// before them: their events, title, context and problems, as much of it as
/// `output` asks for (`project_rows`).
pub(crate) fn project<L: Rows>(
    scanned: Scanned<L>,
    cuts: &[HistoryCut],
    earlier: &HashSet<String>,
    beside: subagents::Beside,
    agents: HashMap<String, HostAgentStart>,
    output: Output<'_>,
) -> io::Result<Transcript> {
    project_asking(scanned, cuts, &|uuid| Ok(earlier.contains(uuid)), beside, agents, output)
}

/// As `project`, `earlier` asked of each UUID.
fn project_asking<L: Rows>(
    scanned: Scanned<L>,
    cuts: &[HistoryCut],
    earlier: Earlier<'_>,
    beside: subagents::Beside,
    agents: HashMap<String, HostAgentStart>,
    output: Output<'_>,
) -> io::Result<Transcript> {
    let Scanned { rows, queued, titles, problems, problems_omitted, refused, .. } = scanned;
    project_rows(&rows, Scan { queued, titles, problems, problems_omitted, refused }, cuts, earlier, beside, agents, output)
}

/// What `rows` show, read through the row source: in its order a few times
/// over, and the rows a judgement needs by number or UUID. The judgements are
/// those a projection of rows held whole makes, of the same values.
#[allow(clippy::too_many_arguments)]
pub(crate) fn project_rows(
    rows: &dyn Rows,
    scanned: Scan,
    cuts: &[HistoryCut],
    earlier: Earlier<'_>,
    beside: subagents::Beside,
    agents: HashMap<String, HostAgentStart>,
    mut output: Output<'_>,
) -> io::Result<Transcript> {
    let Scan { queued, titles, problems, problems_omitted, refused } = scanned;
    let mut transcript = Transcript {
        refused,
        title: None,
        pending_cuts: false,
        retained_tail: None,
        tail_uuid: None,
        context_uuids: Vec::new(),
        context_parent: None,
        context_tail: None,
        context_times: Vec::new(),
        context_messages: Vec::new(),
        synthetic_tail: false,
        events: Vec::new(),
        problems,
        problems_omitted,
        uuids: HashSet::new(),
        beside,
        agents,
    };
    // The queued messages' sources a store holds (past what a reading keeps
    // in memory) are the store's to answer for, as the rows' UUIDs are.
    transcript.uuids = rows.uuids()?.into_iter().chain(queued.held).collect();
    let store = rows.blocks_store();
    transcript.title = titles.title();
    // One reading of the rows in order: the last words or answer, the last
    // compaction that replaced the context, the rows a context is made of,
    // what retractions take away, what compactions name as their anchors,
    // and the problems of the rows' times and retractions — told after those
    // the context and the cuts meet, as a reading in that order tells them.
    let (mut last_message, mut last_replacement) = (None, None);
    // The rows of the kinds a context is made of, by number: four bytes
    // each (a reading counts no more rows than four bytes number).
    if u32::try_from(rows.count()).is_err() {
        return Err(io::Error::other("a record of more rows than a reading counts"));
    }
    let mut contextual: Vec<u32> = Vec::new();
    let mut removed = Members::new(SET_REMOVED);
    // Claude Code names the summary a compaction begins with as the anchor
    // of what it kept (`compactMetadata`).
    let mut anchors: HashMap<String, usize> = HashMap::new();
    // As many as a reading keeps word for word, the rest counted.
    let (mut timing_problems, mut timing_omitted) = (Vec::new(), 0usize);
    // The compactions that replaced the context, each with the parent it
    // names, where the cuts look for theirs: one for each compaction.
    let mut replacements: Vec<(usize, Option<String>)> = Vec::new();
    rows.each(&mut |index, row| {
        if matches!(row.kind(), Some("user" | "assistant")) {
            last_message = Some(index);
        }
        if row.replacement() {
            last_replacement = Some(index);
            replacements.push((index, row.value["logicalParentUuid"].as_str().map(str::to_owned)));
        }
        if matches!(row.value["type"].as_str(), Some("user" | "assistant" | "attachment" | "system")) {
            contextual.push(index as u32);
        }
        let at = time(row);
        if at.is_none() {
            note_problem(&mut timing_problems, &mut timing_omitted, format!("line {}: missing or invalid timestamp", row.line));
        }
        let references = if at.is_none() || row.message_problem().is_some() {
            None
        } else if row.kind() == Some("assistant") {
            row.value.get("supersedesUuids")
        } else if row.kind() == Some("system") && row.value["subtype"] == "model_refusal_fallback" {
            row.value.get("retractedMessageUuids")
        } else {
            None
        };
        if let Some(references) = references {
            if let Some(ids) = references.as_array().filter(|ids| ids.iter().all(Value::is_string)) {
                for id in ids.iter().filter_map(Value::as_str) {
                    removed.insert(id.to_owned(), store)?;
                }
            } else {
                note_problem(&mut timing_problems, &mut timing_omitted, format!("line {}: invalid retracted message UUID list", row.line));
            }
        }
        if row.compaction() {
            let metadata = &row.value["compactMetadata"];
            for uuid in [&metadata["preservedSegment"]["anchorUuid"], &metadata["preservedMessages"]["anchorUuid"]].into_iter().filter_map(Value::as_str) {
                anchors.insert(uuid.to_owned(), index);
            }
        }
        Ok(())
    })?;
    transcript.tail_uuid = match last_message {
        Some(index) => rows.row(index)?.value["uuid"].as_str().map(str::to_owned),
        None => None,
    };
    transcript.context_parent = match last_replacement {
        Some(index) => rows.row(index)?.value["logicalParentUuid"].as_str().map(str::to_owned),
        None => None,
    };
    // A compaction whose kept messages are missing from the file leaves the
    // context it holds unknown, not the conversation unreadable: what it
    // shows is read as ever, and what needs that context refuses on the
    // problem.
    // A row the source fails to give fails the reading; it is no problem of
    // the record's.
    let context_order = match super::context::order(rows, contextual, last_replacement)? {
        Ok(order) => order,
        Err(why) => {
            transcript.note(format!("the context its last compaction left: {why}"));
            Vec::new()
        }
    };
    for &index in context_order.iter().rev() {
        if let Some(uuid) = rows.row(index as usize)?.value["uuid"].as_str() {
            transcript.context_tail = Some(uuid.to_owned());
            break;
        }
    }
    for &index in context_order.iter().rev() {
        let row = rows.row(index as usize)?;
        if matches!(row.kind(), Some("user" | "assistant")) {
            transcript.synthetic_tail = synthetic_reply(&row.value);
            break;
        }
    }

    // The rows the cuts took back, as ranges of their numbers.
    let mut cut_rows: Vec<std::ops::Range<usize>> = Vec::new();
    for cut in cuts {
        if cut.retained.len() != cut.messages {
            return Err(io::Error::other("the World rewind omitted its retained message projections"));
        }
        let Some(until) = rows.index_of(&cut.until)? else {
            transcript.note("the retained World turn's transcript anchor is absent".into());
            continue;
        };
        let boundaries: Vec<usize> = replacements
            .iter()
            .filter(|(_, parent)| parent.as_deref() == Some(cut.parent.as_str()))
            .map(|(index, _)| *index)
            .collect();
        if boundaries.is_empty() {
            transcript.pending_cuts = true;
            continue;
        }
        if boundaries.len() != 1 {
            return Err(io::Error::other("the World rewind has an ambiguous compact boundary"));
        }
        let boundary = boundaries[0];
        if boundary <= until {
            return Err(io::Error::other("the World rewind boundary precedes its retained anchor"));
        }
        let mut copied = Vec::new();
        for index in boundary + 1..rows.count() {
            if copied.len() == cut.messages {
                break;
            }
            let row = rows.row(index)?;
            if row.replacement() {
                break;
            }
            if super::context::projected(&row.value) {
                copied.push(index);
            }
        }
        if copied.len() != cut.messages {
            transcript.pending_cuts = true;
            continue;
        }
        for (&index, expected) in copied.iter().zip(&cut.retained) {
            if !super::context::same(&super::context::projection(&rows.row(index)?.value), expected) {
                return Err(io::Error::other("Claude persisted a different replacement prefix than its mod returned"));
            }
        }
        let tail = copied.last().copied().unwrap_or(boundary);
        transcript.retained_tail = rows.row(tail)?.value["uuid"].as_str().map(str::to_owned);
        cut_rows.push(until + 1..boundary);
        cut_rows.push(boundary + 1..tail + 1);
    }
    for problem in timing_problems {
        transcript.note(problem);
    }
    transcript.problems_omitted += timing_omitted;

    let taken_back = |id: Option<&str>| -> io::Result<bool> {
        match id {
            Some(id) => removed.contains(id, store),
            None => Ok(false),
        }
    };
    let native_visible = |row: &Row| -> io::Result<bool> {
        Ok(!taken_back(row.value.get("uuid").and_then(Value::as_str))?
            && !taken_back(row.value.get("sourceToolAssistantUUID").and_then(Value::as_str))?)
    };
    // The context's messages only where they are asked for: a history and a
    // seam use none of them.
    if matches!(output, Output::Context) {
        for &index in &context_order {
            let row = rows.row(index as usize)?;
            if !native_visible(&row)? || !super::context::projected(&row.value) {
                continue;
            }
            if let Some(uuid) = row.value["uuid"].as_str() {
                transcript.context_uuids.push(uuid.to_owned());
            }
            if let Some(at) = row.value["timestamp"].as_str().and_then(|text| text.parse().ok()) {
                transcript.context_times.push(at);
            }
            transcript.context_messages.push(super::context::projection(&row.value));
        }
    }
    // A row bearing a UUID is the row that UUID names: one a cut took back
    // is not shown.
    let visible = |index: usize, row: &Row| -> io::Result<bool> {
        Ok(native_visible(row)? && !(row.value.get("uuid").is_some() && cut_rows.iter().any(|cut| cut.contains(&index))))
    };
    let mut pairs = Pairs::of(rows.blocks_store());
    let mut summaries: HashMap<String, Vec<usize>> = HashMap::new();
    rows.each(&mut |row_index, row| {
        if time(row).is_none() || !visible(row_index, row)? || row.message_problem().is_some() {
            return Ok(());
        }
        if row.kind() == Some("user")
            && row.value["isCompactSummary"] == true
            && let Some(boundary) = summarized(rows, &anchors, row)?
        {
            let above = rows.row(boundary)?;
            if time(&above).is_some()
                && visible(boundary, &above)?
                && let Some(uuid) = above.value.get("uuid").and_then(Value::as_str)
            {
                summaries.entry(uuid.to_owned()).or_default().push(row_index);
            }
        }
        for (index, block) in row.blocks().into_iter().flatten().enumerate() {
            let reference = Block { row: row_index, index };
            match (row.kind(), block.get("type").and_then(Value::as_str)) {
                (Some("assistant"), Some("tool_use")) => {
                    if let Some(id) = block.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                        pairs.note(id, false, reference)?;
                    }
                }
                (Some("user"), Some("tool_result")) => {
                    if let Some(id) = block.get("tool_use_id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                        pairs.note(id, true, reference)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })?;
    let attached: HashSet<usize> = summaries.values().flatten().copied().collect();
    rows.each(&mut |index, row| {
        // What the rows before made, handed on (`Output`).
        output.hand(&mut transcript.events)?;
        let Some(at) = time(row) else { return Ok(()) };
        if !visible(index, row)? || attached.contains(&index) {
            return Ok(());
        }
        if let Some(uuid) = row.value.get("uuid").and_then(Value::as_str)
            && earlier(uuid)?
        {
            return Ok(());
        }
        if let Some(problem) = row.message_problem() {
            transcript.problem(row, problem);
            transcript.other(row, at, 0, "record", row.value.clone());
            return Ok(());
        }
        if synthetic_reply(&row.value) {
            transcript.hidden(row, at);
            return Ok(());
        }
        if row.kind() == Some("assistant")
            && let Some(model) = row.value["message"]["model"].as_str()
        {
            let items = row
                .blocks()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, block)| {
                    if block["type"] == "tool_use" {
                        block["id"].as_str().map(str::to_owned).unwrap_or_else(|| row.id(index))
                    } else {
                        row.id(index)
                    }
                })
                .collect();
            transcript.events.push(SessionEvent {
                at,
                happened: Happened::Changed {
                    turn: None,
                    change: SessionChange::MessageUsage {
                        items,
                        response: row.value["message"]["id"].as_str().map(str::to_owned),
                        model: model.to_owned(),
                        api_error: row.value["isApiErrorMessage"] == true,
                        usage: row.value["message"]["usage"].clone(),
                    },
                },
            });
        }
        if row.compaction() || row.kind() == Some("user") && row.value["isCompactSummary"] == true {
            let summary = row.value.get("uuid").and_then(Value::as_str).and_then(|id| summaries.get(id));
            let detail = match summary {
                Some(summary) => {
                    let parts = summary.iter().map(|&index| rows.row(index)).collect::<io::Result<Vec<_>>>()?;
                    json!({ "boundary": row.value, "summaries": parts.iter().map(|part| &part.value).collect::<Vec<_>>() })
                }
                None => row.value.clone(),
            };
            transcript.finish(at, row.id(0), at, What::Compaction, detail);
            return Ok(());
        }
        match row.kind() {
            Some("assistant" | "user") => {
                let message = &row.value["message"];
                let prompt = row.kind() == Some("user") && row.prompt();
                if row.kind() == Some("user")
                    && !prompt
                    && !row.blocks().is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
                {
                    transcript.hidden(row, at);
                    return Ok(());
                }
                if prompt && transcript.agent_told(row, at) {
                    return Ok(());
                }
                if prompt {
                    let words = match message.get("content") {
                        Some(Value::String(text)) => Some(text.clone()),
                        Some(Value::Array(blocks))
                            if blocks.iter().any(|block| {
                                block["type"] == "image"
                                    || block["type"] == "text" && block.get("text").is_some_and(Value::is_string)
                            }) =>
                        {
                            Some(
                                blocks
                                    .iter()
                                    .filter(|block| block["type"] == "text")
                                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            )
                        }
                        _ => None,
                    };
                    // The user's words the World's plugin submitted, as they were said.
                    let words = match words {
                        Some(text) if row.value["origin"]["kind"] == "plugin" => Some(super::as_said(&text).to_owned()),
                        words => words,
                    };
                    if let Some(text) = words {
                        transcript.events.push(SessionEvent {
                            at,
                            happened: Happened::Heard {
                                delivery: None,
                                turn: None,
                                from: None,
                                text,
                                message: Some(Box::new(row.user_message())),
                                notice: None,
                            },
                        });
                    }
                }
                match message.get("content") {
                    Some(Value::String(_)) if prompt => {}
                    Some(Value::Array(blocks)) => {
                        // How long the model thought, as Claude Code measured
                        // it: one figure for the row, so only a row with one
                        // block of thinking can give it to that block.
                        let thinking = |block: &Value| {
                            matches!(block.get("type").and_then(Value::as_str), Some("thinking" | "redacted_thinking"))
                        };
                        let thought = row
                            .value
                            .get("thinkingDurationMs")
                            .and_then(Value::as_u64)
                            .filter(|_| blocks.iter().filter(|block| thinking(block)).count() == 1);
                        for (block_index, block) in blocks.iter().enumerate() {
                            let kind = block.get("type").and_then(Value::as_str);
                            let what = match (row.kind(), kind) {
                                (Some("assistant"), Some("text")) => block
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .map(|text| What::Message { text: text.to_string() }),
                                (Some("assistant"), Some("thinking")) => block
                                    .get("thinking")
                                    .and_then(Value::as_str)
                                    .map(|text| What::Thinking { text: text.to_string(), duration_ms: thought }),
                                (Some("assistant"), Some("redacted_thinking")) => {
                                    if block.get("data").is_some_and(Value::is_string) {
                                        Some(What::Thinking { text: String::new(), duration_ms: thought })
                                    } else {
                                        transcript.problem(row, "redacted thinking block has no string data");
                                        None
                                    }
                                }
                                (Some("assistant"), Some("tool_use")) => {
                                    let pair = match block.get("id").and_then(Value::as_str) { Some(id) => pairs.get(id)?, None => None };
                                    let readable = match &pair { Some(pair) => pair.readable(rows)?, None => false };
                                    if let Some(pair) = pair.filter(|_| readable) {
                                        let found = match pair.results.first() { Some(result) => Some((rows.row(result.row)?, result.index)), None => None };
                                        let result = found.as_ref().map(|(found, index)| (&**found, *index, time(found).expect("indexed time")));
                                        transcript.tool(row, at, block, result);
                                        continue;
                                    }
                                    transcript.problem(row, "invalid or ambiguous tool_use");
                                    None
                                }
                                (Some("user"), Some("tool_result")) => {
                                    let pair = match block.get("tool_use_id").and_then(Value::as_str) { Some(id) => pairs.get(id)?, None => None };
                                    if match &pair { Some(pair) => pair.readable(rows)?, None => false } {
                                        continue;
                                    }
                                    transcript.problem(row, "invalid tool_result or no unambiguous valid tool_use");
                                    None
                                }
                                (Some("user"), Some("text")) if prompt => {
                                    if block.get("text").is_some_and(Value::is_string) {
                                        continue;
                                    } else {
                                        transcript.problem(row, "text block has no string text");
                                    }
                                    None
                                }
                                (Some("user"), Some("image" | "document")) if prompt => continue,
                                _ => {
                                    if kind.is_none() {
                                        transcript.problem(row, "content block has no string type");
                                    }
                                    None
                                }
                            };
                            if let Some(what) = what {
                                transcript.finish(at, row.id(block_index), at, what, block.clone());
                            } else {
                                if row.kind() == Some("assistant") && matches!(kind, Some("text" | "thinking")) {
                                    transcript.problem(row, "prose block has no string body");
                                }
                                transcript.other(row, at, block_index, kind.unwrap_or("block"), block.clone());
                            }
                        }
                    }
                    _ => {
                        transcript.problem(row, "message content is not a supported string or block array");
                        transcript.other(row, at, 0, "record", row.value.clone());
                    }
                }
            }
            Some("attachment" | "system" | "queue-operation") => transcript.hidden(row, at),
            _ => transcript.other(row, at, 0, row.kind().expect("type checked"), row.value.clone()),
        }
        Ok(())
    })?;
    output.hand(&mut transcript.events)?;
    Ok(transcript)
}

/// The row of the compaction a summary row belongs to: the one whose record
/// names it as its anchor, else the first boundary above it along its
/// parents, past the attachments Claude Code writes between the two and
/// never across a message.
fn summarized(rows: &dyn Rows, anchors: &HashMap<String, usize>, row: &Row) -> io::Result<Option<usize>> {
    if let Some(&boundary) = row.value.get("uuid").and_then(Value::as_str).and_then(|uuid| anchors.get(uuid)) {
        return Ok(Some(boundary));
    }
    let mut parent = row.value.get("parentUuid").and_then(Value::as_str).map(str::to_owned);
    for _ in 0..32 {
        let Some(above) = parent else { return Ok(None) };
        let Some(index) = rows.index_of(&above)? else { return Ok(None) };
        let above = rows.row(index)?;
        if above.compaction() {
            return Ok(Some(index));
        }
        if matches!(above.kind(), Some("user" | "assistant")) {
            return Ok(None);
        }
        parent = above.value.get("parentUuid").and_then(Value::as_str).map(str::to_owned);
    }
    Ok(None)
}

impl Row {
    /// The row as its line holds it.
    pub(crate) fn value_of(&self) -> &Value {
        &self.value
    }

    fn user_message(&self) -> UserMessage {
        let content = match &self.value["message"]["content"] {
            Value::Array(blocks) => blocks.clone(),
            Value::String(text) => vec![json!({ "type": "text", "text": text })],
            _ => Vec::new(),
        };
        UserMessage {
            attachments: Vec::new(),
            images: Vec::new(),
            id: self.value["uuid"].as_str().expect("validated uuid").to_string(),
            client_id: None,
            content,
            hook_fragments: Vec::new(),
            presentation: Value::Null,
            detail: self.value.clone(),
        }
    }

    fn message_problem(&self) -> Option<&'static str> {
        if !matches!(self.kind(), Some("user" | "assistant")) {
            return None;
        }
        if self.value.get("uuid").and_then(Value::as_str).is_none() {
            return Some("message has no uuid");
        }
        let message = self.value.get("message").filter(|message| message.is_object());
        let Some(message) = message else { return Some("missing or invalid message") };
        if message.get("role").is_some_and(|role| role.as_str() != self.kind())
            || [
                "isMeta",
                "isCompactSummary",
                "isVisibleInTranscriptOnly",
                "isVirtual",
                "turnCompanion",
                "dispatchEcho",
                "stackedExpansion",
            ]
            .iter()
            .any(|key| self.value.get(*key).is_some_and(|value| !value.is_boolean()))
            || self
                .value
                .get("origin")
                .filter(|value| !value.is_null())
                .is_some_and(|origin| origin.get("kind").and_then(Value::as_str).is_none())
        {
            return Some("invalid message role, flag or origin");
        }
        match message.get("content") {
            Some(Value::Array(_)) => None,
            Some(Value::String(_)) if self.kind() == Some("user") => None,
            _ => Some("message content is not a supported string or block array"),
        }
    }

    fn kind(&self) -> Option<&str> {
        self.value.get("type").and_then(Value::as_str)
    }

    fn blocks(&self) -> Option<&Vec<Value>> {
        self.value.get("message")?.get("content")?.as_array()
    }

    fn id(&self, index: usize) -> String {
        match self.value.get("uuid").and_then(Value::as_str) {
            Some(uuid) => serde_json::to_string(&(uuid, index)),
            None => serde_json::to_string(&("transcript-line", self.line, index)),
        }
        .expect("a source coordinate serializes")
    }

    fn compaction(&self) -> bool {
        self.kind() == Some("system")
            && matches!(
                self.value.get("subtype").and_then(Value::as_str),
                Some("compact_boundary" | "microcompact_boundary")
            )
    }

    fn replacement(&self) -> bool {
        self.kind() == Some("system") && self.value["subtype"] == "compact_boundary"
    }

    fn prompt(&self) -> bool {
        if [
            "isCompactSummary",
            "isVisibleInTranscriptOnly",
            "isVirtual",
            "turnCompanion",
            "dispatchEcho",
            "stackedExpansion",
        ]
        .iter()
        .any(|key| self.value[*key] == true)
            || ["sourceToolAssistantUUID", "sourceToolUseID", "toolUseResult"]
                .iter()
                .any(|key| self.value.get(*key).is_some_and(|value| !value.is_null()))
            || self.blocks().is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
        {
            return false;
        }
        // System-origin submissions can be prompts, including a plugin speaking
        // as the user. Meta prompts carry input provenance too; a plugin's
        // session.append context note carries origin alone.
        if self.value["isMeta"] == true
            && !["promptSource", "turnOrigin"].iter().any(|key| self.value.get(*key).is_some_and(Value::is_string))
        {
            return false;
        }
        let first = self.value["message"]["content"]
            .as_str()
            .or_else(|| {
                self.blocks()?
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .find(|text| !text.trim().is_empty())
            })
            .unwrap_or("");
        if [
            "<local-command-caveat>",
            "<local-command-stdout>",
            "<local-command-stderr>",
            "<command-name>",
            "<command-message>",
            "[Request interrupted by user",
        ]
        .iter()
        .any(|prefix| first.trim_start().starts_with(prefix))
        {
            return false;
        }
        true
    }
}

impl Transcript {
    fn problem(&mut self, row: &Row, reason: &str) {
        self.note(format!("line {}: {reason}", row.line));
    }

    /// A problem met, kept as a reading keeps them (`note_problem`).
    fn note(&mut self, problem: String) {
        note_problem(&mut self.problems, &mut self.problems_omitted, problem);
    }

    fn finish(&mut self, at: Timestamp, id: String, started_at: Timestamp, what: What, detail: Value) {
        self.events.push(SessionEvent {
            at,
            happened: Happened::Finished {
                turn: String::new(),
                item: Item { draft: None, id, started_at: Some(started_at), detail: Some(detail), what },
            },
        });
    }

    fn other(&mut self, row: &Row, at: Timestamp, index: usize, kind: &str, detail: Value) {
        self.finish(at, row.id(index), at, What::Other { host_kind: kind.to_string() }, detail);
    }

    fn hidden(&mut self, row: &Row, at: Timestamp) {
        self.finish(
            at,
            row.id(0),
            at,
            What::Hidden { host_kind: row.kind().expect("type checked").to_string() },
            row.value.clone(),
        );
    }

    fn tool(&mut self, row: &Row, at: Timestamp, request: &Value, result: Option<(&Row, usize, Timestamp)>) {
        let answer = result.and_then(|(result_row, ..)| result_row.value.get("toolUseResult"));
        let (ended, output, status, stored) = match result {
            Some((result_row, index, ended)) => {
                let block = &result_row.blocks().expect("indexed blocks")[index];
                let status = if block["is_error"] == true { ToolStatus::Failed } else { ToolStatus::Done };
                (
                    ended,
                    content::content(&block["content"]),
                    status,
                    json!({ "uuid": result_row.value.get("uuid"), "block": block, "toolUseResult": result_row.value.get("toolUseResult"), "sourceToolAssistantUUID": result_row.value.get("sourceToolAssistantUUID") }),
                )
            }
            None => (at, Vec::new(), ToolStatus::Unknown, Value::Null),
        };
        let name = request["name"].as_str().expect("validated name");
        let call = request["id"].as_str().expect("indexed tool id").to_string();
        let mut tool = tools::tool(name, request["input"].clone(), output, status, None);
        // An Agent call tells of the agent it started; what the agent did is
        // in its own record (`read_agent`).
        let agent = matches!(name, "Agent" | "Task")
            .then(|| subagents::of_call(&call, &request["input"], answer, status == ToolStatus::Failed, &self.beside))
            .flatten();
        if let (Some((start, _)), Does::Agent { agents, .. }) = (&agent, &mut tool.does) {
            *agents = vec![start.id.clone()];
        }
        self.finish(
            ended,
            call,
            at,
            What::Tool(Box::new(tool)),
            json!({ "requested": request, "stored": stored, "uuid": row.value.get("uuid") }),
        );
        if let Some((start, end)) = agent {
            let id = start.id.clone();
            self.agents.insert(id.clone(), start.clone());
            self.changed(ended, SessionChange::AgentStarted { agent: Box::new(start) });
            if let Some(end) = end {
                self.changed(ended, SessionChange::AgentEnded { agent_id: id, end });
            }
        }
    }

    fn changed(&mut self, at: Timestamp, change: SessionChange) {
        self.events.push(SessionEvent { at, happened: Happened::Changed { turn: None, change } });
    }

    /// A row Claude Code wrote to tell the session that one of its agents
    /// that went on in the background ended: the agent's end, told here.
    fn agent_told(&mut self, row: &Row, at: Timestamp) -> bool {
        if row.value.pointer("/origin/kind").and_then(Value::as_str) != Some("task-notification") {
            return false;
        }
        let text = match &row.value["message"]["content"] {
            Value::String(text) => text.clone(),
            Value::Array(blocks) => {
                blocks.iter().filter_map(|block| block.get("text").and_then(Value::as_str)).collect()
            }
            _ => return false,
        };
        let Some(mut notified) = subagents::notified(&text) else { return false };
        let known = self.agents.get(&notified.task).map(|start| start.task.clone());
        let beside = self.beside.ids().any(|id| id == notified.task);
        if known.is_none() && !beside {
            return false;
        }
        notified.end.task = known.flatten();
        self.changed(at, SessionChange::AgentEnded { agent_id: notified.task, end: notified.end });
        self.hidden(row, at);
        true
    }
}

fn valid_call(block: &Value) -> bool {
    block.get("name").and_then(Value::as_str).is_some_and(|name| !name.is_empty())
        && block.get("input").is_some_and(Value::is_object)
}

fn valid_result(block: &Value) -> bool {
    block.get("is_error").is_none_or(Value::is_boolean)
        && matches!(block.get("content"), Some(Value::String(_) | Value::Array(_) | Value::Null) | None)
}

fn bookkeeping(kind: &str) -> bool {
    matches!(
        kind,
        "summary"
            | "custom-title"
            | "ai-title"
            | "last-prompt"
            | "tag"
            | "agent-name"
            | "agent-color"
            | "agent-setting"
            | "mode"
            | "permission-mode"
            | "atis-latch"
            | "isolation-latch"
            | "dev-mods"
            | "worktree-state"
            | "cost-state"
            | "file-history-snapshot"
            | "file-history-delta"
            | "attribution-snapshot"
            | "artifact-comment-monitor"
            | "artifact-autoreact-ledger"
            | "bridge-session"
            | "history-suppression"
            | "api-request-shape"
            | "api-request-blob"
            | "api-request"
            | "tools-used"
            | "observer-ref"
            | "fork-context-ref"
            | "relocated"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn row(n: u64, content: Value) -> Value {
        let at = format!("2026-11-12T00:00:{n:02}.000Z");
        if n.is_multiple_of(2) {
            json!({"type": "user", "uuid": format!("u{n}"), "parentUuid": if n == 0 { Value::Null } else { json!(format!("a{}", n - 1)) },
                   "sessionId": "bounded", "timestamp": at, "message": {"role": "user", "content": content}})
        } else {
            json!({"type": "assistant", "uuid": format!("a{n}"), "parentUuid": format!("u{}", n - 1), "sessionId": "bounded",
                   "timestamp": at, "message": {"role": "assistant", "content": [{"type": "text", "text": content}]}})
        }
    }

    /// A record of four rows, a fifth holding more values than World reads,
    /// and a sixth after it; and the bytes of the first four.
    fn record(name: &str) -> (std::path::PathBuf, u64) {
        let directory = std::env::temp_dir().join(format!("fastctx-bounded-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("record.jsonl");
        let mut file = File::create(&path).unwrap();
        let mut before = 0;
        for n in 0..4 {
            let mut line = serde_json::to_vec(&row(n, json!(format!("words {n}")))).unwrap();
            line.push(b'\n');
            before += line.len() as u64;
            file.write_all(&line).unwrap();
        }
        let many = Value::Array(vec![json!(0); crate::bounded::VALUES_MOST + 1]);
        for (n, content) in [(4, many), (5, json!("after"))] {
            let mut line = serde_json::to_vec(&row(n, content)).unwrap();
            line.push(b'\n');
            file.write_all(&line).unwrap();
        }
        (path, before)
    }

    /// A record of the kinds a projection tells apart: words, an answer that
    /// calls a tool and its result, a retracted answer, a row written again
    /// under its UUID, a compaction and its summary, a line not JSON, a
    /// sidechain row, a title.
    fn mixed(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("fastctx-output-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("record.jsonl");
        let at = |n: u64| format!("2026-11-12T00:00:{n:02}.000Z");
        let rows = [
            json!({"type": "custom-title", "customTitle": "mixed"}),
            json!({"type": "user", "uuid": "u0", "parentUuid": null, "timestamp": at(0), "message": {"role": "user", "content": "hello"}}),
            json!({"type": "assistant", "uuid": "a1", "parentUuid": "u0", "timestamp": at(1), "message": {"id": "m1", "role": "assistant", "model": "x",
                   "content": [{"type": "text", "text": "calling"}, {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]}}),
            json!({"type": "user", "uuid": "r1", "parentUuid": "a1", "timestamp": at(2), "sourceToolAssistantUUID": "a1",
                   "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "out"}]}}),
            json!({"type": "assistant", "uuid": "a2", "parentUuid": "r1", "timestamp": at(3), "message": {"id": "m2", "role": "assistant", "model": "x", "content": [{"type": "text", "text": "first"}]}}),
            json!({"type": "assistant", "uuid": "a3", "parentUuid": "a2", "timestamp": at(4), "supersedesUuids": ["a2"],
                   "message": {"id": "m3", "role": "assistant", "model": "x", "content": [{"type": "text", "text": "again"}]}}),
            json!({"type": "user", "uuid": "u0", "parentUuid": null, "timestamp": at(0), "message": {"role": "user", "content": "hello, written again"}}),
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "b1", "parentUuid": "a3", "logicalParentUuid": "a3", "timestamp": at(5)}),
            json!({"type": "user", "uuid": "s1", "parentUuid": "b1", "timestamp": at(6), "isCompactSummary": true, "message": {"role": "user", "content": "summary"}}),
            json!({"type": "user", "uuid": "side", "parentUuid": "s1", "timestamp": at(7), "isSidechain": true, "message": {"role": "user", "content": "aside"}}),
            json!({"type": "user", "uuid": "u9", "parentUuid": "s1", "timestamp": at(8), "message": {"role": "user", "content": "after"}}),
        ];
        let mut file = File::create(&path).unwrap();
        for (n, row) in rows.iter().enumerate() {
            let mut line = serde_json::to_vec(row).unwrap();
            line.push(b'\n');
            file.write_all(&line).unwrap();
            if n == 4 {
                file.write_all(b"{not json\n").unwrap();
            }
        }
        path
    }

    /// A history read with its events handed on as they are made shows what
    /// one held whole shows, but for the context's messages; one for a seam
    /// makes no events and the rest the same.
    #[test]
    fn a_history_handed_on_shows_what_one_held_shows() {
        let path = mixed("history");
        let held = read_continuing(&path, &[], &HashSet::new()).unwrap();
        assert!(!held.events.is_empty() && !held.context_messages.is_empty());
        let mut handed = Vec::new();
        let read = read_history(&path, &[], &HashSet::new(), &mut |event| {
            handed.push(event);
            Ok(())
        })
        .unwrap();
        assert_eq!(handed, held.events);
        assert!(read.events.is_empty() && read.context_messages.is_empty() && read.context_uuids.is_empty() && read.context_times.is_empty());
        let same = |one: &Transcript, other: &Transcript| {
            one.title == other.title
                && one.problems == other.problems
                && one.refused == other.refused
                && one.uuids == other.uuids
                && one.tail_uuid == other.tail_uuid
                && one.context_tail == other.context_tail
                && one.context_parent == other.context_parent
                && one.synthetic_tail == other.synthetic_tail
                && one.pending_cuts == other.pending_cuts
                && one.retained_tail == other.retained_tail
                && one.agents == other.agents
        };
        assert!(same(&read, &held));
        // A sink that fails stops the reading with its error.
        let failed = read_history(&path, &[], &HashSet::new(), &mut |_| Err(io::Error::other("full")));
        assert!(failed.is_err_and(|error| error.to_string() == "full"));
        let rows = |path: &Path| {
            let mut scanned = Scanned::new(0, 0, super::super::titles::Titles::default(), Vec::new(), 0);
            scan(&mut BufReader::new(File::open(path).unwrap()), None, true, &mut scanned, None).unwrap();
            scanned
        };
        let seam = project(rows(&path), &[], &HashSet::new(), subagents::Beside::default(), HashMap::new(), Output::SeamOnly).unwrap();
        let whole = project(rows(&path), &[], &HashSet::new(), subagents::Beside::default(), HashMap::new(), Output::Context).unwrap();
        assert!(seam.events.is_empty() && seam.context_messages.is_empty());
        assert!(same(&seam, &whole));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A small, fixed-seed source of choices.
    struct Choices(u64);
    impl Choices {
        fn pick(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
        fn one<'a>(&mut self, of: &[&'a str]) -> &'a str {
            of[self.pick(of.len() as u64) as usize]
        }
    }

    const UUIDS: [&str; 8] = ["u1", "u2", "u3", "u4", "u5", "u6", "u7", "u8"];
    const TOOLS: [&str; 4] = ["t1", "t2", "t3", "t4"];

    /// One line of a record of Claude Code's, of the kinds a projection tells
    /// apart, its UUIDs and tool ids from few, so that they meet: rows written
    /// again, retracted, paired, summarized and preserved, broken in the ways
    /// the projection reports.
    fn line(choices: &mut Choices) -> Vec<u8> {
        let uuid = choices.one(&UUIDS);
        let parent = if choices.pick(4) == 0 { Value::Null } else { json!(choices.one(&UUIDS)) };
        let at = match choices.pick(12) {
            0 => Value::Null,
            1 => json!("not a time"),
            n => json!(format!("2026-11-12T00:00:{:02}.000Z", n * 3)),
        };
        let text = |choices: &mut Choices| json!(choices.one(&["hello", "<command-name>x", "[Request interrupted by user", "words"]));
        let mut row = match choices.pick(14) {
            0 | 1 => json!({"type": "user", "message": {"role": "user", "content": text(choices)}}),
            2 => json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text(choices)}, {"type": "image", "source": {}}]}}),
            3 | 4 => {
                let mut blocks = vec![json!({"type": "text", "text": "answer"})];
                if choices.pick(2) == 0 {
                    blocks.push(json!({"type": "tool_use", "id": choices.one(&TOOLS), "name": choices.one(&["Bash", "", "Agent"]), "input": if choices.pick(5) == 0 { json!("x") } else { json!({"command": "ls"}) }}));
                }
                if choices.pick(4) == 0 {
                    blocks.push(json!({"type": "thinking", "thinking": "hm"}));
                }
                let mut row = json!({"type": "assistant", "message": {"id": choices.one(&["m1", "m2", ""]), "role": "assistant", "model": "x", "content": blocks}});
                match choices.pick(6) {
                    0 => row["supersedesUuids"] = json!([choices.one(&UUIDS)]),
                    1 => row["supersedesUuids"] = Value::Null,
                    2 => row["supersedesUuids"] = json!([1]),
                    _ => {}
                }
                row
            }
            5 | 6 => {
                let mut row = json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": choices.one(&TOOLS),
                    "content": if choices.pick(6) == 0 { json!(7) } else { json!("out") }, "is_error": choices.pick(5) == 0}]}});
                match choices.pick(4) {
                    0 => {}
                    1 => row["sourceToolAssistantUUID"] = Value::Null,
                    2 => row["sourceToolAssistantUUID"] = json!(3),
                    _ => row["sourceToolAssistantUUID"] = json!(choices.one(&UUIDS)),
                }
                row
            }
            7 => json!({"type": "system", "subtype": "model_refusal_fallback", "retractedMessageUuids": if choices.pick(4) == 0 { json!(null) } else { json!([choices.one(&UUIDS)]) }}),
            8 => json!({"type": "system", "subtype": "informational", "retractedMessageUuids": [choices.one(&UUIDS)]}),
            9 => {
                let metadata = match choices.pick(5) {
                    0 => json!({"preservedSegment": {"anchorUuid": choices.one(&UUIDS), "headUuid": choices.one(&UUIDS), "tailUuid": choices.one(&UUIDS)}}),
                    1 => json!({"preservedMessages": {"anchorUuid": choices.one(&UUIDS), "uuids": [choices.one(&UUIDS), choices.one(&UUIDS)]}}),
                    2 => json!({"preservedMessages": null, "preservedSegment": {"anchorUuid": choices.one(&UUIDS), "headUuid": choices.one(&UUIDS), "tailUuid": choices.one(&UUIDS)}}),
                    3 => json!({}),
                    _ => Value::Null,
                };
                json!({"type": "system", "subtype": choices.one(&["compact_boundary", "microcompact_boundary"]), "logicalParentUuid": choices.one(&UUIDS), "compactMetadata": metadata})
            }
            10 => json!({"type": "user", "isCompactSummary": true, "message": {"role": "user", "content": "summary"}}),
            11 => json!({"type": "attachment", "attachment": {"type": "queued_command", "source_uuid": choices.one(&UUIDS)}}),
            12 => json!({"type": "assistant", "message": {"role": "assistant", "model": "<synthetic>", "content": [{"type": "text", "text": "No response requested."}]}}),
            _ => json!({"type": choices.one(&["custom-title", "progress", "summary", "mystery"]), "customTitle": "t"}),
        };
        if choices.pick(10) != 0 {
            row["uuid"] = json!(uuid);
        }
        if choices.pick(12) == 0 {
            row["isSidechain"] = json!(true);
        }
        if choices.pick(15) == 0 {
            row["origin"] = json!({"kind": "task-notification"});
        }
        row["parentUuid"] = parent;
        row["timestamp"] = at;
        let mut line = if choices.pick(40) == 0 { b"{broken".to_vec() } else { serde_json::to_vec(&row).unwrap() };
        line.push(b'\n');
        line
    }

    fn scanned(bytes: &[u8]) -> Scanned {
        let mut scanned = Scanned::new(0, 0, super::super::titles::Titles::default(), Vec::new(), 0);
        scan(&mut BufReader::new(bytes), None, true, &mut scanned, None).unwrap();
        scanned
    }

    /// Rows kept as their lines and parsed again as they are read back
    /// (`RowStore`), as the node's work store keeps them.
    #[derive(Default)]
    struct Lines {
        rows: Vec<(usize, u64, Vec<u8>)>,
        by_uuid: HashMap<String, usize>,
        blocks: RefCell<Vec<(String, bool, usize, usize)>>,
        members: RefCell<HashSet<(u8, String)>>,
    }

    impl RowStore for Lines {
        fn count(&self) -> usize {
            self.rows.len()
        }
        fn push(&mut self, uuid: Option<&str>, line: usize, start: u64, bytes: &[u8]) -> io::Result<()> {
            if let Some(uuid) = uuid {
                assert!(self.by_uuid.insert(uuid.to_owned(), self.rows.len()).is_none(), "a UUID kept twice");
            }
            self.rows.push((line, start, bytes.to_vec()));
            Ok(())
        }
        fn replace(&mut self, index: usize, line: usize, start: u64, bytes: &[u8]) -> io::Result<()> {
            self.rows[index] = (line, start, bytes.to_vec());
            Ok(())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn note_block(&self, id: &str, result: bool, row: usize, index: usize) -> io::Result<()> {
            self.blocks.borrow_mut().push((id.to_owned(), result, row, index));
            Ok(())
        }
        fn blocks(&self, id: &str) -> io::Result<Vec<(bool, usize, usize)>> {
            let blocks = self.blocks.borrow();
            let side = |result: bool| {
                blocks.iter().filter(move |(noted, of, ..)| noted == id && *of == result).take(2).map(|(_, result, row, index)| (*result, *row, *index))
            };
            Ok(side(false).chain(side(true)).collect())
        }
        fn note_member(&self, set: u8, key: &str) -> io::Result<()> {
            self.members.borrow_mut().insert((set, key.to_owned()));
            Ok(())
        }
        fn is_member(&self, set: u8, key: &str) -> io::Result<bool> {
            Ok(self.members.borrow().contains(&(set, key.to_owned())))
        }
        fn index_of(&self, uuid: &str) -> io::Result<Option<usize>> {
            Ok(self.by_uuid.get(uuid).copied())
        }
        fn row(&self, index: usize) -> io::Result<(usize, u64, Vec<u8>)> {
            Ok(self.rows[index].clone())
        }
        fn each(&self, visit: VisitLine<'_>) -> io::Result<()> {
            self.rows.iter().enumerate().try_for_each(|(index, (line, start, bytes))| visit(index, *line, *start, bytes))
        }
        fn ids_before(&self, to: u64, each: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()> {
            let mut ids: Vec<(usize, &String)> = self.by_uuid.iter().filter(|(_, index)| self.rows[**index].1 < to).map(|(uuid, index)| (*index, uuid)).collect();
            ids.sort();
            ids.into_iter().try_for_each(|(_, uuid)| each(uuid))
        }
        fn clear(&mut self) -> io::Result<()> {
            *self = Lines::default();
            Ok(())
        }
    }

    /// One exchange of a record a follower reads, its times rising from
    /// `at` (seconds): the user's words, mostly an answer calling a tool
    /// and its result — now and then none, the call left waiting — and a
    /// closing answer; now and then a line of `line`'s, written again,
    /// broken or odd.
    fn exchange(choices: &mut Choices, round: usize, at: &mut u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        let push = |row: Value, bytes: &mut Vec<u8>| {
            bytes.extend(serde_json::to_vec(&row).unwrap());
            bytes.push(b'\n');
        };
        let time = |at: &mut u64, choices: &mut Choices| {
            *at += choices.pick(3);
            json!(format!("2026-11-12T{:02}:{:02}:{:02}.000Z", *at / 3600 % 24, *at / 60 % 60, *at % 60))
        };
        let call = format!("k{}", round % 6);
        let words = json!(choices.one(&["hello", "words", "<command-name>x"]));
        push(json!({"type": "user", "uuid": format!("p{round}"), "parentUuid": null, "timestamp": time(at, choices), "message": {"role": "user", "content": words}}), &mut bytes);
        if choices.pick(3) != 0 {
            let name = choices.one(&["Bash", "Agent"]);
            push(
                json!({"type": "assistant", "uuid": format!("a{round}"), "timestamp": time(at, choices),
                       "message": {"id": format!("m{round}"), "role": "assistant", "model": "x",
                                   "content": [{"type": "text", "text": "answer"}, {"type": "tool_use", "id": call, "name": name, "input": {"command": "ls"}}]}}),
                &mut bytes,
            );
            if choices.pick(4) != 0 {
                push(
                    json!({"type": "user", "uuid": format!("r{round}"), "timestamp": time(at, choices),
                           "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": call, "content": "out"}]}}),
                    &mut bytes,
                );
            }
        }
        push(
            json!({"type": "assistant", "uuid": format!("c{round}"), "timestamp": time(at, choices),
                   "message": {"id": format!("n{round}"), "role": "assistant", "model": "x", "content": [{"type": "text", "text": "done"}]}}),
            &mut bytes,
        );
        if choices.pick(8) == 0 {
            bytes.extend(line(choices));
        }
        bytes
    }

    /// What a follower's reading found, to be compared: its events (`events`
    /// where they were handed on), the transcript's title, problems and
    /// agents, the seam (the calls spoken past and the agents told of as
    /// sets), where it ended, its times, the record ids the seam moved past
    /// (`moved` where they were handed on) and, of `asked`, those the
    /// record holds.
    fn tail_shown(read: io::Result<Result<follow::Tail, follow::Restart>>, events: Option<Vec<SessionEvent>>, moved: Option<Vec<String>>, asked: &[String]) -> Value {
        match read {
            Err(error) => json!({ "error": error.to_string() }),
            Ok(Err(restart)) => json!({ "restart": restart.to_string() }),
            Ok(Ok(tail)) => {
                let events = events.unwrap_or_else(|| tail.transcript.events.clone());
                let moved = moved.unwrap_or_else(|| tail.prefix_ids.clone());
                let mut uuids: Vec<&String> = tail.transcript.uuids.iter().filter(|uuid| asked.contains(uuid)).collect();
                uuids.sort();
                // Lists the reading keeps as sets, made in an order of no
                // meaning (a map's).
                let seam = tail.seam.clone().map(|mut seam| {
                    seam.abandoned.sort();
                    seam.told.sort();
                    seam.unlinked.sort();
                    serde_json::to_value(seam).unwrap()
                });
                json!({ "events": serde_json::to_value(events).unwrap(), "title": tail.transcript.title, "problems": tail.transcript.problems,
                        "omitted": tail.transcript.problems_omitted, "agents": serde_json::to_value(tail.transcript.agents()).unwrap(),
                        "refused": tail.transcript.refused, "seam": seam, "end": tail.end, "since": tail.since.map(|at| at.to_string()),
                        "seam_at": tail.seam_at.map(|at| at.to_string()), "moved": moved, "uuids": uuids })
            }
        }
    }

    /// A follower's reading with its rows kept in a store (`follow::tail_kept`,
    /// design §28.51 ⑤-5) finds what one with its rows held finds
    /// (`follow::tail`): the same events, transcript, seam, times and record
    /// ids moved past, the same restarts — read from the start and on from
    /// the seams found, over records growing, rewritten shorter, ending
    /// without a line's end, with agents told of beside them.
    #[test]
    fn a_tail_read_into_a_store_is_the_tail_held() {
        let dir = std::env::temp_dir().join(format!("fastctx-tail-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut choices = Choices(0x2545_f491_4f6c_dd1d);
        // How often: a seam moved from the start, a reading on past a seam,
        // a restart, ids moved past on from a seam, an agent told of.
        let mut met = [0usize; 5];
        for round in 0..2_000 {
            let path = dir.join(format!("record-{round}.jsonl"));
            let realistic = choices.pick(4) != 0;
            let (mut bytes, mut at, mut next) = (Vec::new(), 0u64, 0usize);
            let mut grow = |bytes: &mut Vec<u8>, choices: &mut Choices| {
                let count = 1 + choices.pick(10);
                for _ in 0..count {
                    if realistic {
                        bytes.extend(exchange(choices, next, &mut at));
                        next += 1;
                    } else {
                        bytes.extend(line(choices));
                    }
                }
            };
            grow(&mut bytes, &mut choices);
            let earlier: HashSet<String> = ["p0", "u1", "a2"].iter().filter(|_| choices.pick(6) == 0).map(|uuid| uuid.to_string()).collect();
            let cutoff = choices.pick(3);
            let limit = Timestamp::from_second(1_794_441_600 + choices.pick(40) as i64).unwrap();
            let accept = move |at: Timestamp| cutoff != 0 || at < limit;
            let mut seam = follow::Seam::default();
            let mut seen: HashSet<String> = HashSet::new();
            for step in 0..4 {
                let mut written = bytes.clone();
                // Now and then a last row without its line's end.
                if choices.pick(6) == 0 {
                    written.extend_from_slice(b"{\"type\": \"user\", \"uuid\": \"partial\"");
                }
                std::fs::write(&path, &written).unwrap();
                if choices.pick(4) == 0 {
                    let agents = path.with_extension("").join("subagents");
                    std::fs::create_dir_all(&agents).unwrap();
                    let id = format!("x{}", choices.pick(3));
                    let call = format!("k{}", choices.pick(6));
                    std::fs::write(agents.join(format!("agent-{id}.meta.json")), json!({"toolUseId": call, "agentType": "general"}).to_string()).unwrap();
                    met[4] += 1;
                }
                let ended = choices.pick(3) == 0;
                let asked: Vec<String> = (0..choices.pick(3)).map(|_| choices.one(&["p0", "p1", "a1", "r2", "u1", "u3", "none"]).to_string()).collect();
                let held = follow::tail(&path, &seam, &earlier, &accept, ended, &|id| seen.contains(id));
                let mut lines = Lines::default();
                let (mut events, mut moved) = (Vec::new(), Vec::new());
                let kept = follow::tail_kept(
                    &path,
                    &seam,
                    &|uuid| Ok(earlier.contains(uuid)),
                    &accept,
                    ended,
                    &|ids| Ok(ids.iter().filter(|id| seen.contains(*id)).cloned().collect()),
                    follow::TailStore {
                        rows: &mut lines,
                        events: &mut |event| {
                            events.push(event);
                            Ok(())
                        },
                        moved_past: &mut |id| {
                            moved.push(id.to_string());
                            Ok(())
                        },
                        asked: &asked,
                    },
                );
                let next_seam = match &held {
                    Ok(Ok(tail)) => {
                        if tail.seam.as_ref().is_some_and(|next| next.offset > seam.offset) {
                            met[if seam.offset == 0 { 0 } else { 3 }] += 1;
                        }
                        met[1] += usize::from(seam.offset > 0);
                        seen.extend(tail.prefix_ids.iter().cloned());
                        tail.seam.clone()
                    }
                    Ok(Err(_)) => {
                        met[2] += 1;
                        Some(follow::Seam::default())
                    }
                    Err(_) => None,
                };
                let shown_held = tail_shown(held, None, None, &asked);
                let shown_kept = tail_shown(kept, Some(events), Some(moved), &asked);
                if shown_kept != shown_held {
                    let keys: Vec<String> = shown_held.as_object().map(|held| held.keys().cloned().collect()).unwrap_or_default();
                    let differing: Vec<String> = keys
                        .iter()
                        .filter(|key| shown_kept.get(key.as_str()) != shown_held.get(key.as_str()))
                        .map(|key| format!("{key}: kept {} / held {}", shown_kept.get(key.as_str()).cloned().unwrap_or_default(), shown_held[key.as_str()]))
                        .collect();
                    panic!("round {round}, step {step}, from {seam:?}: {differing:#?} (kept {shown_kept}) of {}", String::from_utf8_lossy(&written));
                }
                if let Some(next_seam) = next_seam {
                    if next_seam.offset == 0 {
                        seen.clear();
                    }
                    seam = next_seam;
                }
                // The record grows — or, now and then, is written again
                // shorter than where it was read through.
                if choices.pick(10) == 0 {
                    bytes.truncate(bytes.len() / 2);
                } else {
                    grow(&mut bytes, &mut choices);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(met.iter().all(|count| *count > 20), "{met:?}");
    }

    /// A row the store fails to give fails the reading — here one only the
    /// context's order reads, walking a compaction's kept segment — rather
    /// than leave the context unknown as though the record said so.
    #[test]
    fn a_row_the_store_fails_to_give_fails_the_reading() {
        struct Failing(Lines, usize);
        impl RowStore for Failing {
            fn count(&self) -> usize {
                self.0.count()
            }
            fn push(&mut self, uuid: Option<&str>, line: usize, start: u64, bytes: &[u8]) -> io::Result<()> {
                self.0.push(uuid, line, start, bytes)
            }
            fn replace(&mut self, index: usize, line: usize, start: u64, bytes: &[u8]) -> io::Result<()> {
                self.0.replace(index, line, start, bytes)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
            fn note_block(&self, id: &str, result: bool, row: usize, index: usize) -> io::Result<()> {
                self.0.note_block(id, result, row, index)
            }
            fn blocks(&self, id: &str) -> io::Result<Vec<(bool, usize, usize)>> {
                self.0.blocks(id)
            }
            fn note_member(&self, set: u8, key: &str) -> io::Result<()> {
                self.0.note_member(set, key)
            }
            fn is_member(&self, set: u8, key: &str) -> io::Result<bool> {
                self.0.is_member(set, key)
            }
            fn index_of(&self, uuid: &str) -> io::Result<Option<usize>> {
                self.0.index_of(uuid)
            }
            fn row(&self, index: usize) -> io::Result<(usize, u64, Vec<u8>)> {
                if index == self.1 {
                    return Err(io::Error::other("the store failed"));
                }
                self.0.row(index)
            }
            fn each(&self, visit: VisitLine<'_>) -> io::Result<()> {
                self.0.each(visit)
            }
        }
        let at = |second: usize| format!("2026-01-01T00:00:{second:02}Z");
        let mut lines = vec![
            json!({"type": "user", "uuid": "u1", "timestamp": at(0), "message": {"role": "user", "content": "first"}}),
            json!({"type": "user", "uuid": "u2", "parentUuid": "u1", "timestamp": at(1), "message": {"role": "user", "content": "second"}}),
        ];
        // More rows than a store's reading keeps parsed (`RECENT`): the
        // segment's tail is read from the store again.
        lines.extend((0..RECENT + 2).map(|n| json!({"type": "user", "uuid": format!("f{n}"), "timestamp": at(2 + n), "message": {"role": "user", "content": "more"}})));
        lines.push(json!({"type": "system", "subtype": "compact_boundary", "uuid": "b1", "logicalParentUuid": "u2", "timestamp": at(20),
                          "compactMetadata": {"preservedSegment": {"anchorUuid": "s1", "headUuid": "u1", "tailUuid": "u2"}}}));
        lines.push(json!({"type": "user", "uuid": "s1", "parentUuid": "b1", "isCompactSummary": true, "timestamp": at(21), "message": {"role": "user", "content": "summary"}}));
        let bytes: Vec<u8> = lines.iter().flat_map(|line| format!("{line}\n").into_bytes()).collect();
        let read = |fail: usize| {
            let mut store = Failing(Lines::default(), fail);
            let mut scanned = Scanned::with(Kept::new(&mut store), 0, 0, super::super::titles::Titles::default(), Vec::new(), 0);
            scan(&mut BufReader::new(&bytes[..]), None, true, &mut scanned, None).unwrap();
            project(scanned, &[], &HashSet::new(), subagents::Beside::default(), HashMap::new(), Output::Context)
        };
        // The segment's tail is read only to walk to its head.
        let failed = read(1);
        assert!(failed.as_ref().is_err_and(|error| error.to_string() == "the store failed"), "{:?}", failed.map(|read| read.problems));
        let whole = read(usize::MAX).unwrap();
        assert!(whole.problems.is_empty(), "{:?}", whole.problems);
        assert_eq!(whole.context_uuids, ["s1", "u1", "u2"]);
    }

    /// A row of many tool calls, their results in one row after it, is read
    /// from the store a few times, not again for each call it pairs: the row
    /// read in order and those read lately are kept parsed.
    #[test]
    fn a_row_of_many_calls_is_not_read_again_for_each() {
        struct Counted(Lines, std::cell::Cell<usize>);
        impl RowStore for Counted {
            fn count(&self) -> usize {
                self.0.count()
            }
            fn push(&mut self, uuid: Option<&str>, line: usize, start: u64, bytes: &[u8]) -> io::Result<()> {
                self.0.push(uuid, line, start, bytes)
            }
            fn replace(&mut self, index: usize, line: usize, start: u64, bytes: &[u8]) -> io::Result<()> {
                self.0.replace(index, line, start, bytes)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
            fn note_block(&self, id: &str, result: bool, row: usize, index: usize) -> io::Result<()> {
                self.0.note_block(id, result, row, index)
            }
            fn blocks(&self, id: &str) -> io::Result<Vec<(bool, usize, usize)>> {
                self.0.blocks(id)
            }
            fn note_member(&self, set: u8, key: &str) -> io::Result<()> {
                self.0.note_member(set, key)
            }
            fn is_member(&self, set: u8, key: &str) -> io::Result<bool> {
                self.0.is_member(set, key)
            }
            fn index_of(&self, uuid: &str) -> io::Result<Option<usize>> {
                self.0.index_of(uuid)
            }
            fn row(&self, index: usize) -> io::Result<(usize, u64, Vec<u8>)> {
                self.1.set(self.1.get() + 1);
                self.0.row(index)
            }
            fn each(&self, visit: VisitLine<'_>) -> io::Result<()> {
                self.0.each(visit)
            }
        }
        let calls: Vec<Value> = (0..200).map(|n| json!({"type": "tool_use", "id": format!("t{n}"), "name": "Bash", "input": {"command": "ls"}})).collect();
        let results: Vec<Value> = (0..200).map(|n| json!({"type": "tool_result", "tool_use_id": format!("t{n}"), "content": "out"})).collect();
        let lines = [
            json!({"type": "user", "uuid": "u1", "timestamp": "2026-01-01T00:00:00Z", "message": {"role": "user", "content": "go"}}),
            json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1", "timestamp": "2026-01-01T00:00:01Z",
                   "message": {"id": "m1", "role": "assistant", "model": "m", "content": calls}}),
            json!({"type": "user", "uuid": "r1", "parentUuid": "a1", "timestamp": "2026-01-01T00:00:02Z", "message": {"role": "user", "content": results}}),
        ];
        let bytes: Vec<u8> = lines.iter().flat_map(|line| format!("{line}\n").into_bytes()).collect();
        let mut store = Counted(Lines::default(), std::cell::Cell::new(0));
        let mut scanned = Scanned::with(Kept::new(&mut store), 0, 0, super::super::titles::Titles::default(), Vec::new(), 0);
        scan(&mut BufReader::new(&bytes[..]), None, true, &mut scanned, None).unwrap();
        let read = project(scanned, &[], &HashSet::new(), subagents::Beside::default(), HashMap::new(), Output::Context).unwrap();
        let tools = read.events.iter().filter(|event| matches!(&event.happened, Happened::Finished { item, .. } if matches!(item.what, What::Tool(_)))).count();
        assert_eq!(tools, 200);
        assert!(store.1.get() <= 8, "{} rows read from the store", store.1.get());
    }

    /// A record of more problems than a reading keeps word for word keeps
    /// the first, counts the rest, and lists them so.
    #[test]
    fn many_problems_are_counted_past_the_first() {
        let mut bytes = Vec::new();
        bytes.extend(format!("{}\n", json!({"type": "user", "uuid": "u0", "timestamp": "2026-01-01T00:00:00Z", "message": {"role": "user", "content": "hi"}})).into_bytes());
        for _ in 0..PROBLEMS_KEPT + 50 {
            bytes.extend(b"not json\n");
        }
        let held = project(scanned(&bytes), &[], &HashSet::new(), subagents::Beside::default(), HashMap::new(), Output::Context).unwrap();
        assert_eq!((held.problems.len(), held.problems_omitted), (PROBLEMS_KEPT, 50));
        assert!(held.problems[0].starts_with("line 2: invalid JSON"));
        assert!(listed(&held.problems, held.problems_omitted).ends_with("; and 50 more"));
        let kept = kept(&bytes, &[], &HashSet::new()).unwrap();
        assert_eq!((kept.problems, kept.problems_omitted), (held.problems, 50));
    }

    /// Rows taking back more UUIDs, and more queued messages, than a reading
    /// holds in memory (`MEMBERS_HELD`) read as rows held whole read: the
    /// sets are noted in the store and asked of it.
    #[test]
    fn many_taken_back_and_queued_read_as_held() {
        let at = |n: usize| format!("2026-01-01T00:{:02}:{:02}Z", n / 60, n % 60);
        let many = MEMBERS_HELD + 36;
        let mut lines: Vec<Value> = (0..many)
            .map(|n| json!({"type": "user", "uuid": format!("u{n}"), "timestamp": at(n), "message": {"role": "user", "content": format!("words {n}")}}))
            .collect();
        let taken: Vec<String> = (0..many).step_by(2).map(|n| format!("u{n}")).chain((0..many).map(|n| format!("x{n}"))).collect();
        lines.push(json!({"type": "assistant", "uuid": "a", "timestamp": at(many), "supersedesUuids": taken,
                          "message": {"id": "m", "role": "assistant", "model": "m", "content": [{"type": "text", "text": "again"}]}}));
        lines.extend((0..many).map(|n| json!({"type": "attachment", "uuid": format!("q{n}"), "timestamp": at(many + 1),
                                               "attachment": {"type": "queued_command", "source_uuid": format!("s{n}")}})));
        let bytes: Vec<u8> = lines.iter().flat_map(|line| format!("{line}\n").into_bytes()).collect();
        let held = project(scanned(&bytes), &[], &HashSet::new(), subagents::Beside::default(), HashMap::new(), Output::Context);
        let held_read = held.as_ref().unwrap();
        // Words of an even row taken back, of an odd one left.
        let heard = |words: &str| {
            held_read.events.iter().any(|event| matches!(&event.happened, Happened::Heard { text, .. } if text == words))
        };
        assert!(held_read.uuids.contains("s1") && !heard("words 0") && heard("words 1"));
        assert_eq!(shown(&kept(&bytes, &[], &HashSet::new())), shown(&held));
    }

    /// Many rows naming one tool use's id read as rows held whole read, the
    /// store asked for no more than the first few of them each time.
    #[test]
    fn many_blocks_of_one_id_are_judged_by_the_first() {
        let mut lines = Vec::new();
        for n in 0..300 {
            let at = format!("2026-01-01T00:{:02}:{:02}Z", n / 60, n % 60);
            lines.push(json!({"type": "assistant", "uuid": format!("a{n}"), "timestamp": at,
                              "message": {"id": format!("m{n}"), "role": "assistant", "model": "m", "content": [{"type": "tool_use", "id": "same", "name": "Bash", "input": {}}]}}));
            if n % 3 == 0 {
                lines.push(json!({"type": "user", "uuid": format!("r{n}"), "timestamp": at, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "same", "content": "out"}]}}));
            }
        }
        let bytes: Vec<u8> = lines.iter().flat_map(|line| format!("{line}\n").into_bytes()).collect();
        let held = project(scanned(&bytes), &[], &HashSet::new(), subagents::Beside::default(), HashMap::new(), Output::Context);
        assert!(held.as_ref().unwrap().problems.iter().any(|problem| problem.contains("ambiguous")));
        assert_eq!(shown(&kept(&bytes, &[], &HashSet::new())), shown(&held));
        let lines = Lines::default();
        for n in 0..300 {
            lines.note_block("same", n % 2 == 0, n, 0).unwrap();
        }
        assert_eq!(lines.blocks("same").unwrap(), vec![(false, 1, 0), (false, 3, 0), (true, 0, 0), (true, 2, 0)]);
    }

    /// A set of a few long keys goes to the store by the bytes it holds,
    /// though it holds fewer keys than `MEMBERS_HELD`.
    #[test]
    fn a_set_of_long_keys_goes_to_the_store_by_its_bytes() {
        let lines = Lines::default();
        let mut members = Members::new(SET_QUEUED);
        let long = |n: usize| format!("{n}-{}", "k".repeat(300));
        for n in 0..20 {
            members.insert(long(n), Some(&lines)).unwrap();
        }
        assert!(members.noted && members.held.is_empty());
        assert!(members.contains(&long(3), Some(&lines)).unwrap() && !members.contains("other", Some(&lines)).unwrap());
        // Rows held in memory hold their sets there.
        let mut held = Members::new(SET_QUEUED);
        for n in 0..20 {
            held.insert(long(n), None).unwrap();
        }
        assert!(!held.noted && held.contains(&long(3), None).unwrap());
    }

    /// A seam kept before a reading kept no more than a few problems is taken
    /// up with the first kept and the rest counted.
    #[test]
    fn a_seam_of_many_problems_is_taken_up_as_counted() {
        let problems: Vec<String> = (0..PROBLEMS_KEPT + 30).map(|n| format!("p{n}")).collect();
        let scanned = Scanned::new(0, 0, super::super::titles::Titles::default(), problems, 5);
        assert_eq!((scanned.problems.len(), scanned.problems_omitted), (PROBLEMS_KEPT, 35));
        assert_eq!(scanned.problems[0], "p0");
    }

    /// What the rows `bytes` hold show, kept in a store as they are scanned:
    /// with the store's UUIDs, as a reading held whole gives them.
    fn kept(bytes: &[u8], cuts: &[HistoryCut], earlier: &HashSet<String>) -> io::Result<Transcript> {
        let mut lines = Lines::default();
        let mut scanned = Scanned::with(Kept::new(&mut lines), 0, 0, super::super::titles::Titles::default(), Vec::new(), 0);
        scan(&mut BufReader::new(bytes), None, true, &mut scanned, None).unwrap();
        scanned.rows.scanned()?;
        let mut read = project(scanned, cuts, earlier, subagents::Beside::default(), HashMap::new(), Output::Context)?;
        read.uuids.extend(lines.by_uuid.into_keys());
        read.uuids.extend(lines.members.into_inner().into_iter().filter(|(set, _)| *set == SET_QUEUED).map(|(_, key)| key));
        Ok(read)
    }

    /// What a projection made, compared field by field.
    fn shown(read: &io::Result<Transcript>) -> String {
        match read {
            Err(error) => format!("error: {error}"),
            Ok(read) => {
                let mut uuids: Vec<&String> = read.uuids.iter().collect();
                uuids.sort();
                let mut agents: Vec<(&String, &HostAgentStart)> = read.agents.iter().collect();
                agents.sort_by_key(|(id, _)| *id);
                format!(
                    "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
                    read.events,
                    read.problems,
                    read.title,
                    uuids,
                    read.tail_uuid,
                    read.context_uuids,
                    read.context_parent,
                    read.context_tail,
                    read.context_times,
                    read.context_messages,
                    read.synthetic_tail,
                    read.pending_cuts,
                    read.retained_tail,
                    read.refused,
                    agents
                )
            }
        }
    }

    /// Thousands of small records, as tangled as the kinds allow: the
    /// projection shows each as the projection before it did (`before`),
    /// field for field, failures too.
    #[test]
    fn the_projection_is_the_projection_it_was() {
        let mut choices = Choices(0x9e37_79b9_7f4a_7c15);
        // How often what the rules meet was met: tools paired, compactions
        // told, a context its compaction left unknown, a rewind failing, a
        // retraction taking a row away, context held, context held where a
        // compaction kept messages.
        let mut met = [0usize; 7];
        for round in 0..20_000 {
            let bytes: Vec<u8> = (0..choices.pick(24)).flat_map(|_| line(&mut choices)).collect();
            let earlier: HashSet<String> = UUIDS.iter().filter(|_| choices.pick(8) == 0).map(|uuid| uuid.to_string()).collect();
            let cuts: Vec<HistoryCut> = (0..u64::from(choices.pick(6) == 0))
                .map(|_| HistoryCut { until: choices.one(&UUIDS).into(), parent: choices.one(&UUIDS).into(), messages: 0, retained: Vec::new() })
                .collect();
            let now = project(scanned(&bytes), &cuts, &earlier, subagents::Beside::default(), HashMap::new(), Output::Context);
            let was = before::project(scanned(&bytes), &cuts, &earlier, subagents::Beside::default(), HashMap::new(), Output::Context);
            assert_eq!(shown(&now), shown(&was), "round {round}: {}", String::from_utf8_lossy(&bytes));
            assert_eq!(shown(&kept(&bytes, &cuts, &earlier)), shown(&now), "round {round}, kept: {}", String::from_utf8_lossy(&bytes));
            match &now {
                Err(_) => met[3] += 1,
                Ok(read) => {
                    let finished = |what: fn(&What) -> bool| {
                        read.events.iter().any(|event| matches!(&event.happened, Happened::Finished { item, .. } if what(&item.what)))
                    };
                    met[0] += usize::from(finished(|what| matches!(what, What::Tool(_))));
                    met[1] += usize::from(finished(|what| matches!(what, What::Compaction)));
                    met[2] += usize::from(read.problems.iter().any(|problem| problem.contains("the context its last compaction left")));
                    met[4] += usize::from(bytes.windows(15).any(|window| window == b"supersedesUuids") && read.events.len() < read.uuids.len());
                    met[5] += usize::from(!read.context_messages.is_empty());
                    met[6] += usize::from(
                        !read.context_messages.is_empty() && bytes.windows(10).any(|window| window == b"anchorUuid"),
                    );
                }
            }
        }
        assert!(met.iter().all(|count| *count > 20), "{met:?}");
    }

    /// A record read through a seam is checked whole by its blocks' hashes
    /// (design §28.53): the same file grown still holds; a byte before the
    /// seam changed in place, or the file put in another's place, does not.
    #[test]
    fn the_bytes_before_a_seam_are_checked_whole_by_their_hashes() {
        let dir = std::env::temp_dir().join(format!("fastctx-prefix-checked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("record.jsonl");
        let mut choices = Choices(0x6c8e_9cf5_7086_4e2b);
        let (mut bytes, mut at) = (Vec::new(), 0u64);
        let mut round = 0;
        while bytes.len() < 3 << 20 {
            bytes.extend(exchange(&mut choices, round, &mut at));
            round += 1;
        }
        std::fs::write(&path, &bytes).unwrap();
        let tail = follow::tail(&path, &follow::Seam::default(), &HashSet::new(), &|_| true, true, &|_| false)
            .unwrap()
            .unwrap_or_else(|why| panic!("restarted: {why}"));
        // The file as read, through two and a half blocks: hashed a block at
        // a time as a reading hashes what it read.
        let mut seam = tail.seam.expect("a seam");
        seam.offset = 5 << 19;
        seam.blocks = bytes[..seam.offset as usize].chunks(1 << 20).map(xxhash_rust::xxh3::xxh3_64).collect();
        assert!(follow::prefix_unchanged(&path, &seam, &|| false).unwrap());
        // Grown: what was read through holds still.
        let mut grown = bytes.clone();
        grown.extend(exchange(&mut choices, round, &mut at));
        std::fs::write(&path, &grown).unwrap();
        assert!(follow::prefix_unchanged(&path, &seam, &|| false).unwrap());
        // A byte before the seam changed in place, each block in turn.
        for block in 0..seam.offset.div_ceil(1 << 20) {
            let mut changed = grown.clone();
            let at = (block << 20) as usize + 7;
            changed[at] = if changed[at] == b'x' { b'y' } else { b'x' };
            {
                use std::io::{Seek, SeekFrom, Write};
                let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.seek(SeekFrom::Start(0)).unwrap();
                file.write_all(&changed).unwrap();
            }
            assert!(!follow::prefix_unchanged(&path, &seam, &|| false).unwrap(), "block {block}");
            let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            std::io::Write::write_all(&mut file, &grown).unwrap();
        }
        assert!(follow::prefix_unchanged(&path, &seam, &|| false).unwrap());
        // Another file in its place, the same bytes.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, &grown).unwrap();
        assert!(!follow::prefix_unchanged(&path, &seam, &|| false).unwrap());
        // A seam at the start has nothing before it but the file itself.
        let start = follow::Seam { offset: 0, blocks: Vec::new(), identity: seam.identity.clone(), ..follow::Seam::default() };
        assert!(!follow::prefix_unchanged(&path, &start, &|| false).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_larger_than_world_reads_ends_a_history_shown_and_fails_what_needs_it_whole() {
        let (path, before) = record("history");
        // What needs the record whole — a fork, a rewind, the context — fails.
        let whole = read(&path).unwrap_err();
        assert!(crate::bounded::is_refused(&whole) && whole.to_string().contains("line 5"), "{whole}");
        // A history shown goes up to it, and says why no further.
        let shown = read_continuing(&path, &[], &HashSet::new()).unwrap();
        assert!(shown.problems.iter().any(|problem| problem.contains("line 5") && problem.contains("more than World reads")));
        // And says it stopped short: what it shows is not the record whole.
        assert!(shown.refused.as_deref().is_some_and(|why| why.contains("line 5")));
        assert!(!shown.uuids.contains("u4") && !shown.uuids.contains("a5") && shown.uuids.contains("a3"), "{:?}", shown.uuids);
        // Followed, the reading stops before it: alive or ended, the same.
        for ended in [false, true] {
            let tail = follow::tail(&path, &follow::Seam::default(), &HashSet::new(), &|_| true, ended, &|_| false)
                .unwrap()
                .unwrap_or_else(|why| panic!("restarted: {why}"));
            assert_eq!(tail.end, before);
            assert!(tail.transcript.problems.iter().any(|problem| problem.contains("more than World reads")));
            assert!(!tail.transcript.uuids.contains("u4") && tail.transcript.uuids.contains("a3"));
            assert!(tail.transcript.refused.is_some(), "a reading short of the record says so");
        }
        // A tail passing large lines goes over it to the rows after it; one
        // that does not stops there.
        let mut kinds = Vec::new();
        crate::claude::tail::Tail::passing_large()
            .read(&path, |found| {
                if let crate::claude::tail::Found::Row(row) = found {
                    kinds.push(row["uuid"].as_str().unwrap_or_default().to_string());
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(kinds, ["u0", "a1", "u2", "a3", "a5"]);
        assert!(crate::claude::tail::Tail::default().read(&path, |_| Ok(())).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
