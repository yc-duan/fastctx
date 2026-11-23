//! Following a Claude Code record as it grows (sync-v2-impl-design §10).
//!
//! What was read through a seam is never parsed again: its bytes are hashed,
//! a MiB at a time with xxh3, and two blocks of them checked at each advance
//! (the last and one other; the rest by the background check). What lies
//! past the seam — the turn under way and whatever came since — is read,
//! read again to see that it stood still, and projected afresh, so an item
//! of it the host changes is a new revision of the same item. A seam moves
//! only to the start of a user's words with no tool call before it still
//! waiting for its result, and only where the caller accepts (`tail`):
//! before it nothing the record says later can change what it shows, short
//! of what refers back past a seam, which is read whole instead (`Restart`).
//!
//! A turn of hours has one such start, so what lies past the seam is the
//! whole turn, read and projected again at every advance. The record is
//! therefore also read through a place inside the turn (`Tail::read`): the
//! start of a later answer of the model's with no call waiting. What lies
//! between the seam and that place is not settled — whoever places it in
//! turns, names and orders it keeps it as open as ever — but it is not read
//! or projected again: a tail read on from the place (`TailStore::read_on`)
//! shows the rows past it as a reading from the seam does, and one that
//! refers back before the place (`refers_back`) is read from the seam.
//!
//! A tail read from the start is the whole record: the reading that cannot
//! take up where it was is this one, from the beginning.

use super::{Earlier, Held, Keep, Kept, Output, RowStore, Rows, SET_QUEUED, Scanned, project, project_asking, scan};
use crate::claude::subagents;
use crate::claude::titles::Titles;
use fastctx_platform::file_read;
use fastctx_types::file_identity::FileIdentity;
use fastctx_types::session::{HostAgentStart, SessionEvent};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use xxhash_rust::xxh3::{Xxh3, xxh3_64};

/// The prefix is hashed a block at a time.
const BLOCK: u64 = 1 << 20;
/// How many rows past a compaction one it names that never comes, or its
/// summary, is waited for before the record is read through past it: the
/// records looked at named none further than eleven.
const COMPACTION_SPAN: usize = 64;
/// How many times the user speaks past a tool call still waiting for its
/// answer before it no longer holds a seam back.
const ABANDONED_AFTER: usize = 2;

/// Where a record was read through, and what a reading that takes up there
/// needs of what came before.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Seam {
    /// The file read; none before the first reading.
    pub identity: Option<FileIdentity>,
    pub offset: u64,
    /// The lines before it.
    pub line: usize,
    /// The xxh3 of each MiB of the record before it, the last one short.
    pub blocks: Vec<u64>,
    pub titles: Titles,
    /// What could not be made out before it, the first `PROBLEMS_KEPT`
    /// word for word, and how many more.
    pub problems: Vec<String>,
    #[serde(default)]
    pub problems_omitted: usize,
    /// The agents started before it, for the ends told of them later: what
    /// an end needs, not what each was asked (CC19).
    pub agents: HashMap<String, HostAgentStart>,
    /// The latest time of a row before it.
    #[serde(default)]
    pub latest: Option<Timestamp>,
    /// The tool calls before it the user spoke past unanswered: an answer to
    /// one that comes after it reads the record whole (CC08).
    #[serde(default)]
    pub abandoned: Vec<String>,
    /// The agents the files beside the record told of that no call before
    /// it started, as a whole reading found them, each with the call its
    /// file named then (`id@call`): no reason to read whole again (CC10) —
    /// unless a file comes to name another call (EE02).
    #[serde(default)]
    pub unlinked: Vec<String>,
    /// Every agent the files beside the record told of at the reading that
    /// placed it, linked or not, each with the call its file named then
    /// (`id@call`): an agent linked before it whose file comes to name
    /// another call is linked again by a whole reading.
    #[serde(default)]
    pub told: Vec<String>,
}

/// Why a record is read whole rather than from its seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restart {
    /// Another file is at the path.
    Replaced,
    /// The file is shorter than the seam.
    Truncated,
    /// Bytes before the seam changed.
    Rewritten,
    /// The tail did not read the same twice.
    Unsteady,
    /// The tail refers back past the seam in a way it cannot show alone.
    Unsupported(&'static str),
}

impl std::fmt::Display for Restart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Replaced => f.write_str("another file is at the record's path"),
            Self::Truncated => f.write_str("the record is shorter than where it was read through"),
            Self::Rewritten => f.write_str("the record changed before where it was read through"),
            Self::Unsteady => f.write_str("the record's tail changed while it was read"),
            Self::Unsupported(why) => write!(f, "the record's tail holds {why}"),
        }
    }
}

/// What a reading past a seam found.
#[derive(Debug)]
pub struct Tail {
    /// What the rows past the seam show, with the title and problems of the
    /// whole record.
    pub transcript: super::Transcript,
    /// Where the record is read through now, when the seam moved.
    pub seam: Option<Seam>,
    /// Where the rows read whole end.
    pub end: u64,
    /// The time of the first row past the seam, if any has one.
    pub since: Option<Timestamp>,
    /// The time of the row the seam moved to, when it moved.
    pub seam_at: Option<Timestamp>,
    /// The record ids of the rows the seam moved past in this reading.
    pub prefix_ids: Vec<String>,
    /// A place inside what lies past the seam that the record is read
    /// through (`TailStore::read_past`): the start of the last row that
    /// begins the user's words or an answer of the model's with no tool call
    /// before it waiting, past every compaction and what it names. Not a
    /// seam: what lies before it is not settled, only not read again while
    /// nothing after it refers back to it (`TailStore::read_on`). None where
    /// none was asked for or none is past the seam.
    pub read: Option<Seam>,
}

/// Reads `path` past `seam`: the prefix checked, the tail read twice and
/// projected, `earlier` the record ids sessions before it hold. The seam
/// moves to the last start of the user's words past it that `accept` takes,
/// given the time they were said, with no tool call before it waiting.
/// `ended`: the session's process is gone, so a last row without its line's
/// end will never have one. `seen` says whether a record id is of a row
/// before the seam: a row of one again after it reads the record whole
/// (CC09).
pub fn tail(
    path: &Path,
    seam: &Seam,
    earlier: &HashSet<String>,
    accept: &dyn Fn(Timestamp) -> bool,
    ended: bool,
    seen: &dyn Fn(&str) -> bool,
) -> io::Result<Result<Tail, Restart>> {
    let mut file = file_read::open(path)?;
    let metadata = file.metadata()?;
    let identity = file_read::snapshot(&file, &metadata)?.identity();
    if seam.identity.as_ref().is_some_and(|known| *known != identity) {
        return Ok(Err(Restart::Replaced));
    }
    if metadata.len() < seam.offset {
        return Ok(Err(Restart::Truncated));
    }
    if !prefix_holds(&mut file, seam)? {
        return Ok(Err(Restart::Rewritten));
    }
    // The tail, read once.
    let mut first = Xxh3::new();
    let mut scanned = Scanned::new(seam.line, seam.offset, seam.titles.clone(), seam.problems.clone(), seam.problems_omitted);
    file.seek(SeekFrom::Start(seam.offset))?;
    scan(&mut BufReader::with_capacity(1 << 20, (&mut file).take(metadata.len() - seam.offset)), None, false, &mut scanned, Some(&mut first))?;
    let (_, end) = scanned.reached();
    if seam.offset > 0
        && let Some(why) = unsupported(&scanned.rows, seam, seen)?
    {
        return Ok(Err(Restart::Unsupported(why)));
    }
    // A whole reading takes a last row that lacks only its line's end, once
    // nothing will write the rest (CC17). A line larger than World reads
    // ends the reading here as there, the rows before it read (§28.49).
    if ended && end < metadata.len() && scanned.refused().is_none() {
        return Ok(Err(Restart::Unsupported("a last row without its line's end")));
    }
    let (since, moved, _) = next_seam(&scanned.rows, seam, accept)?;
    // Read again: the same bytes, the same file at the path, no shorter.
    if !reads_again(&mut file, seam.offset, end, first.digest())? {
        return Ok(Err(Restart::Unsteady));
    }
    let now = file_read::open(path)?;
    let still = file_read::snapshot(&now, &now.metadata()?)?;
    if still.identity() != identity || still.size < end {
        return Ok(Err(Restart::Unsteady));
    }
    let beside = subagents::beside(path);
    let seam_at = moved.as_ref().map(|(_, said, _, _)| *said);
    let prefix_ids: Vec<String> = match &moved {
        Some((to, _, _, _)) => scanned
            .rows()
            .iter()
            .filter(|row| row.start < *to)
            .filter_map(|row| row.value.get("uuid").and_then(Value::as_str).map(str::to_string))
            .collect(),
        None => Vec::new(),
    };
    let next = match moved {
        Some((to, _, latest, abandoned)) => {
            let mut next = advanced(&mut file, seam, to, identity, &|uuid| Ok(earlier.contains(uuid)), &beside, Held::default())?;
            next.latest = latest;
            next.abandoned = seam.abandoned.iter().cloned().chain(abandoned).collect();
            // What the seam is placed by is what was read and is shown:
            // read once more, the tail is still the bytes the first reading
            // took (CC11).
            if !reads_again(&mut file, seam.offset, end, first.digest())? {
                return Ok(Err(Restart::Unsteady));
            }
            Some(next)
        }
        None if seam.identity.is_none() => Some(Seam { identity: Some(identity), ..seam.clone() }),
        None => None,
    };
    let told: Vec<(String, String)> = beside.calls().map(|(id, call)| (id.to_string(), call.to_string())).collect();
    let pairs: Vec<String> = told.iter().map(|(_, call)| beside.told_of(call)).collect();
    if seam.offset > 0 && told.iter().zip(&pairs).any(|((id, call), pair)| seam.agents.contains_key(id) && !told_before(seam, id, call, pair)) {
        return Ok(Err(Restart::Unsupported("an agent before the seam told of otherwise")));
    }
    let transcript = project(scanned, &[], earlier, beside, seam.agents.clone(), super::Output::Context)?;
    // An agent the files beside tell of that no call started — before the
    // seam or past it — is one a call before the seam started, its
    // telling come late: a whole reading links it (CC10). One whose file
    // names another call than when a whole reading found it unlinked may
    // link now (EE02).
    let unlinked: Vec<String> = told
        .into_iter()
        .filter(|(id, _)| !seam.agents.contains_key(id) && !transcript.agents().contains_key(id))
        .map(|(id, call)| format!("{id}@{call}"))
        .collect();
    if seam.offset > 0 && unlinked.iter().any(|id| !seam.unlinked.contains(id)) {
        return Ok(Err(Restart::Unsupported("an agent before the seam told of late")));
    }
    let next = next.map(|mut next| {
        next.unlinked = if seam.offset == 0 { unlinked } else { seam.unlinked.clone() };
        next.told = pairs;
        next
    });
    Ok(Ok(Tail { transcript, seam: next, end, since, seam_at, prefix_ids, read: None }))
}

/// Where a tail read into a store (`tail_kept`, design §28.51 ⑤-5) keeps
/// its rows and gives what it found: the rows kept in `rows`; the events
/// handed to `events` as they are made, none held; the record id of each
/// row the seam moved past handed to `moved_past` (not `Tail::prefix_ids`);
/// of `asked`, those the record holds — among its rows or the queued
/// messages' sources it names — told in the transcript's `uuids`, which
/// holds no others.
pub struct TailStore<'k> {
    pub rows: &'k mut dyn RowStore,
    pub events: &'k mut dyn FnMut(SessionEvent) -> io::Result<()>,
    pub moved_past: &'k mut dyn FnMut(&str) -> io::Result<()>,
    pub asked: &'k [String],
    /// The seam read from is a place read through inside a turn (`Tail::read`
    /// of an earlier reading), not the settled one: which of some keys
    /// (`KEY_ROW` and the like) the rows between the settled seam and it
    /// hold, asked a batch at a time. A tail that refers back to one of
    /// them is not read on from there (`Restart::Unsupported`), and the
    /// seam it gives is the place read through next.
    pub read_on: Option<Before<'k>>,
    /// Handed the keys of the rows a place read through moves past — from
    /// the settled seam to `Tail::read`, or, read on from one (`read_on`),
    /// from it to the next — as they are found, before the tail is known
    /// good: kept only when it is. None where no place read through is
    /// wanted.
    pub read_past: Option<Leaves<'k>>,
}

/// What is handed each key a row leaves (`TailStore::read_past`).
pub type Leaves<'k> = &'k mut dyn FnMut(&str) -> io::Result<()>;

/// Which of some keys the rows before a place read through hold (`TailStore`).
pub type Before<'h> = &'h dyn Fn(&[String]) -> io::Result<HashSet<String>>;

/// The tool blocks noted of each tool use (`RowStore::note_block`): whether
/// a result, the row, the place in it.
type NotedBlocks = HashMap<String, Vec<(bool, usize, usize)>>;

/// The rows of a short tail held in memory (`RowStore`): what a record read
/// on from a place inside a turn reads at each look, a few rows, with no
/// work store begun for them.
#[derive(Default)]
pub struct HeldRows {
    rows: Vec<(usize, u64, Vec<u8>)>,
    by_uuid: HashMap<String, usize>,
    blocks: std::cell::RefCell<NotedBlocks>,
    members: std::cell::RefCell<HashSet<(u8, String)>>,
}

impl RowStore for HeldRows {
    fn count(&self) -> usize {
        self.rows.len()
    }

    fn push(&mut self, uuid: Option<&str>, line: usize, start: u64, bytes: &[u8]) -> io::Result<()> {
        if let Some(uuid) = uuid {
            self.by_uuid.insert(uuid.to_owned(), self.rows.len());
        }
        self.rows.push((line, start, bytes.to_vec()));
        Ok(())
    }

    fn replace(&mut self, index: usize, line: usize, start: u64, bytes: &[u8]) -> io::Result<()> {
        let row = self.rows.get_mut(index).ok_or_else(|| io::Error::other("no such row held"))?;
        *row = (line, start, bytes.to_vec());
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn note_block(&self, id: &str, result: bool, row: usize, index: usize) -> io::Result<()> {
        // No more of a side than `blocks` gives: a tool use named in very
        // many rows is not gone through for each of them.
        let mut blocks = self.blocks.borrow_mut();
        let noted = blocks.entry(id.to_owned()).or_default();
        if noted.iter().filter(|(of, ..)| *of == result).count() < 2 {
            noted.push((result, row, index));
        }
        Ok(())
    }

    fn blocks(&self, id: &str) -> io::Result<Vec<(bool, usize, usize)>> {
        let blocks = self.blocks.borrow();
        let noted = blocks.get(id).map(Vec::as_slice).unwrap_or_default();
        let side = |result: bool| noted.iter().filter(move |(of, ..)| *of == result).take(2).copied();
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
        self.rows.get(index).cloned().ok_or_else(|| io::Error::other("no such row held"))
    }

    fn each(&self, visit: super::VisitLine<'_>) -> io::Result<()> {
        self.rows.iter().enumerate().try_for_each(|(index, (line, start, bytes))| visit(index, *line, *start, bytes))
    }

    fn ids_before(&self, to: u64, each: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        let mut ids: Vec<(usize, &String)> = self.by_uuid.iter().filter(|(_, index)| self.rows[**index].1 < to).map(|(uuid, index)| (*index, uuid)).collect();
        ids.sort_unstable();
        ids.into_iter().try_for_each(|(_, uuid)| each(uuid))
    }

    fn clear(&mut self) -> io::Result<()> {
        *self = HeldRows::default();
        Ok(())
    }
}

/// What a row leaves that a row after it may refer back to, each a key: its
/// record id, the tool calls it makes, the calls it answers, the record ids
/// it takes back, and the record id an answer of it names as its call's.
pub const KEY_ROW: &str = "u:";
pub const KEY_CALL: &str = "c:";
pub const KEY_ANSWER: &str = "a:";
pub const KEY_TAKEN_BACK: &str = "x:";
pub const KEY_SOURCE: &str = "s:";
/// And the record ids it names that no row before the place bears: its
/// parent, where it is a row a compaction's summary is found through (the
/// summary, or what Claude Code writes between it and its compaction); and
/// what a compaction says it kept.
pub const KEY_PARENT: &str = "n:";

/// The record ids `row` takes back (`project_rows`, `removed`).
fn taken_back(row: &super::Row) -> impl Iterator<Item = &str> {
    let references = match row.kind() {
        Some("assistant") => row.value.get("supersedesUuids"),
        Some("system") if row.value["subtype"] == "model_refusal_fallback" => row.value.get("retractedMessageUuids"),
        _ => None,
    };
    references.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
}

/// The tool calls `row` makes and those it answers, by id.
fn calls_of(row: &super::Row) -> impl Iterator<Item = (bool, &str)> {
    let assistant = row.kind() == Some("assistant");
    let user = row.kind() == Some("user");
    row.blocks().into_iter().flatten().filter_map(move |block| match block.get("type").and_then(Value::as_str) {
        Some("tool_use") if assistant => block.get("id").and_then(Value::as_str).map(|id| (false, id)),
        Some("tool_result") if user => block.get("tool_use_id").and_then(Value::as_str).map(|id| (true, id)),
        _ => None,
    })
}

/// The keys `row` leaves for the rows after it (`KEY_ROW`); `before` says
/// whether a record id is of a row before the place the keys are left for.
fn keys_left(row: &super::Row, before: &dyn Fn(&str) -> io::Result<bool>, each: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()> {
    if let Some(uuid) = row.value.get("uuid").and_then(Value::as_str) {
        each(&format!("{KEY_ROW}{uuid}"))?;
    }
    // A summary is found from its compaction along its parents, through
    // what is no message (`summarized`): a parent not written yet, once it
    // comes, joins what stands before the place.
    let message = matches!(row.kind(), Some("user" | "assistant")) && row.value["isCompactSummary"] != true;
    if !message
        && let Some(parent) = row.value.get("parentUuid").and_then(Value::as_str)
        && !before(parent)?
    {
        each(&format!("{KEY_PARENT}{parent}"))?;
    }
    // And what a compaction names of what it kept: one of them written
    // later changes what the compaction is told to hold.
    if row.compaction() {
        let metadata = &row.value["compactMetadata"];
        let named = [
            &row.value["logicalParentUuid"],
            &metadata["preservedSegment"]["headUuid"],
            &metadata["preservedSegment"]["tailUuid"],
            &metadata["preservedSegment"]["anchorUuid"],
            &metadata["preservedMessages"]["anchorUuid"],
        ];
        let kept = metadata["preservedMessages"]["uuids"].as_array().into_iter().flatten();
        for uuid in named.into_iter().chain(kept).filter_map(Value::as_str) {
            if !before(uuid)? {
                each(&format!("{KEY_PARENT}{uuid}"))?;
            }
        }
    }
    for (answer, id) in calls_of(row) {
        each(&format!("{}{id}", if answer { KEY_ANSWER } else { KEY_CALL }))?;
    }
    for target in taken_back(row) {
        each(&format!("{KEY_TAKEN_BACK}{target}"))?;
    }
    if let Some(source) = row.value.get("sourceToolAssistantUUID").and_then(Value::as_str) {
        each(&format!("{KEY_SOURCE}{source}"))?;
    }
    Ok(())
}

/// What in a tail read on from a place inside a turn refers back to the
/// rows between the settled seam and that place, which the tail cannot show
/// alone: a tool call of theirs made or answered again, a row they took
/// back before it was written, a row an answer of theirs named as its
/// call's before it was there, an answer naming one of them as its call's.
/// A whole reading pairs calls and answers, takes back and judges an
/// answer's source over all the rows; none of these is known past the place.
fn refers_back<L: Rows>(rows: &L, held: Before<'_>) -> io::Result<Option<&'static str>> {
    let mut asked: Vec<(String, &'static str)> = Vec::new();
    rows.each(&mut |_, row| {
        for (_, id) in calls_of(row) {
            asked.push((format!("{KEY_CALL}{id}"), "a tool call made before where it was read through"));
            asked.push((format!("{KEY_ANSWER}{id}"), "a tool call answered before where it was read through"));
        }
        if let Some(uuid) = row.value.get("uuid").and_then(Value::as_str) {
            asked.push((format!("{KEY_TAKEN_BACK}{uuid}"), "a row taken back before it was written"));
            asked.push((format!("{KEY_SOURCE}{uuid}"), "a row an answer named as its call's before it was written"));
            asked.push((format!("{KEY_PARENT}{uuid}"), "a row a compaction before where it was read through names"));
        }
        if let Some(source) = row.value.get("sourceToolAssistantUUID").and_then(Value::as_str) {
            asked.push((format!("{KEY_ROW}{source}"), "an answer naming a row before where it was read through as its call's"));
            // An answer to a call of a row taken back is not shown.
            asked.push((format!("{KEY_TAKEN_BACK}{source}"), "an answer to a call of a row taken back before it was written"));
        }
        Ok(())
    })?;
    let mut found = HashSet::new();
    for batch in asked.chunks(SEEN_AT_ONCE) {
        let keys: Vec<String> = batch.iter().map(|(key, _)| key.clone()).collect();
        found.extend(held(&keys)?);
    }
    Ok(asked.into_iter().find(|(key, _)| found.contains(key)).map(|(_, why)| why))
}

/// Which of some record ids are of rows before a seam (`tail_kept`), asked
/// a batch at a time: those that are.
pub type Seen<'s> = &'s dyn Fn(&[String]) -> io::Result<HashSet<String>>;

/// How many record ids past a seam are asked of at once (`Seen`).
const SEEN_AT_ONCE: usize = 4096;

/// As `tail`, the rows past `seam` kept in `kept.rows` rather than held,
/// read back as they are asked for, and the events handed on as they are
/// made (`Output::History`); `earlier` asked of each record id, and `seen`
/// of the record ids past the seam a batch at a time. The same checks, the
/// same seam, the same events. All that is asked of the rows past the seam
/// is asked before the seam's own rows are read again into the store, which
/// lets the others go (`advanced`); the checks after it are made in the
/// order `tail` makes them.
pub fn tail_kept(
    path: &Path,
    seam: &Seam,
    earlier: Earlier<'_>,
    accept: &dyn Fn(Timestamp) -> bool,
    ended: bool,
    seen: Seen<'_>,
    kept: TailStore<'_>,
) -> io::Result<Result<Tail, Restart>> {
    let TailStore { rows: store, events, moved_past, asked, read_on, read_past } = kept;
    let mut file = file_read::open(path)?;
    let metadata = file.metadata()?;
    let identity = file_read::snapshot(&file, &metadata)?.identity();
    if seam.identity.as_ref().is_some_and(|known| *known != identity) {
        return Ok(Err(Restart::Replaced));
    }
    if metadata.len() < seam.offset {
        return Ok(Err(Restart::Truncated));
    }
    if !prefix_holds(&mut file, seam)? {
        return Ok(Err(Restart::Rewritten));
    }
    // The tail, read once, into the store.
    let mut first = Xxh3::new();
    store.clear()?;
    let mut scanned = Scanned::with(Kept::new(&mut *store), seam.line, seam.offset, seam.titles.clone(), seam.problems.clone(), seam.problems_omitted);
    file.seek(SeekFrom::Start(seam.offset))?;
    scan(&mut BufReader::with_capacity(1 << 20, (&mut file).take(metadata.len() - seam.offset)), None, false, &mut scanned, Some(&mut first))?;
    scanned.rows.scanned()?;
    let (_, end) = scanned.reached();
    if seam.offset > 0 {
        // The record ids past the seam asked a batch at a time whether they
        // are of rows before it; looked up of those that are as each row is
        // judged, in its order (`unsupported`).
        let mut before = HashSet::new();
        let mut batch = Vec::with_capacity(SEEN_AT_ONCE);
        let mut ask = |batch: &mut Vec<String>| -> io::Result<()> {
            before.extend(seen(batch)?);
            batch.clear();
            Ok(())
        };
        let rows = scanned.rows.blocks_store().ok_or_else(|| io::Error::other("rows kept with no store"))?;
        rows.ids_before(u64::MAX, &mut |uuid| {
            batch.push(uuid.to_string());
            if batch.len() >= SEEN_AT_ONCE { ask(&mut batch) } else { Ok(()) }
        })?;
        ask(&mut batch)?;
        if let Some(why) = unsupported(&scanned.rows, seam, &|uuid| before.contains(uuid))? {
            return Ok(Err(Restart::Unsupported(why)));
        }
        if let Some(held) = read_on
            && let Some(why) = refers_back(&scanned.rows, held)?
        {
            return Ok(Err(Restart::Unsupported(why)));
        }
    }
    if ended && end < metadata.len() && scanned.refused().is_none() {
        return Ok(Err(Restart::Unsupported("a last row without its line's end")));
    }
    let (since, settled, inner) = next_seam(&scanned.rows, seam, accept)?;
    // Read on from a place inside a turn, the seam it gives is the next
    // such place; else the settled one, with a place read through past it
    // where one is wanted.
    let (moved, read) = match (read_on.is_some(), read_past.is_some()) {
        (true, _) => (inner, None),
        (false, true) => {
            let from = settled.as_ref().map_or(seam.offset, |(to, ..)| *to);
            (settled, inner.filter(|(to, ..)| *to > from))
        }
        (false, false) => (settled, None),
    };
    if !reads_again(&mut file, seam.offset, end, first.digest())? {
        return Ok(Err(Restart::Unsteady));
    }
    let now = file_read::open(path)?;
    let still = file_read::snapshot(&now, &now.metadata()?)?;
    if still.identity() != identity || still.size < end {
        return Ok(Err(Restart::Unsteady));
    }
    // What the rows a place read through moves past leave, while the rows
    // are at hand.
    if let Some(read_past) = read_past {
        let span = match (read_on.is_some(), &moved, &read) {
            (true, Some((to, ..)), _) => Some((seam.offset, *to)),
            (false, moved, Some((to, ..))) => Some((moved.as_ref().map_or(seam.offset, |(from, ..)| *from), *to)),
            _ => None,
        };
        if let Some((from, to)) = span {
            // Of a row before the place: among the rows read, starting
            // before it.
            let before = |uuid: &str| -> io::Result<bool> {
                match scanned.rows.index_of(uuid)? {
                    Some(index) => Ok(scanned.rows.row(index)?.start < to),
                    None => Ok(false),
                }
            };
            scanned.rows.each(&mut |_, row| if row.start >= from && row.start < to { keys_left(row, &before, read_past) } else { Ok(()) })?;
        }
    }
    let beside = subagents::beside(path);
    let seam_at = moved.as_ref().map(|(_, said, _, _)| *said);
    let mut transcript = project_asking(scanned, &[], earlier, beside.clone(), seam.agents.clone(), Output::History(events))?;
    // A reading from the seam tells what it could not make out in the order
    // of its passes over all the rows, not in theirs: read on from a place
    // inside a turn, a tail that adds to it is not told as that reading
    // tells it.
    if read_on.is_some() && transcript.problems.len() + transcript.problems_omitted != seam.problems.len() + seam.problems_omitted {
        return Ok(Err(Restart::Unsupported("a row that could not be made out")));
    }
    // Asked of the rows past the seam while the store holds them.
    if let Some((to, ..)) = &moved
        && read_on.is_none()
    {
        store.ids_before(*to, moved_past)?;
    }
    for uuid in asked {
        if transcript.uuids.contains(uuid) || store.index_of(uuid)?.is_some() || store.is_member(SET_QUEUED, uuid)? {
            transcript.uuids.insert(uuid.clone());
        }
    }
    transcript.uuids.retain(|uuid| asked.contains(uuid));
    let next = match moved {
        Some((to, _, latest, abandoned)) => {
            store.clear()?;
            let mut next = advanced(&mut file, seam, to, identity.clone(), earlier, &beside, Kept::new(&mut *store))?;
            next.latest = latest;
            next.abandoned = seam.abandoned.iter().cloned().chain(abandoned).collect();
            if !reads_again(&mut file, seam.offset, end, first.digest())? {
                return Ok(Err(Restart::Unsteady));
            }
            Some(next)
        }
        None if seam.identity.is_none() => Some(Seam { identity: Some(identity.clone()), ..seam.clone() }),
        None => None,
    };
    // The place read through, of the rows from the seam to it as the seam
    // itself is made of those it moves past.
    let read = match read {
        Some((to, _, latest, abandoned)) => {
            store.clear()?;
            let mut read = advanced(&mut file, seam, to, identity, earlier, &beside, Kept::new(&mut *store))?;
            read.latest = latest;
            read.abandoned = seam.abandoned.iter().cloned().chain(abandoned).collect();
            if !reads_again(&mut file, seam.offset, end, first.digest())? {
                return Ok(Err(Restart::Unsteady));
            }
            Some(read)
        }
        None => None,
    };
    let told: Vec<(String, String)> = beside.calls().map(|(id, call)| (id.to_string(), call.to_string())).collect();
    let pairs: Vec<String> = told.iter().map(|(_, call)| beside.told_of(call)).collect();
    if seam.offset > 0 && told.iter().zip(&pairs).any(|((id, call), pair)| seam.agents.contains_key(id) && !told_before(seam, id, call, pair)) {
        return Ok(Err(Restart::Unsupported("an agent before the seam told of otherwise")));
    }
    // Read on from a place inside a turn, the files beside the record are
    // to tell just what they told as the place was read through: one come,
    // gone or changed since may tell otherwise of a row before the place —
    // an agent's start there, a notice of one not known then — which a
    // reading from the seam shows.
    if read_on.is_some() {
        let (mut now, mut then) = (pairs.clone(), seam.told.clone());
        now.sort_unstable();
        then.sort_unstable();
        if now != then {
            return Ok(Err(Restart::Unsupported("the agents' files beside the record telling otherwise")));
        }
    }
    let unlinked: Vec<String> = told
        .into_iter()
        .filter(|(id, _)| !seam.agents.contains_key(id) && !transcript.agents().contains_key(id))
        .map(|(id, call)| format!("{id}@{call}"))
        .collect();
    if seam.offset > 0 && unlinked.iter().any(|id| !seam.unlinked.contains(id)) {
        return Ok(Err(Restart::Unsupported("an agent before the seam told of late")));
    }
    let finish = |mut next: Seam| {
        next.unlinked = if seam.offset == 0 { unlinked.clone() } else { seam.unlinked.clone() };
        next.told = pairs.clone();
        next
    };
    let (next, read) = (next.map(&finish), read.map(&finish));
    Ok(Ok(Tail { transcript, seam: next, end, since, seam_at, prefix_ids: Vec::new(), read }))
}

/// Whether the file beside the record told of the agent `id`, of the call
/// `call`, at the reading that placed `seam` as it tells of it now (`told`,
/// `Beside::told_of`): the same call, and of what the file says of the
/// agent — its kind, its task, whether it works on in the background — the
/// same. A seam placed before these were kept tells the call alone, and is
/// taken at its word until it is placed again.
fn told_before(seam: &Seam, id: &str, call: &str, told: &str) -> bool {
    seam.told.iter().any(|before| before == told) || seam.told.iter().any(|before| before.len() == id.len() + 1 + call.len() && before.strip_prefix(id).and_then(|rest| rest.strip_prefix('@')) == Some(call))
}

/// Whether `[from, to)` of `file` hashes to `expected` read again.
fn reads_again(file: &mut File, from: u64, to: u64, expected: u64) -> io::Result<bool> {
    let mut again = Xxh3::new();
    file.seek(SeekFrom::Start(from))?;
    let mut window = vec![0u8; 1 << 20];
    let mut remaining = to - from;
    while remaining > 0 {
        let want = remaining.min(window.len() as u64) as usize;
        file.read_exact(&mut window[..want])?;
        again.update(&window[..want]);
        remaining -= want as u64;
    }
    Ok(again.digest() == expected)
}

/// The xxh3 of each MiB of `[0, to)`, those wholly before `seam` kept from it.
fn blocks(file: &mut File, seam: &Seam, to: u64) -> io::Result<Vec<u64>> {
    let kept = (seam.offset / BLOCK) as usize;
    let mut blocks = seam.blocks[..kept.min(seam.blocks.len())].to_vec();
    let mut bytes = vec![0u8; BLOCK as usize];
    let mut start = kept as u64 * BLOCK;
    file.seek(SeekFrom::Start(start))?;
    while start < to {
        let length = (to - start).min(BLOCK) as usize;
        file.read_exact(&mut bytes[..length])?;
        blocks.push(xxh3_64(&bytes[..length]));
        start += length as u64;
    }
    Ok(blocks)
}

/// Where a seam moves to (`next_seam`): the start of the row, its time, the
/// latest time before it and the calls spoken past unanswered before it.
type Moved = (u64, Timestamp, Option<Timestamp>, Vec<String>);

/// Where the seam goes next: the start of the last row past it that starts
/// the user's words, its time accepted and later than every row before it
/// (CC13: events at one moment are named by their order there, so none are
/// split across it), with every tool call before it answered (`Moved`).
/// With it, the time of the first row that has one; and where the record
/// may be read through inside a turn (`Tail::read`): the start of the last
/// row that starts the user's words, whatever its time is accepted for, or
/// an answer of the model's — a message of another id than the answer row
/// before it — as late and with as little waiting, past any compaction
/// with the rows it names and its summary. The rows taken once in their
/// order.
fn next_seam<L: Rows>(rows: &L, seam: &Seam, accept: &dyn Fn(Timestamp) -> bool) -> io::Result<(Option<Timestamp>, Option<Moved>, Option<Moved>)> {
    // Each call still waiting, with how many times the user spoke since.
    let mut waiting: HashMap<String, usize> = HashMap::new();
    // Where it would go, with how many calls were spoken past by then: the
    // calls only ever grow, so those before it are the first so many.
    type Found = Option<(u64, Timestamp, Option<Timestamp>, usize)>;
    let (mut found, mut inner, mut last_start): (Found, Found, Option<u64>) = (None, None, None);
    let mut latest = seam.latest;
    let mut abandoned: Vec<String> = Vec::new();
    let mut since = None;
    // The message the answer row before was of.
    let mut answering: Option<String> = None;
    // A compaction not yet whole: the number of the last row it names, its
    // own, and whether its summary came.
    let mut compacting: Option<(usize, usize, bool)> = None;
    rows.each(&mut |index, row| {
        // A row a later one of its UUID replaced stands out of file order:
        // no seam is placed by it.
        let in_order = last_start.is_none_or(|last| row.start > last);
        last_start = Some(last_start.map_or(row.start, |last| last.max(row.start)));
        // Nor one with a row after it in the record's order that stands
        // before it in the file: the bytes before such a seam would not be
        // the rows before it (EE03).
        if found.as_ref().is_some_and(|(to, ..)| row.start < *to) {
            found = None;
        }
        if inner.as_ref().is_some_and(|(to, ..)| row.start < *to) {
            inner = None;
        }
        let prompt = row.kind() == Some("user") && row.prompt();
        if prompt {
            // A call the user spoke past twice without its answer is one an
            // interruption left unanswered: it holds no seam back. An answer
            // that still comes after the seam is set right by the
            // background check (§10, bounded eventual sameness).
            waiting.retain(|id, spoken| {
                *spoken += 1;
                let holds = *spoken < ABANDONED_AFTER;
                if !holds {
                    abandoned.push(id.to_string());
                }
                holds
            });
        }
        // An answer of another message than the answer row before it was of.
        let answer = row.kind() == Some("assistant") && {
            let message = row.value.pointer("/message/id").and_then(Value::as_str).filter(|id| !id.is_empty());
            let another = message.zip(answering.as_deref()).is_some_and(|(id, before)| id != before);
            answering = message.map(str::to_owned);
            another
        };
        if row.compaction() {
            let metadata = &row.value["compactMetadata"];
            let named = [
                &row.value["logicalParentUuid"],
                &metadata["preservedSegment"]["headUuid"],
                &metadata["preservedSegment"]["tailUuid"],
                &metadata["preservedSegment"]["anchorUuid"],
                &metadata["preservedMessages"]["anchorUuid"],
            ];
            let kept = metadata["preservedMessages"]["uuids"].as_array().into_iter().flatten();
            let mut last = index;
            for uuid in named.into_iter().chain(kept).filter_map(Value::as_str) {
                // One it names that is not among the rows will not come
                // (the rows hold all that was written): waited for no
                // longer than any other is far.
                last = last.max(rows.index_of(uuid)?.unwrap_or(index + COMPACTION_SPAN));
            }
            compacting = Some((last, index, false));
        } else if row.value["isCompactSummary"] == true
            && let Some((_, _, summarized)) = compacting.as_mut()
        {
            *summarized = true;
        }
        let whole = compacting.is_none_or(|(last, own, summarized)| index > last && (summarized || index > own + COMPACTION_SPAN));
        if whole {
            compacting = None;
        }
        let time = at(&row.value);
        if since.is_none() {
            since = time;
        }
        // A row after a place read through whose time is no later than the
        // latest before the place would stand, by its time, among what the
        // place leaves unread: no such place is given.
        if let (Some((_, _, Some(before), _)), Some(time)) = (&inner, time)
            && time <= *before
        {
            inner = None;
        }
        if in_order
            && waiting.is_empty()
            && row.start > seam.offset
            && (prompt || answer)
            && let Some(said) = time.filter(|said| latest.is_none_or(|latest| *said > latest))
        {
            if prompt && accept(said) {
                found = Some((row.start, said, latest, abandoned.len()));
            }
            if whole {
                inner = Some((row.start, said, latest, abandoned.len()));
            }
        }
        if let Some(time) = time {
            latest = Some(latest.map_or(time, |latest| latest.max(time)));
        }
        for block in row.blocks().into_iter().flatten() {
            match (row.kind(), block.get("type").and_then(Value::as_str)) {
                (Some("assistant"), Some("tool_use")) => {
                    // One with no id is paired with no answer (`Pairs`):
                    // none is waited for.
                    if let Some(id) = block.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                        waiting.insert(id.to_string(), 0);
                    }
                }
                (Some("user"), Some("tool_result")) => {
                    if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                        waiting.remove(id);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })?;
    let moved = |found: Found| {
        found.map(|(to, said, latest, spoken)| (to, said, latest, abandoned[..spoken.min(abandoned.len())].to_vec()))
    };
    Ok((since, moved(found), moved(inner)))
}

/// The seam at `to`: what the rows between `seam` and it leave for a reading
/// that takes up there, read again into `rows`.
#[allow(clippy::too_many_arguments)]
fn advanced<L: Keep>(
    file: &mut File,
    seam: &Seam,
    to: u64,
    identity: FileIdentity,
    earlier: Earlier<'_>,
    beside: &subagents::Beside,
    rows: L,
) -> io::Result<Seam> {
    let blocks = blocks(file, seam, to)?;
    let mut scanned = Scanned::with(rows, seam.line, seam.offset, seam.titles.clone(), seam.problems.clone(), seam.problems_omitted);
    file.seek(SeekFrom::Start(seam.offset))?;
    scan(&mut BufReader::new(file.take(to - seam.offset)), None, false, &mut scanned, None)?;
    scanned.rows.scanned()?;
    let (line, offset) = scanned.reached();
    if offset != to {
        return Err(io::Error::other("the record's rows before the new seam did not read whole"));
    }
    let titles = scanned.titles().clone();
    // Only what the seam keeps of the rows before it: none of their events.
    let between = project_asking(scanned, &[], earlier, beside.clone(), seam.agents.clone(), Output::SeamOnly)?;
    Ok(Seam {
        identity: Some(identity),
        offset,
        line,
        blocks,
        titles,
        problems: between.problems.clone(),
        problems_omitted: between.problems_omitted,
        agents: between
            .agents()
            .iter()
            .map(|(id, start)| (id.clone(), HostAgentStart { prompt: None, ..start.clone() }))
            .collect(),
        latest: None,
        abandoned: Vec::new(),
        unlinked: Vec::new(),
        told: Vec::new(),
    })
}

/// What in a tail refers back past its seam in a way the tail cannot show
/// alone: a compaction naming rows before the seam (`names_before`), a row
/// that takes back rows not in the tail, and the answer to a call before
/// the seam the user had spoken past.
fn unsupported<L: Rows>(rows: &L, seam: &Seam, seen: &dyn Fn(&str) -> bool) -> io::Result<Option<&'static str>> {
    // Looked up, not walked, for each answer.
    let abandoned: HashSet<&str> = seam.abandoned.iter().map(String::as_str).collect();
    let mut why = None;
    rows.each(&mut |_, row| {
        if why.is_none() {
            why = unsupported_row(rows, row, &abandoned, seen)?;
        }
        Ok(())
    })?;
    Ok(why)
}

/// What in `row` of `rows` refers back past the seam (`unsupported`), the
/// calls before it the user spoke past `abandoned`.
fn unsupported_row<L: Rows>(rows: &L, row: &super::Row, abandoned: &HashSet<&str>, seen: &dyn Fn(&str) -> bool) -> io::Result<Option<&'static str>> {
    if row.value.get("uuid").and_then(Value::as_str).is_some_and(seen) {
        return Ok(Some("a row of a record before the seam again"));
    }
    if row.kind() == Some("user")
        && !abandoned.is_empty()
        && row.blocks().into_iter().flatten().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_result")
                && block.get("tool_use_id").and_then(Value::as_str).is_some_and(|id| abandoned.contains(id))
        })
    {
        return Ok(Some("the answer to a call before the seam"));
    }
    if (row.compaction() || row.value["isCompactSummary"] == true) && names_before(rows, row)? {
        return Ok(Some("a compaction naming rows before the seam"));
    }
    let references = match row.kind() {
        Some("assistant") => row.value.get("supersedesUuids"),
        Some("system") if row.value["subtype"] == "model_refusal_fallback" => row.value.get("retractedMessageUuids"),
        _ => None,
    };
    if let Some(ids) = references.and_then(Value::as_array) {
        for id in ids.iter().filter_map(Value::as_str) {
            if rows.index_of(id)?.is_none() {
                return Ok(Some("a row taking back rows before the seam"));
            }
        }
    }
    Ok(None)
}

/// Whether the compaction `row` begins, or the summary it is, is shown of a
/// row before the seam, `rows` being those past it. A compaction is shown
/// of what it kept and the message it went on from, all written before it:
/// one not among `rows` lies before the seam. Its anchor, the summary that
/// follows it, may not be written yet, as a whole reading may not find it
/// either. A summary is shown with the compaction found along its parents
/// past what is no message (`summarized`): a parent not among `rows` lies
/// before the seam. Past it all, the tail shows the compaction as a whole
/// reading does — Claude Code keeps the last few messages before it, which
/// a long turn holds (2.1.322).
fn names_before<L: Rows>(rows: &L, row: &super::Row) -> io::Result<bool> {
    if row.compaction() {
        let metadata = &row.value["compactMetadata"];
        let before = [&row.value["logicalParentUuid"], &metadata["preservedSegment"]["headUuid"], &metadata["preservedSegment"]["tailUuid"]];
        let kept = metadata["preservedMessages"]["uuids"].as_array().into_iter().flatten();
        for uuid in before.into_iter().chain(kept).filter_map(Value::as_str) {
            if rows.index_of(uuid)?.is_none() {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let mut parent = row.value.get("parentUuid").and_then(Value::as_str).map(str::to_owned);
    for _ in 0..32 {
        let Some(above) = parent else { return Ok(false) };
        let Some(index) = rows.index_of(&above)? else { return Ok(true) };
        let above = rows.row(index)?;
        if above.compaction() || matches!(above.kind(), Some("user" | "assistant")) {
            return Ok(false);
        }
        parent = above.value.get("parentUuid").and_then(Value::as_str).map(str::to_owned);
    }
    Ok(false)
}

fn at(row: &Value) -> Option<Timestamp> {
    row.get("timestamp").and_then(Value::as_str).and_then(|text| text.parse().ok())
}

/// Whether the record at `path` is still the one read through `seam`: the
/// same file, no shorter, and every block before the seam the bytes hashed
/// as it was read — the whole prefix, as an advance checks two blocks of it
/// (the background check §10 leaves the rest to; design §28.53). Hashed a
/// block at a time, nothing parsed; given up, interrupted, as soon as `stop`
/// says so (a check ended for taking too long).
pub fn prefix_unchanged(path: &Path, seam: &Seam, stop: &dyn Fn() -> bool) -> io::Result<bool> {
    let Some(known) = &seam.identity else { return Ok(false) };
    let mut file = file_read::open(path)?;
    let metadata = file.metadata()?;
    let snapshot = file_read::snapshot(&file, &metadata)?;
    if snapshot.identity() != *known || snapshot.size < seam.offset {
        return Ok(false);
    }
    let count = seam.offset.div_ceil(BLOCK) as usize;
    if seam.blocks.len() != count {
        return Ok(false);
    }
    let mut bytes = vec![0u8; BLOCK as usize];
    for (index, block) in seam.blocks.iter().enumerate() {
        if stop() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "the check was stopped"));
        }
        let start = index as u64 * BLOCK;
        let length = (seam.offset - start).min(BLOCK) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut bytes[..length])?;
        if xxh3_64(&bytes[..length]) != *block {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether the bytes before the seam are the ones read then, as far as an
/// advance looks: its last block and one other, taken in turn. Claude Code
/// only ever adds to a record, so a look costs what was added, not what was
/// read (user 2026-11-21: "这个要低性能"); bytes changed where an advance
/// did not look are found by the background check, which reads every block
/// (`prefix_unchanged`, §10).
fn prefix_holds(file: &mut File, seam: &Seam) -> io::Result<bool> {
    if seam.offset == 0 {
        return Ok(true);
    }
    let count = seam.offset.div_ceil(BLOCK) as usize;
    if seam.blocks.len() != count {
        return Ok(false);
    }
    let checked: Vec<usize> = if count < 2 {
        (0..count).collect()
    } else {
        // In turn over every advance of the process: a clock's last digits
        // are not spread over the blocks on every system (codex-8d58c146).
        static TURN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let turn = TURN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % (count - 1);
        vec![count - 1, turn]
    };
    let mut bytes = vec![0u8; BLOCK as usize];
    for index in checked {
        let start = index as u64 * BLOCK;
        let length = (seam.offset - start).min(BLOCK) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut bytes[..length])?;
        if xxh3_64(&bytes[..length]) != seam.blocks[index] {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
