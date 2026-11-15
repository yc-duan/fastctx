//! Following a Claude Code record as it grows (sync-v2-impl-design §10).
//!
//! What was read through a seam is never parsed again: its bytes are hashed,
//! a MiB at a time with xxh3, and checked at each advance (the whole prefix
//! up to 32 MiB, two blocks past that). What lies past the seam — the turn
//! under way and whatever came since — is read, read again to see that it
//! stood still, and projected afresh, so an item of it the host changes is
//! a new revision of the same item. A seam moves only to the start of a
//! user's words with no tool call before it still waiting for its result,
//! and only where the caller accepts (`tail`): before it nothing the record
//! says later can change what it shows, short of what refers back past a
//! seam, which is read whole instead (`Restart`).
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
/// Up to this much prefix is checked whole at every advance; past it, two
/// blocks are, the rest left to a background check (§10).
const WHOLE_PREFIX: u64 = 32 << 20;
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
    let (since, moved) = next_seam(&scanned.rows, seam, accept)?;
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
    let pairs: Vec<String> = told.iter().map(|(id, call)| format!("{id}@{call}")).collect();
    if seam.offset > 0
        && told.iter().zip(&pairs).any(|((id, _), pair)| seam.agents.contains_key(id) && !seam.told.contains(pair))
    {
        return Ok(Err(Restart::Unsupported("an agent before the seam told of another call")));
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
    Ok(Ok(Tail { transcript, seam: next, end, since, seam_at, prefix_ids }))
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
    let TailStore { rows: store, events, moved_past, asked } = kept;
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
    }
    if ended && end < metadata.len() && scanned.refused().is_none() {
        return Ok(Err(Restart::Unsupported("a last row without its line's end")));
    }
    let (since, moved) = next_seam(&scanned.rows, seam, accept)?;
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
    let mut transcript = project_asking(scanned, &[], earlier, beside.clone(), seam.agents.clone(), Output::History(events))?;
    // Asked of the rows past the seam while the store holds them.
    if let Some((to, ..)) = &moved {
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
            let mut next = advanced(&mut file, seam, to, identity, earlier, &beside, Kept::new(&mut *store))?;
            next.latest = latest;
            next.abandoned = seam.abandoned.iter().cloned().chain(abandoned).collect();
            if !reads_again(&mut file, seam.offset, end, first.digest())? {
                return Ok(Err(Restart::Unsteady));
            }
            Some(next)
        }
        None if seam.identity.is_none() => Some(Seam { identity: Some(identity), ..seam.clone() }),
        None => None,
    };
    let told: Vec<(String, String)> = beside.calls().map(|(id, call)| (id.to_string(), call.to_string())).collect();
    let pairs: Vec<String> = told.iter().map(|(id, call)| format!("{id}@{call}")).collect();
    if seam.offset > 0
        && told.iter().zip(&pairs).any(|((id, _), pair)| seam.agents.contains_key(id) && !seam.told.contains(pair))
    {
        return Ok(Err(Restart::Unsupported("an agent before the seam told of another call")));
    }
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
    Ok(Ok(Tail { transcript, seam: next, end, since, seam_at, prefix_ids: Vec::new() }))
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
/// With it, the time of the first row that has one. The rows taken once in
/// their order.
fn next_seam<L: Rows>(rows: &L, seam: &Seam, accept: &dyn Fn(Timestamp) -> bool) -> io::Result<(Option<Timestamp>, Option<Moved>)> {
    // Each call still waiting, with how many times the user spoke since.
    let mut waiting: HashMap<String, usize> = HashMap::new();
    // Where it would go, with how many calls were spoken past by then: the
    // calls only ever grow, so those before it are the first so many.
    let (mut found, mut last_start) = (None::<(u64, Timestamp, Option<Timestamp>, usize)>, None::<u64>);
    let mut latest = seam.latest;
    let mut abandoned: Vec<String> = Vec::new();
    let mut since = None;
    rows.each(&mut |_, row| {
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
        let time = at(&row.value);
        if since.is_none() {
            since = time;
        }
        if in_order
            && waiting.is_empty()
            && row.start > seam.offset
            && prompt
            && let Some(said) = time.filter(|said| latest.is_none_or(|latest| *said > latest) && accept(*said))
        {
            found = Some((row.start, said, latest, abandoned.len()));
        }
        if let Some(time) = time {
            latest = Some(latest.map_or(time, |latest| latest.max(time)));
        }
        for block in row.blocks().into_iter().flatten() {
            match (row.kind(), block.get("type").and_then(Value::as_str)) {
                (Some("assistant"), Some("tool_use")) => {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
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
    abandoned.truncate(found.as_ref().map_or(0, |(.., spoken)| *spoken));
    Ok((since, found.map(|(to, said, latest, _)| (to, said, latest, abandoned))))
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
/// alone: a compaction, whose summary names what it kept, a row that takes
/// back rows not in the tail, and the answer to a call before the seam the
/// user had spoken past.
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
    if row.compaction() || row.replacement() || row.value["isCompactSummary"] == true {
        return Ok(Some("a compaction"));
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

fn at(row: &Value) -> Option<Timestamp> {
    row.get("timestamp").and_then(Value::as_str).and_then(|text| text.parse().ok())
}

/// Whether the record at `path` is still the one read through `seam`: the
/// same file, no shorter, and every block before the seam the bytes hashed
/// as it was read — the whole prefix, as an advance checks only up to 32 MiB
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

/// Whether the bytes before the seam are the ones read then: all of them up
/// to 32 MiB, the last block and one other past that.
fn prefix_holds(file: &mut File, seam: &Seam) -> io::Result<bool> {
    if seam.offset == 0 {
        return Ok(true);
    }
    let count = seam.offset.div_ceil(BLOCK) as usize;
    if seam.blocks.len() != count {
        return Ok(false);
    }
    let checked: Vec<usize> = if seam.offset <= WHOLE_PREFIX || count < 2 {
        (0..count).collect()
    } else {
        let turn = (Timestamp::now().as_nanosecond().unsigned_abs() % (count as u128 - 1)) as usize;
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
