//! Indexed call facts. Hot step detail is capped at 200 rows / 2 MiB per call; folded
//! identities remain until conversation deletion so old full updates deduplicate.

use super::{projections, snapshot::StoredCall};
use crate::record_payloads::{self as payloads, Reference};
use fastctx_storage::{
    Error, Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::run_facts::{JobFacts, RunStep, StepSummary, WriteFact};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// A call's newest steps keep their detail, at most this many of them and
/// this many bytes; older ones fold into a count by machine and kind.
const HOT_STEPS: i64 = 200;
const HOT_BYTES: i64 = 2 * 1024 * 1024;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS run_calls (
    conversation TEXT NOT NULL, call TEXT NOT NULL, turn TEXT, body BLOB NOT NULL,
    PRIMARY KEY(conversation,call)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS run_calls_turn ON run_calls(conversation,turn,call);
CREATE TABLE IF NOT EXISTS run_scripts (
    conversation TEXT NOT NULL, call TEXT NOT NULL, script TEXT NOT NULL,
    PRIMARY KEY(conversation,call)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS run_job_sources (
    conversation TEXT NOT NULL, call TEXT NOT NULL, machine TEXT NOT NULL, job TEXT NOT NULL,
    record TEXT, script TEXT, started TEXT,
    PRIMARY KEY(conversation,call,machine,job)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS run_versions (
    conversation TEXT PRIMARY KEY, version INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS run_inline_budget (conversation TEXT NOT NULL, call TEXT NOT NULL, used INTEGER NOT NULL, PRIMARY KEY(conversation,call)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS run_inline_hashes (conversation TEXT NOT NULL, call TEXT NOT NULL, id TEXT NOT NULL, hash TEXT NOT NULL, PRIMARY KEY(conversation,call,id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS run_writes (
    position INTEGER NOT NULL, conversation TEXT NOT NULL, call TEXT NOT NULL,
    id TEXT NOT NULL, body BLOB NOT NULL, PRIMARY KEY(conversation,position), UNIQUE(conversation,call,id)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS run_visits (
    position INTEGER NOT NULL, conversation TEXT NOT NULL, call TEXT NOT NULL,
    machine TEXT NOT NULL, job TEXT NOT NULL, PRIMARY KEY(conversation,position), UNIQUE(conversation,call,machine,job)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS run_steps (
    position INTEGER NOT NULL, conversation TEXT NOT NULL, call TEXT NOT NULL,
    id TEXT NOT NULL, body BLOB, fingerprint TEXT NOT NULL, finished INTEGER NOT NULL,
    machine TEXT NOT NULL, kind TEXT NOT NULL, first INTEGER NOT NULL, last INTEGER NOT NULL,
    PRIMARY KEY(conversation,position), UNIQUE(conversation,call,id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS run_steps_hot ON run_steps(conversation,call,position) WHERE body IS NOT NULL;
CREATE TABLE IF NOT EXISTS run_folded (
    conversation TEXT NOT NULL, call TEXT NOT NULL, machine TEXT NOT NULL, kind TEXT NOT NULL,
    count INTEGER NOT NULL, first INTEGER NOT NULL, last INTEGER NOT NULL,
    PRIMARY KEY(conversation,call,machine,kind)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS run_jobs (
    conversation TEXT NOT NULL, call TEXT NOT NULL, machine TEXT NOT NULL, job TEXT NOT NULL,
    sequence BLOB NOT NULL, body BLOB NOT NULL, PRIMARY KEY(conversation,call,machine,job)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS run_calls_active ON run_calls(conversation,json_extract(body,'$.facts.machine'),call) WHERE json_valid(body) AND json_extract(body,'$.unavailable') IS NULL AND (json_extract(body,'$.facts.state.kind')='running' OR (json_extract(body,'$.facts.state.kind')='exited' AND json_extract(body,'$.facts.state.remaining')>0));
CREATE INDEX IF NOT EXISTS run_jobs_active ON run_jobs(conversation,call,machine,job) WHERE json_valid(body) AND (json_extract(body,'$.state.kind')='running' OR (json_extract(body,'$.state.kind')='exited' AND json_extract(body,'$.state.remaining')>0));
CREATE INDEX IF NOT EXISTS run_jobs_turn ON run_jobs(conversation,json_extract(body,'$.turn'),call,machine,job) WHERE json_valid(body);
CREATE INDEX IF NOT EXISTS run_calls_invalid ON run_calls(conversation,call) WHERE NOT json_valid(body);
CREATE INDEX IF NOT EXISTS run_jobs_invalid ON run_jobs(conversation,call,machine,job) WHERE NOT json_valid(body);
CREATE TABLE IF NOT EXISTS run_imported (
    conversation TEXT PRIMARY KEY, checked INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
";

pub(super) fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| Error::new(error.to_string()))
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| Error::with_kind(fastctx_storage::ErrorKind::Corrupt, error.to_string()))
}

pub(super) fn head(db: &Connection, conversation: &str, call: &str) -> Result<Option<StoredCall>> {
    let bytes: Option<Vec<u8>> = db
        .query_row("SELECT body FROM run_calls WHERE conversation=?1 AND call=?2", (conversation, call), |row| {
            row.get(0)
        })
        .optional()?;
    bytes.as_deref().map(decode).transpose()
}

pub(super) fn save(db: &Connection, conversation: &str, stored: &StoredCall) -> Result<()> {
    if !stored.facts.notes.is_empty() || !stored.facts.spans.is_empty() {
        return Err(Error::new("call projections must be staged before committing metadata"));
    }
    if !stored.script.is_empty() {
        db.execute(
            "INSERT INTO run_scripts(conversation,call,script) VALUES(?1,?2,?3)
            ON CONFLICT(conversation,call) DO UPDATE SET script=excluded.script WHERE script!=excluded.script",
            (conversation, &stored.call_id, &stored.script),
        )?;
    }
    if stored.unavailable.is_some() {
        super::retirement::unavailable(db, conversation, &stored.call_id)?;
    }
    let mut head = stored.clone();
    head.script.clear();
    head.facts.steps.clear();
    head.facts.writes.clear();
    head.facts.visited.clear();
    head.facts.folded_steps.clear();
    let body = encode(&head)?;
    db.execute(
        "INSERT INTO run_calls(conversation,call,turn,body) VALUES(?1,?2,?3,?4)
        ON CONFLICT(conversation,call) DO UPDATE SET turn=excluded.turn,body=excluded.body
        WHERE run_calls.body != excluded.body",
        params![conversation, stored.call_id, stored.facts.turn, body],
    )?;
    Ok(())
}

pub(super) fn changed(db: &Connection, conversation: &str) -> Result<()> {
    db.execute(
        "INSERT INTO run_versions(conversation,version) VALUES(?1,1)
        ON CONFLICT(conversation) DO UPDATE SET version=version+1",
        [conversation],
    )?;
    Ok(())
}

const INLINE_BUDGET: u64 = 256 * 1024;
fn inline_hash(fact: &WriteFact) -> Result<Option<String>> {
    fact.diff
        .as_ref()
        .and_then(|diff| diff.lines.as_ref())
        .map(|lines| encode(lines).map(|bytes| fingerprint(&bytes)))
        .transpose()
}
fn same_write(left: &WriteFact, right: &WriteFact) -> bool {
    let (mut left, mut right) = (left.clone(), right.clone());
    if let Some(diff) = &mut left.diff {
        diff.lines = None;
    }
    if let Some(diff) = &mut right.diff {
        diff.lines = None;
    }
    left == right
}
fn known_write(db: &Connection, conversation: &str, call: &str, fact: &WriteFact, prior: &WriteFact) -> Result<bool> {
    if !same_write(prior, fact) {
        return Err(Error::new("write fact identity changed"));
    }
    let Some(incoming) = inline_hash(fact)? else { return Ok(true) };
    let recorded: Option<String> = db
        .query_row(
            "SELECT hash FROM run_inline_hashes WHERE conversation=?1 AND call=?2 AND id=?3",
            (conversation, call, &fact.id),
            |row| row.get(0),
        )
        .optional()?;
    let recorded = match recorded {
        Some(hash) => Some(hash),
        None => inline_hash(prior)?,
    };
    match recorded {
        Some(recorded) if recorded != incoming => Err(Error::new("inline write content changed")),
        Some(_) => Ok(true),
        None => Ok(false),
    }
}
pub(super) fn writes(db: &Connection, conversation: &str, call: &str, incoming: &[WriteFact]) -> Result<()> {
    if incoming.is_empty() {
        return Ok(());
    }
    // An earlier SQL format may already contain inline rows. Reserve its whole
    // allowance instead of scanning its history in a writer transaction.
    db.prepare_cached("INSERT OR IGNORE INTO run_inline_budget(conversation,call,used) VALUES(?1,?2,CASE WHEN EXISTS(SELECT 1 FROM run_writes WHERE conversation=?1 AND call=?2) THEN ?3 ELSE 0 END)")?
        .execute((conversation,call,INLINE_BUDGET as i64))?;
    let mut used = db
        .prepare_cached("SELECT used FROM run_inline_budget WHERE conversation=?1 AND call=?2")?
        .query_row((conversation, call), |row| row.get::<_, i64>(0))? as u64;
    let mut prior = db.prepare_cached("SELECT body FROM run_writes WHERE conversation=?1 AND call=?2 AND id=?3")?;
    let mut hashes = db.prepare_cached("INSERT INTO run_inline_hashes(conversation,call,id,hash) VALUES(?1,?2,?3,?4) ON CONFLICT(conversation,call,id) DO NOTHING")?;
    let mut insert = db.prepare_cached("INSERT INTO run_writes(conversation,call,id,body,position) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(conversation,call,id) DO UPDATE SET body=excluded.body")?;
    // Positions for the facts written here, taken at once with the first.
    let mut positions = 0..0;
    for (index, fact) in incoming.iter().enumerate() {
        let prior: Option<Vec<u8>> = prior.query_row((conversation, call, &fact.id), |row| row.get(0)).optional()?;
        if let Some(prior) = &prior
            && known_write(db, conversation, call, fact, &decode(prior)?)?
        {
            continue;
        }
        let lines = fact.diff.as_ref().and_then(|diff| diff.lines.as_ref()).map(encode).transpose()?;
        let mut kept = fact.clone();
        if let (Some(diff), Some(lines)) = (&mut kept.diff, &lines) {
            let size = diff.bytes.unwrap_or(fastctx_types::run_facts::INLINE_DIFF).max(lines.len() as u64);
            if size > INLINE_BUDGET.saturating_sub(used) {
                diff.lines = None;
            } else {
                used += size;
            }
        }
        if let Some(lines) = &lines {
            hashes.execute((conversation, call, &fact.id, fingerprint(lines)))?;
        }
        if positions.is_empty() {
            let count = (incoming.len() - index) as i64;
            let first = crate::records::positions::reserve(db, conversation, "run-writes", count)?;
            positions = first..first + count;
        }
        let position = positions.next().expect("a position for every fact left");
        insert.execute(params![conversation, call, fact.id, encode(&kept)?, position])?;
    }
    db.prepare_cached("UPDATE run_inline_budget SET used=?3 WHERE conversation=?1 AND call=?2 AND used!=?3")?
        .execute((conversation, call, used as i64))?;
    Ok(())
}

pub(super) fn visits(
    db: &Connection,
    conversation: &str,
    call: &str,
    incoming: &[(fastctx_types::MachineName, fastctx_types::JobId)],
) -> Result<()> {
    if incoming.is_empty() {
        return Ok(());
    }
    let mut insert = db.prepare_cached(
        "INSERT OR IGNORE INTO run_visits(conversation,call,machine,job,position) VALUES(?1,?2,?3,?4,?5)",
    )?;
    let first = crate::records::positions::reserve(db, conversation, "run-visits", incoming.len() as i64)?;
    for (position, (machine, job)) in (first..).zip(incoming) {
        insert.execute((conversation, call, machine.as_str(), job.as_str(), position))?;
    }
    Ok(())
}

pub(super) fn steps(db: &Connection, conversation: &str, call: &str, incoming: &[RunStep]) -> Result<()> {
    if incoming.is_empty() {
        return Ok(());
    }
    let mut query = db.prepare_cached(
        "SELECT fingerprint,finished,body IS NULL FROM run_steps WHERE conversation=?1 AND call=?2 AND id=?3",
    )?;
    let mut insert = db.prepare_cached("INSERT INTO run_steps(conversation,call,id,body,fingerprint,finished,machine,kind,first,last,position)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
        ON CONFLICT(conversation,call,id) DO UPDATE SET body=excluded.body,fingerprint=excluded.fingerprint,
            finished=excluded.finished,machine=excluded.machine,kind=excluded.kind,first=excluded.first,last=excluded.last")?;
    // Positions for the steps written here, taken at once with the first.
    let mut positions = 0..0;
    for (index, step) in incoming.iter().enumerate() {
        let bytes = encode(step)?;
        let fingerprint = fingerprint(&bytes);
        let previous: Option<(String, bool, bool)> = query
            .query_row((conversation, call, &step.id), |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        if previous
            .as_ref()
            .is_some_and(|(hash, finished, _)| *hash == fingerprint || (*finished && step.ended.is_none()))
        {
            continue;
        }
        let first = step.began.as_microsecond();
        let last = step.ended.unwrap_or(step.began).as_microsecond();
        if previous.is_some_and(|(_, _, folded)| folded) {
            db.prepare_cached("UPDATE run_folded SET last=max(last,?4),first=min(first,?5)
                WHERE conversation=?1 AND call=?2 AND (machine,kind)=(SELECT machine,kind FROM run_steps WHERE conversation=?1 AND call=?2 AND id=?3)")?
                .execute((conversation,call,&step.id,last,first))?;
            db.prepare_cached(
                "UPDATE run_steps SET fingerprint=?4,finished=?5,last=?6 WHERE conversation=?1 AND call=?2 AND id=?3",
            )?
            .execute((conversation, call, &step.id, fingerprint, step.ended.is_some(), last))?;
            continue;
        }
        if positions.is_empty() {
            let count = (incoming.len() - index) as i64;
            let first = crate::records::positions::reserve(db, conversation, "run-steps", count)?;
            positions = first..first + count;
        }
        insert.execute(params![
            conversation,
            call,
            step.id,
            bytes,
            fingerprint,
            step.ended.is_some(),
            step.machine.as_str(),
            step.did.kind(),
            first,
            last,
            positions.next().expect("a position for every step left")
        ])?;
    }
    fold(db, conversation, call)
}

/// Folds the oldest hot steps of `call` past its window, reading only those.
fn fold(db: &Connection, conversation: &str, call: &str) -> Result<()> {
    // The scoped primary key satisfies the ordering but scans retired rows.
    // Keep retirement bounded by the hot-only index, independently of planner estimates.
    let (mut hot, mut bytes) = db
        .prepare_cached("SELECT count(*),coalesce(sum(length(body)),0) FROM run_steps INDEXED BY run_steps_hot WHERE conversation=?1 AND call=?2 AND body IS NOT NULL")?
        .query_row((conversation, call), |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    if hot <= HOT_STEPS && bytes <= HOT_BYTES {
        return Ok(());
    }
    let mut folded = BTreeMap::<(String, String), (i64, i64, i64)>::new();
    let mut through = None;
    let mut oldest = db.prepare_cached("SELECT position,machine,kind,first,last,length(body) FROM run_steps INDEXED BY run_steps_hot WHERE conversation=?1 AND call=?2 AND body IS NOT NULL ORDER BY position")?;
    let mut rows = oldest.query((conversation, call))?;
    while hot > HOT_STEPS || bytes > HOT_BYTES {
        let Some(row) = rows.next()? else { break };
        let (first, last) = (row.get::<_, i64>(3)?, row.get::<_, i64>(4)?);
        let count = folded.entry((row.get(1)?, row.get(2)?)).or_insert((0, first, last));
        *count = (count.0 + 1, count.1.min(first), count.2.max(last));
        through = Some(row.get::<_, i64>(0)?);
        hot -= 1;
        bytes -= row.get::<_, i64>(5)?;
    }
    drop(rows);
    let mut fold = db.prepare_cached("INSERT INTO run_folded(conversation,call,machine,kind,count,first,last) VALUES(?1,?2,?3,?4,?5,?6,?7)
        ON CONFLICT(conversation,call,machine,kind) DO UPDATE SET count=count+excluded.count,first=min(first,excluded.first),last=max(last,excluded.last)")?;
    for ((machine, kind), (count, first, last)) in folded {
        fold.execute(params![conversation, call, machine, kind, count, first, last])?;
    }
    if let Some(through) = through {
        db.prepare_cached("UPDATE run_steps INDEXED BY run_steps_hot SET body=NULL WHERE conversation=?1 AND call=?2 AND body IS NOT NULL AND position<=?3")?
            .execute((conversation, call, through))?;
    }
    Ok(())
}

pub(super) fn load(db: &Connection, conversation: &str, call: &str) -> Result<Option<StoredCall>> {
    if db.is_autocommit() {
        let tx = db.unchecked_transaction()?;
        let result = load(&tx, conversation, call)?;
        tx.commit()?;
        return Ok(result);
    }
    let Some(mut stored) = head(db, conversation, call)? else { return Ok(None) };
    if let Some(bytes) = payloads::read_root(db, conversation, projections::NOTES, call)? {
        stored.facts.notes = decode(&bytes)?;
    }
    if let Some(bytes) = payloads::read_root(db, conversation, projections::SPANS, call)? {
        stored.facts.spans = decode(&bytes)?;
    }
    stored.script = db
        .query_row("SELECT script FROM run_scripts WHERE conversation=?1 AND call=?2", (conversation, call), |row| {
            row.get(0)
        })
        .optional()?
        .unwrap_or_default();
    stored.facts.writes = values(
        db,
        "SELECT body FROM run_writes WHERE conversation=?1 AND call=?2 ORDER BY position",
        conversation,
        call,
    )?;
    stored.facts.steps = values(
        db,
        "SELECT body FROM run_steps INDEXED BY run_steps_hot WHERE conversation=?1 AND call=?2 AND body IS NOT NULL ORDER BY position",
        conversation,
        call,
    )?;
    let mut query =
        db.prepare_cached("SELECT machine,job FROM run_visits WHERE conversation=?1 AND call=?2 ORDER BY position")?;
    let visits =
        query.query_map((conversation, call), |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    for visit in visits {
        let (machine, job) = visit?;
        stored.facts.visited.push((
            machine.try_into().map_err(|error: fastctx_types::names::NameError| Error::new(error.to_string()))?,
            job.try_into().map_err(|error: fastctx_types::names::NameError| Error::new(error.to_string()))?,
        ));
    }
    let mut query = db.prepare_cached(
        "SELECT machine,kind,count,first,last FROM run_folded WHERE conversation=?1 AND call=?2 ORDER BY machine,kind",
    )?;
    let folded = query.query_map((conversation, call), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    for row in folded {
        let (machine, kind, count, first, last) = row?;
        stored.facts.folded_steps.push(StepSummary {
            machine: machine
                .try_into()
                .map_err(|error: fastctx_types::names::NameError| Error::new(error.to_string()))?,
            kind,
            count: count as u64,
            first: jiff::Timestamp::from_microsecond(first).map_err(|error| Error::new(error.to_string()))?,
            last: jiff::Timestamp::from_microsecond(last).map_err(|error| Error::new(error.to_string()))?,
        });
    }
    Ok(Some(stored))
}

fn values<T: serde::de::DeserializeOwned>(
    db: &Connection,
    sql: &str,
    conversation: &str,
    call: &str,
) -> Result<Vec<T>> {
    let mut query = db.prepare_cached(sql)?;
    query.query_map((conversation, call), |row| row.get::<_, Vec<u8>>(0))?.map(|row| decode(&row?)).collect()
}

pub(super) fn job(db: &Connection, conversation: &str, update: &JobFacts) -> Result<Option<JobFacts>> {
    let bytes: Option<Vec<u8>> = db
        .query_row(
            "SELECT body FROM run_jobs WHERE conversation=?1 AND call=?2 AND machine=?3 AND job=?4",
            (conversation, &update.call_id, update.machine.as_str(), update.job.as_str()),
            |row| row.get(0),
        )
        .optional()?;
    bytes.as_deref().map(decode).transpose()
}

pub(super) fn save_job(
    db: &Connection,
    conversation: &str,
    update: &JobFacts,
    notes: Option<&Reference>,
) -> Result<()> {
    if update.notes.is_some() {
        return Err(Error::new("job notes must be staged before committing metadata"));
    }
    let mut metadata = update.clone();
    let mut current = true;
    if let Some(previous) = job(db, conversation, update)? {
        if previous.sequence > update.sequence {
            metadata = previous.clone();
            current = false;
        }
        metadata.state = metadata.state.or(previous.state);
        metadata.turn = metadata.turn.or(previous.turn);
        metadata.output = metadata.output.or(previous.output);
    }
    metadata.turn = metadata.turn.or(update.turn.clone());
    metadata.output = metadata.output.or(update.output.clone());
    if let Some(notes) = notes {
        let key = projections::job_key(update)?;
        if current || payloads::root(db, conversation, projections::JOB_NOTES, &key)?.is_none() {
            payloads::link(db, conversation, projections::JOB_NOTES, &key, Some(notes))?;
        }
    }
    if update.record.is_some() || update.script.is_some() || update.started_at.is_some() {
        db.execute("INSERT INTO run_job_sources(conversation,call,machine,job,record,script,started) VALUES(?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(conversation,call,machine,job) DO UPDATE SET
                record=coalesce(excluded.record,record),script=coalesce(excluded.script,script),started=coalesce(excluded.started,started)
            WHERE (excluded.record IS NOT NULL AND excluded.record IS NOT record)
                OR (excluded.script IS NOT NULL AND excluded.script IS NOT script)
                OR (excluded.started IS NOT NULL AND excluded.started IS NOT started)",
            params![conversation,update.call_id,update.machine.as_str(),update.job.as_str(),update.record,update.script,update.started_at.map(|at|at.to_string())])?;
    }
    metadata.record = None;
    metadata.script = None;
    metadata.started_at = None;
    metadata.writes.clear();
    metadata.steps.clear();
    metadata.visited.clear();
    db.execute(
        "INSERT INTO run_jobs(conversation,call,machine,job,sequence,body) VALUES(?1,?2,?3,?4,?5,?6)
        ON CONFLICT(conversation,call,machine,job) DO UPDATE SET sequence=excluded.sequence,body=excluded.body
        WHERE run_jobs.sequence!=excluded.sequence OR run_jobs.body!=excluded.body",
        params![
            conversation,
            update.call_id,
            update.machine.as_str(),
            update.job.as_str(),
            metadata.sequence.to_be_bytes().as_slice(),
            encode(&metadata)?
        ],
    )?;
    Ok(())
}

pub(super) fn job_source(db: &Connection, conversation: &str, update: &mut JobFacts) -> Result<()> {
    if let Some(bytes) = payloads::read_root(db, conversation, projections::JOB_NOTES, &projections::job_key(update)?)?
    {
        update.notes = Some(decode(&bytes)?);
    }
    type Source = (Option<String>, Option<String>, Option<String>);
    let source: Option<Source> = db.query_row("SELECT record,script,started FROM run_job_sources WHERE conversation=?1 AND call=?2 AND machine=?3 AND job=?4",
        (conversation,&update.call_id,update.machine.as_str(),update.job.as_str()),|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    if let Some((record, script, started)) = source {
        update.record = record;
        update.script = script;
        update.started_at =
            started.map(|at| at.parse().map_err(|error: jiff::Error| Error::new(error.to_string()))).transpose()?;
    }
    Ok(())
}

/// Reduces legacy full progress on a read connection, before admission to the
/// writer. The write transaction repeats identity and sequence checks.
pub(super) fn additions(db: &Connection, conversation: &str, update: &JobFacts) -> Result<Option<JobFacts>> {
    let Some(stored) = head(db, conversation, &update.call_id)? else { return Ok(None) };
    super::snapshot::validate(&stored, update)?;
    let members = difference(db, conversation, &update.call_id, &update.writes, &update.steps, &update.visited)?;
    let mut next = update.clone();
    next.writes = members.writes;
    next.steps = members.steps;
    next.visited = members.visited;
    Ok(Some(next))
}

#[derive(Default)]
pub(super) struct Members {
    pub writes: Vec<WriteFact>,
    pub steps: Vec<RunStep>,
    pub visited: Vec<(fastctx_types::MachineName, fastctx_types::JobId)>,
}

pub(super) fn difference(
    db: &Connection,
    conversation: &str,
    call: &str,
    writes: &[WriteFact],
    steps: &[RunStep],
    visited: &[(fastctx_types::MachineName, fastctx_types::JobId)],
) -> Result<Members> {
    let mut next = Members::default();
    let mut write_query =
        db.prepare_cached("SELECT body FROM run_writes WHERE conversation=?1 AND call=?2 AND id=?3")?;
    for fact in writes {
        let previous: Option<Vec<u8>> =
            write_query.query_row((conversation, call, &fact.id), |row| row.get(0)).optional()?;
        if let Some(previous) = previous
            && known_write(db, conversation, call, fact, &decode(&previous)?)?
        {
            continue;
        }
        next.writes.push(fact.clone());
    }
    let mut step_query =
        db.prepare_cached("SELECT fingerprint,finished FROM run_steps WHERE conversation=?1 AND call=?2 AND id=?3")?;
    for step in steps {
        let previous: Option<(String, bool)> =
            step_query.query_row((conversation, call, &step.id), |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        if let Some((known_hash, finished)) = previous {
            if finished && step.ended.is_none() {
                continue;
            }
            if known_hash == fingerprint(&encode(step)?) {
                continue;
            }
        }
        next.steps.push(step.clone());
    }
    let mut visit_query =
        db.prepare_cached("SELECT 1 FROM run_visits WHERE conversation=?1 AND call=?2 AND machine=?3 AND job=?4")?;
    for (machine, job) in visited {
        if visit_query
            .query_row((conversation, call, machine.as_str(), job.as_str()), |row| row.get::<_, i32>(0))
            .optional()?
            .is_none()
        {
            next.visited.push((machine.clone(), job.clone()));
        }
    }
    Ok(next)
}

fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
