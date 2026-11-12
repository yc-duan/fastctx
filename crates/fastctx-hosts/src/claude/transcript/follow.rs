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

use super::{Scanned, project, scan};
use crate::claude::subagents;
use crate::claude::titles::Titles;
use fastctx_platform::file_read;
use fastctx_types::file_identity::FileIdentity;
use fastctx_types::session::HostAgentStart;
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
    /// What could not be made out before it.
    pub problems: Vec<String>,
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
}

/// Reads `path` past `seam`: the prefix checked, the tail read twice and
/// projected, `earlier` the record ids sessions before it hold. The seam
/// moves to the last start of the user's words past it that `accept` takes,
/// given the time they were said, with no tool call before it waiting.
/// `ended`: the session's process is gone, so a last row without its line's
/// end will never have one.
pub fn tail(
    path: &Path,
    seam: &Seam,
    earlier: &HashSet<String>,
    accept: &dyn Fn(Timestamp) -> bool,
    ended: bool,
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
    let mut scanned = Scanned::new(seam.line, seam.offset, seam.titles.clone(), seam.problems.clone());
    file.seek(SeekFrom::Start(seam.offset))?;
    scan(&mut BufReader::with_capacity(1 << 20, (&mut file).take(metadata.len() - seam.offset)), None, false, &mut scanned, Some(&mut first))?;
    let (_, end) = scanned.reached();
    if seam.offset > 0
        && let Some(why) = unsupported(&scanned, seam)
    {
        return Ok(Err(Restart::Unsupported(why)));
    }
    // A whole reading takes a last row that lacks only its line's end, once
    // nothing will write the rest (CC17).
    if ended && end < metadata.len() {
        return Ok(Err(Restart::Unsupported("a last row without its line's end")));
    }
    let since = scanned.rows.iter().find_map(|row| at(&row.value));
    let moved = next_seam(&scanned, seam, accept);
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
    let next = match moved {
        Some((to, _, latest, abandoned)) => {
            let mut next = advanced(&mut file, seam, to, identity, earlier, &beside)?;
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
    let transcript = project(scanned, &[], earlier, beside, seam.agents.clone())?;
    Ok(Ok(Tail { transcript, seam: next, end, since, seam_at }))
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

/// Where the seam goes next: the start of the last row past it that starts
/// the user's words, its time accepted and later than every row before it
/// (CC13: events at one moment are named by their order there, so none are
/// split across it), with every tool call before it answered. With it, the
/// latest time before it and the calls spoken past unanswered before it.
#[allow(clippy::type_complexity)]
fn next_seam(
    scanned: &Scanned,
    seam: &Seam,
    accept: &dyn Fn(Timestamp) -> bool,
) -> Option<(u64, Timestamp, Option<Timestamp>, Vec<String>)> {
    // Each call still waiting, with how many times the user spoke since.
    let mut waiting: HashMap<&str, usize> = HashMap::new();
    let (mut found, mut last_start) = (None, None::<u64>);
    let mut latest = seam.latest;
    let mut abandoned: Vec<String> = Vec::new();
    for row in &scanned.rows {
        // A row a later one of its UUID replaced stands out of file order:
        // no seam is placed by it.
        let in_order = last_start.is_none_or(|last| row.start > last);
        last_start = Some(last_start.map_or(row.start, |last| last.max(row.start)));
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
        if in_order
            && waiting.is_empty()
            && row.start > seam.offset
            && prompt
            && let Some(said) = time.filter(|said| latest.is_none_or(|latest| *said > latest) && accept(*said))
        {
            found = Some((row.start, said, latest, abandoned.clone()));
        }
        if let Some(time) = time {
            latest = Some(latest.map_or(time, |latest| latest.max(time)));
        }
        for block in row.blocks().into_iter().flatten() {
            match (row.kind(), block.get("type").and_then(Value::as_str)) {
                (Some("assistant"), Some("tool_use")) => {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        waiting.insert(id, 0);
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
    }
    found
}

/// The seam at `to`: what the rows between `seam` and it leave for a reading
/// that takes up there.
fn advanced(
    file: &mut File,
    seam: &Seam,
    to: u64,
    identity: FileIdentity,
    earlier: &HashSet<String>,
    beside: &subagents::Beside,
) -> io::Result<Seam> {
    let blocks = blocks(file, seam, to)?;
    let mut scanned = Scanned::new(seam.line, seam.offset, seam.titles.clone(), seam.problems.clone());
    file.seek(SeekFrom::Start(seam.offset))?;
    scan(&mut BufReader::new(file.take(to - seam.offset)), None, false, &mut scanned, None)?;
    let (line, offset) = scanned.reached();
    if offset != to {
        return Err(io::Error::other("the record's rows before the new seam did not read whole"));
    }
    let titles = scanned.titles().clone();
    let between = project(scanned, &[], earlier, beside.clone(), seam.agents.clone())?;
    Ok(Seam {
        identity: Some(identity),
        offset,
        line,
        blocks,
        titles,
        problems: between.problems.clone(),
        agents: between
            .agents()
            .iter()
            .map(|(id, start)| (id.clone(), HostAgentStart { prompt: None, ..start.clone() }))
            .collect(),
        latest: None,
        abandoned: Vec::new(),
    })
}

/// What in a tail refers back past its seam in a way the tail cannot show
/// alone: a compaction, whose summary names what it kept, a row that takes
/// back rows not in the tail, and the answer to a call before the seam the
/// user had spoken past.
fn unsupported(scanned: &Scanned, seam: &Seam) -> Option<&'static str> {
    for row in &scanned.rows {
        if row.kind() == Some("user")
            && !seam.abandoned.is_empty()
            && row.blocks().into_iter().flatten().any(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_result")
                    && block.get("tool_use_id").and_then(Value::as_str).is_some_and(|id| seam.abandoned.iter().any(|known| known == id))
            })
        {
            return Some("the answer to a call before the seam");
        }
        if row.compaction() || row.replacement() || row.value["isCompactSummary"] == true {
            return Some("a compaction");
        }
        let references = match row.kind() {
            Some("assistant") => row.value.get("supersedesUuids"),
            Some("system") if row.value["subtype"] == "model_refusal_fallback" => row.value.get("retractedMessageUuids"),
            _ => None,
        };
        if let Some(ids) = references.and_then(Value::as_array)
            && ids.iter().filter_map(Value::as_str).any(|id| !scanned.by_uuid.contains_key(id))
        {
            return Some("a row taking back rows before the seam");
        }
    }
    None
}

fn at(row: &Value) -> Option<Timestamp> {
    row.get("timestamp").and_then(Value::as_str).and_then(|text| text.parse().ok())
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
