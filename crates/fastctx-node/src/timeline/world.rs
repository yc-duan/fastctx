//! The World's record of a conversation read into a work store of its own,
//! a page at a time and written there as it is read, never held whole
//! (design §28.51 ⑥-1). What the readings of a history ask of it — the
//! turns it tells and the words heard in each, the names and keys a merge
//! weighs, where its record is read through, its events in the order a
//! merge lays them out — are given from the store; only what grows with its
//! turns is held.
//!
//! Its rows are those it was given open first (the starts of turns open
//! before, `open`), then the record's in their order; read from its start,
//! the events of the turns a cut took back are left out (`cut`), as
//! the World's record read whole always left them (`recorded_after`, before
//! design §28.51 ⑥-1). A merge asks of a view of them
//! (`view`): all, those from a time on, or as a Codex reading has them
//! beside its host's (`codex::watch::recorded_beside`) — each numbered
//! afresh from 0, as the events of a list filtered so would be.

use super::spill::Spill;
use crate::talk::merging::{self, Key, Kind, Names};
use fastctx_storage::rusqlite::{Connection, params};
use fastctx_types::layout::ConversationDir;
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use jiff::Timestamp;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// The kinds of the World's events the store tells apart (`world.kind`).
const OTHER: i64 = 0;
const STARTED: i64 = 1;
const ENDED: i64 = 2;
const HEARD: i64 = 3;
/// A host's history put in place of another's (`HistoryChanged`).
const REPLACED: i64 = 4;
/// A request, or what resolved one.
const ASKED: i64 = 5;

/// The World's events written in one transaction, or read back at once:
/// about this many bytes of them.
const WRITTEN_BYTES: usize = 4 << 20;
const READ_BYTES: usize = 4 << 20;
/// Rows read back at most at once, and keys written in one transaction.
const AT_ONCE: i64 = 4_096;
/// What SQLite keeps of the store in memory, in KiB.
const CACHE_KIB: i64 = 2 * 1024;

fn failed(error: impl std::fmt::Display) -> String {
    format!("the World's record kept on disk: {error}")
}

fn kind_of(happened: &Happened) -> i64 {
    match happened {
        Happened::TurnStarted { .. } => STARTED,
        Happened::TurnEnded { .. } => ENDED,
        Happened::Heard { .. } => HEARD,
        Happened::Changed { change: SessionChange::HistoryChanged, .. } => REPLACED,
        Happened::Changed { change: SessionChange::Request { .. } | SessionChange::Resolved { .. }, .. } => ASKED,
        _ => OTHER,
    }
}

/// A sequence as the store keeps it: its highest bit turned, so that SQLite's
/// signed integers order as the sequences do, all 64 bits of them.
fn ordered(sequence: u64) -> i64 {
    (sequence ^ (1 << 63)) as i64
}

/// The sequence the store keeps as `kept` (`ordered`).
fn sequence_of(kept: i64) -> u64 {
    (kept as u64) ^ (1 << 63)
}

/// `at` as the store compares times: its seconds, then its nanoseconds,
/// both of its sign.
fn parts(at: Timestamp) -> (i64, i32) {
    (at.as_second(), at.subsec_nanosecond())
}

fn digest(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn decoded(body: &[u8]) -> Result<SessionEvent, String> {
    serde_json::from_slice(body).map_err(failed)
}

/// The words heard in each turn, as the store finds them again: by their
/// length and digest, the rows of those so — the words themselves read back
/// to tell.
type Said = HashMap<String, HashMap<(usize, u64), Vec<i64>>>;

/// A turn's start as the record tells it: its row, sequence, time, turn.
struct Start {
    n: i64,
    sequence: u64,
    at: Timestamp,
    turn: String,
}

/// The World's record of a conversation, kept on disk (the module's doc).
pub(crate) struct World {
    spill: Spill,
    latest: u64,
    cut: bool,
    /// How many of the rows are those given open, first.
    open: i64,
    /// Of what it records — those open, then those read but the turns a cut
    /// took back — the starts of turns and their ends, in their order (an
    /// end: its time and turn), and the words heard in each turn.
    starts: Vec<Start>,
    ends: Vec<(Timestamp, String)>,
    heard: Said,
    /// How many the view holds.
    viewed: usize,
    /// Read as it was recorded, no turn cut (`read_as_recorded`).
    as_recorded: bool,
}

/// A row of the World's record lined up to be written: its number,
/// sequence, time, kind, turn and body, and how a history of the World's
/// record alone orders it (`Order`).
type Row = (i64, u64, Timestamp, i64, Option<String>, Vec<u8>, Order);

/// How `talk::recorded_history` orders an event of a history of the World's
/// record alone, and whether it keeps it: whether it is kept
/// (`Happened::kept`) and tells a change of history (`talk::changes_history`,
/// left out after), its place within its turn, and its own time (an item's
/// start where it has one).
#[derive(Clone, Copy)]
struct Order {
    kept: bool,
    changes: bool,
    phase: i64,
    at: Timestamp,
}

impl Order {
    fn of(event: &SessionEvent) -> Self {
        let phase = match &event.happened {
            Happened::TurnStarted { .. } => 0,
            Happened::Heard { .. } => 1,
            Happened::TurnEnded { .. } => 3,
            _ => 2,
        };
        let at = match &event.happened {
            Happened::Finished { item, .. } => item.started_at.unwrap_or(event.at),
            _ => event.at,
        };
        Order { kept: event.happened.kept(), changes: crate::talk::changes_history(event), phase, at }
    }
}

/// The rows of a World's record being written (`World::read`).
struct Writing {
    spill: Spill,
    n: i64,
    open: i64,
    rows: Vec<Row>,
    bytes: usize,
    removed: HashSet<String>,
    /// When each turn began, by the first time of those of it kept
    /// (`talk::recorded_history`).
    began: HashMap<String, Timestamp>,
}

impl Writing {
    fn new(mut spill: Spill, open: &[SessionEvent]) -> Result<Self, String> {
        spill.begin()?;
        // Written in order and read back mostly so, beside a reading's own
        // store: a smaller cache than a reading's does.
        spill.connection()?.execute_batch(&format!("PRAGMA cache_size = -{CACHE_KIB};")).map_err(failed)?;
        let mut writing = Writing { spill, n: 0, open: 0, rows: Vec::new(), bytes: 0, removed: HashSet::new(), began: HashMap::new() };
        for event in open {
            writing.push(0, event, serde_json::to_vec(event).map_err(failed)?)?;
        }
        writing.open = writing.n;
        Ok(writing)
    }

    fn push(&mut self, sequence: u64, event: &SessionEvent, body: Vec<u8>) -> Result<(), String> {
        if let Happened::Changed { change: SessionChange::HistoryCut { removed_turns }, .. } = &event.happened {
            self.removed.extend(removed_turns.iter().cloned());
        }
        self.n += 1;
        self.bytes += body.len();
        let turn = crate::talk::turn_of(&event.happened).map(str::to_string);
        let order = Order::of(event);
        if let Some(turn) = turn.as_ref().filter(|_| order.kept) {
            self.began.entry(turn.clone()).and_modify(|at| *at = (*at).min(event.at)).or_insert(event.at);
        }
        self.rows.push((self.n, sequence, event.at, kind_of(&event.happened), turn, body, order));
        if self.bytes >= WRITTEN_BYTES || self.rows.len() >= AT_ONCE as usize {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        let rows = std::mem::take(&mut self.rows);
        self.bytes = 0;
        let db = self.spill.connection()?;
        fastctx_platform::roles::in_place("keeping the World's record on disk", || -> Result<(), fastctx_storage::rusqlite::Error> {
            let transaction = db.unchecked_transaction()?;
            {
                let mut insert = transaction.prepare_cached(
                    "INSERT INTO world (n, sequence, second, nanosecond, kind, turn, body, kept, changes, phase, sort_second, sort_nanosecond)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                )?;
                for (n, sequence, at, kind, turn, body, order) in rows {
                    let ((second, nanosecond), (sort_second, sort_nanosecond)) = (parts(at), parts(order.at));
                    insert.execute(params![
                        n,
                        ordered(sequence),
                        second,
                        nanosecond,
                        kind,
                        turn,
                        body,
                        order.kept,
                        order.changes,
                        order.phase,
                        sort_second,
                        sort_nanosecond
                    ])?;
                }
            }
            transaction.commit()
        })
        .map_err(failed)
    }

    /// The record written, read through `latest`: from its start
    /// (`from_start`), the turns a cut took back left out — but as it was
    /// recorded (`as_recorded`), all of it.
    fn finish(mut self, latest: u64, from_start: bool, as_recorded: bool) -> Result<World, String> {
        self.flush()?;
        let cut = !self.removed.is_empty();
        let db = self.spill.connection()?;
        if as_recorded {
            let began = std::mem::take(&mut self.began);
            fastctx_platform::roles::in_place("keeping the World's record on disk", || -> Result<(), fastctx_storage::rusqlite::Error> {
                let transaction = db.unchecked_transaction()?;
                for (turn, at) in &began {
                    let (second, nanosecond) = parts(*at);
                    transaction.prepare_cached("INSERT INTO began (turn, second, nanosecond) VALUES (?1, ?2, ?3)")?.execute(params![turn, second, nanosecond])?;
                }
                transaction.commit()
            })
            .map_err(failed)?;
        }
        if from_start && cut && !as_recorded {
            let removed = std::mem::take(&mut self.removed);
            fastctx_platform::roles::in_place("keeping the World's record on disk", || -> Result<(), fastctx_storage::rusqlite::Error> {
                let transaction = db.unchecked_transaction()?;
                for turn in &removed {
                    transaction.prepare_cached("INSERT OR IGNORE INTO removed (turn) VALUES (?1)")?.execute(params![turn])?;
                }
                transaction.execute("UPDATE world SET cut = 1 WHERE n > ?1 AND turn IN (SELECT turn FROM removed)", params![self.open])?;
                transaction.commit()
            })
            .map_err(failed)?;
        }
        // What the turns are read from: their starts, ends and words, as
        // `claude::history::turns` and `placement::Turns` read them — of no
        // use to a history of the World's record alone.
        let (mut starts, mut ends, mut heard) = (Vec::new(), Vec::new(), Said::new());
        let mut after = 0i64;
        loop {
            if as_recorded {
                break;
            }
            // A batch at a time, of at most `AT_ONCE` and about `READ_BYTES`.
            let rows = fastctx_platform::roles::in_place("reading the World's record kept on disk", || -> Result<Vec<(i64, u64, Vec<u8>)>, fastctx_storage::rusqlite::Error> {
                let mut query = db.prepare_cached("SELECT n, sequence, body FROM world WHERE n > ?1 AND cut = 0 AND kind IN (?2, ?3, ?4) ORDER BY n LIMIT ?5")?;
                let mut rows = query.query(params![after, STARTED, ENDED, HEARD, AT_ONCE])?;
                let (mut batch, mut bytes) = (Vec::new(), 0usize);
                while bytes < READ_BYTES
                    && let Some(row) = rows.next()?
                {
                    let body: Vec<u8> = row.get(2)?;
                    bytes += body.len();
                    batch.push((row.get(0)?, sequence_of(row.get(1)?), body));
                }
                Ok(batch)
            })
            .map_err(failed)?;
            let Some((last, ..)) = rows.last() else { break };
            after = *last;
            for (n, sequence, body) in rows {
                let event = decoded(&body)?;
                match event.happened {
                    Happened::TurnStarted { turn } => starts.push(Start { n, sequence, at: event.at, turn }),
                    Happened::TurnEnded { turn, .. } => ends.push((event.at, turn)),
                    Happened::Heard { turn: Some(turn), text, .. } => {
                        heard.entry(turn).or_default().entry((text.len(), digest(&text))).or_default().push(n);
                    }
                    _ => {}
                }
            }
        }
        Ok(World { spill: self.spill, latest, cut, open: self.open, starts, ends, heard, viewed: 0, as_recorded })
    }
}

impl World {
    /// The World's record of the conversation in `directory` after the
    /// sequence `after`, read a page at a time into `spill` beside the
    /// starts of the turns `open` before, as the record was read whole
    /// (from its start, the turns a cut took back left out).
    pub(crate) fn read(spill: Spill, directory: &ConversationDir, after: u64, open: &[SessionEvent]) -> Result<Self, String> {
        let mut writing = Writing::new(spill, open)?;
        let (mut from, mut through) = (after, None);
        let latest = loop {
            let (latest, next, count) = crate::session_record::page_each(directory, from, through, 512, &mut |sequence, body| {
                let event = decoded(body)?;
                writing.push(sequence, &event, body.to_vec())
            })
            .map_err(failed)?;
            through = Some(latest);
            if count == 0 {
                break latest;
            }
            from = next;
            if from >= latest {
                break latest;
            }
        };
        writing.finish(latest, after == 0, false)
    }

    /// All of the World's record of the conversation in `directory`, as it
    /// was recorded — no turn a cut took back left out — for a history of
    /// it alone (`view_as_recorded`), as `talk::read_here` reads one whose
    /// host is not read.
    pub(crate) fn read_as_recorded(spill: Spill, directory: &ConversationDir) -> Result<Self, String> {
        let mut writing = Writing::new(spill, &[])?;
        let (mut from, mut through) = (0, None);
        let latest = loop {
            let (latest, next, count) = crate::session_record::page_each(directory, from, through, 512, &mut |sequence, body| {
                let event = decoded(body)?;
                writing.push(sequence, &event, body.to_vec())
            })
            .map_err(failed)?;
            through = Some(latest);
            if count == 0 || next >= latest {
                break latest;
            }
            from = next;
        };
        writing.finish(latest, true, true)
    }

    /// As `read`, of `events` with their sequences, read through `latest`.
    #[cfg(any(test, feature = "bench"))]
    pub(crate) fn of(spill: Spill, events: impl IntoIterator<Item = (u64, SessionEvent)>, latest: u64, after: u64, open: &[SessionEvent]) -> Result<Self, String> {
        let mut writing = Writing::new(spill, open)?;
        for (sequence, event) in events {
            let body = serde_json::to_vec(&event).map_err(failed)?;
            writing.push(sequence, &event, body)?;
        }
        writing.finish(latest, after == 0, false)
    }

    /// As `read_as_recorded`, of `events` with their sequences.
    #[cfg(any(test, feature = "bench"))]
    pub(crate) fn of_recorded(spill: Spill, events: impl IntoIterator<Item = (u64, SessionEvent)>) -> Result<Self, String> {
        let mut writing = Writing::new(spill, &[])?;
        let mut latest = 0;
        for (sequence, event) in events {
            let body = serde_json::to_vec(&event).map_err(failed)?;
            writing.push(sequence, &event, body)?;
            latest = sequence;
        }
        writing.finish(latest, true, true)
    }

    /// The view the history of the World's record alone, as
    /// `talk::recorded_history` and `talk::merged` make it: those kept, each
    /// turn where it began — its start, the words it heard, the rest in the
    /// order the record holds it, its end — and what belongs to no turn at
    /// its own time; those that tell a change of history left out.
    pub(crate) fn view_as_recorded(&mut self) -> Result<(), String> {
        if !self.as_recorded {
            return Err(failed("a history of the World's record alone of one not read as it was recorded"));
        }
        let db = self.spill.connection()?;
        let viewed = fastctx_platform::roles::in_place("keeping the World's record on disk", || -> Result<usize, fastctx_storage::rusqlite::Error> {
            let transaction = db.unchecked_transaction()?;
            transaction.execute("DELETE FROM view", [])?;
            let viewed = transaction.execute(
                "INSERT INTO view (pos, n)
                   SELECT row_number() OVER (ORDER BY o1, o2, o3, o4, o5, n) - 1, n FROM (
                     SELECT w.n AS n, w.changes AS changes,
                       CASE WHEN b.turn IS NULL THEN w.sort_second ELSE b.second END AS o1,
                       CASE WHEN b.turn IS NULL THEN w.sort_nanosecond ELSE b.nanosecond END AS o2,
                       CASE WHEN b.turn IS NULL THEN 2 ELSE w.phase END AS o3,
                       w.sort_second AS o4, w.sort_nanosecond AS o5
                     FROM world w LEFT JOIN began b ON b.turn = w.turn
                     WHERE w.kept = 1)
                   WHERE changes = 0",
                [],
            )?;
            transaction.commit()?;
            Ok(viewed)
        })
        .map_err(failed)?;
        self.viewed = viewed;
        Ok(())
    }

    /// Each event of the view in its order, none held past `each`.
    pub(crate) fn each_event(&self, each: &mut dyn FnMut(SessionEvent) -> Result<(), String>) -> Result<(), String> {
        self.each_viewed(&mut |_, event| each(event))
    }

    /// The latest sequence when the reading began.
    pub(crate) fn latest(&self) -> u64 {
        self.latest
    }

    /// Whether a cut took turns back among those read.
    pub(crate) fn cut(&self) -> bool {
        self.cut
    }

    fn db(&self) -> Result<&Connection, String> {
        self.spill.connection()
    }

    /// Of the events read (not those open) not heard before — those past
    /// the sequence `heard` — whether one put a host's history in place of
    /// another's, and whether one is of a time before `settled`.
    pub(crate) fn heard_since(&self, heard: Option<u64>, settled: Option<Timestamp>) -> Result<(bool, bool), String> {
        let after = heard.map(ordered);
        let db = self.db()?;
        fastctx_platform::roles::in_place("reading the World's record kept on disk", || -> Result<(bool, bool), fastctx_storage::rusqlite::Error> {
            let replaced: bool = db.query_row(
                "SELECT EXISTS (SELECT 1 FROM world WHERE n > ?1 AND cut = 0 AND (?2 IS NULL OR sequence > ?2) AND kind = ?3)",
                params![self.open, after, REPLACED],
                |row| row.get(0),
            )?;
            let late = match settled {
                Some(settled) => {
                    let (second, nanosecond) = parts(settled);
                    db.query_row(
                        "SELECT EXISTS (SELECT 1 FROM world WHERE n > ?1 AND cut = 0 AND (?2 IS NULL OR sequence > ?2) AND (second, nanosecond) < (?3, ?4))",
                        params![self.open, after, second, nanosecond],
                        |row| row.get(0),
                    )?
                }
                None => false,
            };
            Ok((replaced, late))
        })
        .map_err(failed)
    }

    /// Each turn's start and its end, as `claude::history::turns` gives them
    /// of what it records.
    pub(crate) fn turn_spans(&self) -> Vec<(Timestamp, Option<Timestamp>)> {
        let ends = self.ends_by_turn();
        self.starts.iter().map(|start| (start.at, ends.get(start.turn.as_str()).copied())).collect()
    }

    /// The turn a host's event later than all it records is placed in
    /// (`placement::decide`): of the turns under way, the last begun — the
    /// later told of those begun at one moment; none where none is.
    pub(crate) fn running(&self) -> Option<String> {
        let ends = self.ends_by_turn();
        self.starts.iter().filter(|start| !ends.contains_key(start.turn.as_str())).max_by_key(|start| start.at).map(|start| start.turn.clone())
    }

    /// Each turn's end: the last told of it.
    fn ends_by_turn(&self) -> HashMap<&str, Timestamp> {
        let mut ends = HashMap::new();
        for (at, turn) in &self.ends {
            ends.insert(turn.as_str(), *at);
        }
        ends
    }

    /// The turns as `placement::Turns::of` makes them of what it records,
    /// the words heard in each asked of `heard` (`World::heard_in`).
    pub(crate) fn turns<'a>(&self, heard: &'a dyn Fn(&str, &str) -> bool) -> crate::claude::history::placement::Turns<'a> {
        let ends = self.ends_by_turn();
        let turns = self.starts.iter().map(|start| (start.at, ends.get(start.turn.as_str()).copied(), start.turn.clone())).collect();
        crate::claude::history::placement::Turns::kept(turns, heard)
    }

    /// Whether the World heard `text` in `turn`, as `placement::Turns` asks:
    /// those of its length and digest read back and told by their words.
    pub(crate) fn heard_in(&self, turn: &str, text: &str) -> Result<bool, String> {
        let Some(said) = self.heard.get(turn).and_then(|said| said.get(&(text.len(), digest(text)))) else { return Ok(false) };
        for n in said {
            if let Happened::Heard { text: heard, .. } = self.event(*n)?.happened
                && heard == text
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The event of the row `n`.
    fn event(&self, n: i64) -> Result<SessionEvent, String> {
        let db = self.db()?;
        let body: Vec<u8> = fastctx_platform::roles::in_place("reading the World's record kept on disk", || {
            db.prepare_cached("SELECT body FROM world WHERE n = ?1")?.query_row(params![n], |row| row.get(0))
        })
        .map_err(failed)?;
        decoded(&body)
    }

    /// The view all it records, or those from `since` on: numbered afresh
    /// in their order.
    pub(crate) fn view(&mut self, since: Option<Timestamp>) -> Result<(), String> {
        let db = self.spill.connection()?;
        let viewed = fastctx_platform::roles::in_place("keeping the World's record on disk", || -> Result<usize, fastctx_storage::rusqlite::Error> {
            let transaction = db.unchecked_transaction()?;
            transaction.execute("DELETE FROM view", [])?;
            let viewed = match since {
                Some(since) => {
                    let (second, nanosecond) = parts(since);
                    transaction.execute(
                        "INSERT INTO view (pos, n) SELECT row_number() OVER (ORDER BY n) - 1, n FROM world WHERE cut = 0 AND (second, nanosecond) >= (?1, ?2)",
                        params![second, nanosecond],
                    )?
                }
                None => transaction.execute("INSERT INTO view (pos, n) SELECT row_number() OVER (ORDER BY n) - 1, n FROM world WHERE cut = 0", [])?,
            };
            transaction.commit()?;
            Ok(viewed)
        })
        .map_err(failed)?;
        self.viewed = viewed;
        Ok(())
    }

    /// The view as `codex::watch::recorded_beside` leaves those from `since`
    /// on, of a host that has the turns `hosted` tells: requests and what
    /// resolved them left out, and before the last time its history was put
    /// in place of another's, the turns the host has none of.
    pub(crate) fn view_beside(&mut self, since: Option<Timestamp>, hosted: &dyn Fn(&str) -> bool) -> Result<(), String> {
        self.view(since)?;
        let db = self.spill.connection()?;
        let viewed = fastctx_platform::roles::in_place("keeping the World's record on disk", || -> Result<usize, fastctx_storage::rusqlite::Error> {
            let checkpoint: Option<i64> =
                db.query_row("SELECT MAX(v.pos) FROM view v JOIN world w ON w.n = v.n WHERE w.kind = ?1", params![REPLACED], |row| row.get(0))?;
            // The turns before it, gathered once; then those the host has
            // let go, a batch at a time along their names.
            db.execute("DELETE FROM removed", [])?;
            if let Some(checkpoint) = checkpoint {
                db.execute(
                    "INSERT INTO removed (turn) SELECT DISTINCT w.turn FROM view v JOIN world w ON w.n = v.n WHERE v.pos < ?1 AND w.turn IS NOT NULL",
                    params![checkpoint],
                )?;
                // Sought along its key: the first page, then each after the
                // last of the one before (a plain bound, `reads::seek`).
                let mut last: Option<String> = None;
                loop {
                    let turns: Vec<String> = match &last {
                        None => db.prepare_cached("SELECT turn FROM removed ORDER BY turn LIMIT ?1")?.query_map(params![AT_ONCE], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>(),
                        Some(last) => db
                            .prepare_cached("SELECT turn FROM removed WHERE turn > ?1 ORDER BY turn LIMIT ?2")?
                            .query_map(params![last, AT_ONCE], |row| row.get::<_, String>(0))?
                            .collect::<Result<Vec<_>, _>>(),
                    }?;
                    let Some(next) = turns.last().cloned() else { break };
                    let transaction = db.unchecked_transaction()?;
                    for turn in turns.iter().filter(|turn| hosted(turn)) {
                        transaction.prepare_cached("DELETE FROM removed WHERE turn = ?1")?.execute(params![turn])?;
                    }
                    transaction.commit()?;
                    last = Some(next);
                }
            }
            let transaction = db.unchecked_transaction()?;
            transaction.execute_batch(
                "CREATE TEMP TABLE beside AS SELECT v.n AS n FROM view v JOIN world w ON w.n = v.n
                   WHERE w.kind != 5 /* ASKED */ AND (w.turn IS NULL OR w.turn NOT IN (SELECT turn FROM removed)) ORDER BY v.pos;
                 DELETE FROM view;",
            )?;
            let viewed = transaction.execute("INSERT INTO view (pos, n) SELECT row_number() OVER (ORDER BY rowid) - 1, n FROM beside", [])?;
            transaction.execute_batch("DROP TABLE beside; DELETE FROM removed;")?;
            transaction.commit()?;
            Ok(viewed)
        })
        .map_err(failed)?;
        self.viewed = viewed;
        Ok(())
    }

    /// Each event of the view in its order, with its row: read back a batch
    /// at a time (at most `AT_ONCE`, about `READ_BYTES` of them), each batch
    /// handed on once its reading is done — so `each` may write to the store
    /// — and none held past it.
    fn each_viewed(&self, each: &mut dyn FnMut(i64, SessionEvent) -> Result<(), String>) -> Result<(), String> {
        let db = self.db()?;
        let mut from = 0i64;
        loop {
            let batch = fastctx_platform::roles::in_place("reading the World's record kept on disk", || -> Result<Vec<(i64, i64, Vec<u8>)>, String> {
                let mut query = db
                    .prepare_cached("SELECT v.pos, v.n, w.body FROM view v JOIN world w ON w.n = v.n WHERE v.pos >= ?1 ORDER BY v.pos LIMIT ?2")
                    .map_err(failed)?;
                let mut rows = query.query(params![from, AT_ONCE]).map_err(failed)?;
                let (mut batch, mut bytes) = (Vec::new(), 0usize);
                while bytes < READ_BYTES
                    && let Some(row) = rows.next().map_err(failed)?
                {
                    let body: Vec<u8> = row.get(2).map_err(failed)?;
                    bytes += body.len();
                    batch.push((row.get(0).map_err(failed)?, row.get(1).map_err(failed)?, body));
                }
                Ok(batch)
            })?;
            let Some((last, ..)) = batch.last() else { return Ok(()) };
            from = last + 1;
            for (_, n, body) in batch {
                each(n, decoded(&body)?)?;
            }
        }
    }

    /// The events of the view, held: for a reading that holds them all.
    pub(crate) fn events(&self) -> Result<Vec<SessionEvent>, String> {
        let mut events = Vec::with_capacity(self.viewed);
        self.each_viewed(&mut |_, event| {
            events.push(event);
            Ok(())
        })?;
        Ok(events)
    }

    /// The names the view's events name, numbered and closed (as
    /// `merging::Names::over` numbers them), and the turns words were heard
    /// in among them; each one's key (`merging::key`) kept for the merge
    /// (`keys`).
    pub(crate) fn name(&self) -> Result<(Names, HashSet<String>), String> {
        let (mut names, mut sent) = (Names::default(), HashSet::new());
        let mut keyed: Vec<(i64, Vec<u8>)> = Vec::new();
        let db = self.db()?;
        let write = |keyed: &mut Vec<(i64, Vec<u8>)>| -> Result<(), String> {
            let keyed = std::mem::take(keyed);
            fastctx_platform::roles::in_place("keeping the World's record on disk", || -> Result<(), fastctx_storage::rusqlite::Error> {
                let transaction = db.unchecked_transaction()?;
                for (n, key) in keyed {
                    transaction.prepare_cached("INSERT OR REPLACE INTO world_key (n, key) VALUES (?1, ?2)")?.execute(params![n, key])?;
                }
                transaction.commit()
            })
            .map_err(failed)
        };
        // Written a batch at a time, between the batches read
        // (`each_viewed`).
        self.each_viewed(&mut |n, event| {
            if let Happened::Heard { turn: Some(turn), .. } = &event.happened {
                sent.insert(turn.clone());
            }
            keyed.push((n, pack_key(&merging::key(&event, &mut names))));
            if keyed.len() >= AT_ONCE as usize {
                write(&mut keyed)?;
            }
            Ok(())
        })?;
        write(&mut keyed)?;
        names.close();
        Ok((names, sent))
    }

    /// The keys of the view's events, as `name` kept them, for the merge.
    pub(crate) fn keys(&self) -> ViewKeys<'_> {
        ViewKeys(self)
    }

    /// Where the World's record is read through once the events before
    /// `settled` are settled, as `timeline::tail::world_after` finds it of
    /// the events read (not those open) and `open`, the starts of the turns
    /// open before; `before` where it was read through.
    pub(crate) fn world_after(&self, open: Vec<SessionEvent>, settled: Timestamp, before: Option<u64>) -> Result<(u64, Vec<SessionEvent>), String> {
        let (second, nanosecond) = parts(settled);
        let db = self.db()?;
        let first: Option<i64> = fastctx_platform::roles::in_place("reading the World's record kept on disk", || {
            db.query_row(
                "SELECT MIN(sequence) FROM world WHERE n > ?1 AND cut = 0 AND (second, nanosecond) >= (?2, ?3)",
                params![self.open, second, nanosecond],
                |row| row.get(0),
            )
        })
        .map_err(failed)?;
        let through = first.map_or_else(|| self.latest.max(before.unwrap_or(0)), |sequence| sequence_of(sequence).saturating_sub(1));
        let ended: HashSet<&str> = self.ends.iter().filter(|(at, _)| *at <= settled).map(|(_, turn)| turn.as_str()).collect();
        let started = open.into_iter().chain(
            self.starts
                .iter()
                .filter(|start| start.n > self.open && start.sequence <= through)
                .map(|start| SessionEvent { at: start.at, happened: Happened::TurnStarted { turn: start.turn.clone() } }),
        );
        let open = started.filter(|event| matches!(&event.happened, Happened::TurnStarted { turn } if !ended.contains(turn.as_str()))).collect();
        Ok((through, open))
    }

    /// The history laid out as `slots` give it, each place's event handed to
    /// `each` in turn: a host's from `hosted`, the World's read back from
    /// the store. The places are written to the store first and let go, so
    /// that what laid them out is let go before the events are made; then
    /// read back in their order with the World's events beside them.
    pub(crate) fn apply<E>(
        &self,
        slots: impl Iterator<Item = merging::Slot>,
        hosted: &mut dyn FnMut(usize) -> Result<SessionEvent, E>,
        each: &mut dyn FnMut(SessionEvent) -> Result<(), E>,
        failure: fn(String) -> E,
    ) -> Result<(), E> {
        self.write_slots(slots).map_err(failure)?;
        let db = self.db().map_err(failure)?;
        let mut query = db
            .prepare(
                "SELECT s.base, s.over, w.body FROM slot s
                   LEFT JOIN view v ON (s.base & 1) = 1 AND v.pos = (s.base >> 1)
                   LEFT JOIN world w ON w.n = v.n
                 ORDER BY s.seq",
            )
            .map_err(|error| failure(failed(error)))?;
        let mut rows = query.query([]).map_err(|error| failure(failed(error)))?;
        while let Some(row) = rows.next().map_err(|error| failure(failed(error)))? {
            let base: i64 = row.get(0).map_err(|error| failure(failed(error)))?;
            let over: Option<Vec<u8>> = row.get(1).map_err(|error| failure(failed(error)))?;
            let body: Option<Vec<u8>> = row.get(2).map_err(|error| failure(failed(error)))?;
            let mut event = if base & 1 == 0 {
                hosted((base >> 1) as usize)?
            } else {
                decoded(&body.ok_or_else(|| failure(failed("an event of the view not kept")))?).map_err(failure)?
            };
            for at in over.unwrap_or_default().chunks_exact(4) {
                let at = u32::from_le_bytes(at.try_into().expect("four bytes"));
                let mut next = self.viewed_event(at as usize).map_err(failure)?;
                crate::user_images::merge(&mut next, &event);
                event = next;
            }
            each(event)?;
        }
        Ok(())
    }

    /// The event at `pos` of the view.
    fn viewed_event(&self, pos: usize) -> Result<SessionEvent, String> {
        let db = self.db()?;
        let body: Vec<u8> = fastctx_platform::roles::in_place("reading the World's record kept on disk", || {
            db.prepare_cached("SELECT w.body FROM view v JOIN world w ON w.n = v.n WHERE v.pos = ?1")?.query_row(params![pos as i64], |row| row.get(0))
        })
        .map_err(failed)?;
        decoded(&body)
    }

    fn write_slots(&self, slots: impl Iterator<Item = merging::Slot>) -> Result<(), String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping the World's record on disk", || -> Result<(), String> {
            db.execute("DELETE FROM slot", []).map_err(failed)?;
            let mut slots = slots.peekable();
            let mut seq = 0i64;
            while slots.peek().is_some() {
                let transaction = db.unchecked_transaction().map_err(failed)?;
                {
                    let mut insert = transaction.prepare_cached("INSERT INTO slot (seq, base, over) VALUES (?1, ?2, ?3)").map_err(failed)?;
                    for slot in slots.by_ref().take(64 * 1024) {
                        let base = match slot.base {
                            merging::Base::Hosted(index) => (index as i64) << 1,
                            merging::Base::Recorded(at) => ((at as i64) << 1) | 1,
                        };
                        let over = (!slot.over.is_empty())
                            .then(|| slot.over.iter().flat_map(|at| u32::try_from(*at).expect("fewer events than numbers").to_le_bytes()).collect::<Vec<u8>>());
                        insert.execute(params![seq, base, over]).map_err(failed)?;
                        seq += 1;
                    }
                }
                transaction.commit().map_err(failed)?;
            }
            Ok(())
        })
    }
}

/// The keys of a view's events (`World::keys`), read back in their order.
pub(crate) struct ViewKeys<'w>(&'w World);

impl merging::sparse::Keys for ViewKeys<'_> {
    fn len(&self) -> usize {
        self.0.viewed
    }

    fn each(&self, each: &mut dyn FnMut(usize, &Key) -> Result<(), String>) -> Result<(), String> {
        let db = self.0.db()?;
        let mut count = 0usize;
        fastctx_platform::roles::in_place("reading the World's record kept on disk", || -> Result<(), String> {
            let mut query = db.prepare("SELECT v.pos, k.key FROM view v JOIN world_key k ON k.n = v.n ORDER BY v.pos").map_err(failed)?;
            let mut rows = query.query([]).map_err(failed)?;
            while let Some(row) = rows.next().map_err(failed)? {
                let pos: i64 = row.get(0).map_err(failed)?;
                let key: Vec<u8> = row.get(1).map_err(failed)?;
                if pos as usize != count {
                    return Err(failed(format!("the key of event {count} of the view was not kept")));
                }
                each(count, &unpack_key(&key)?)?;
                count += 1;
            }
            Ok(())
        })?;
        if count != self.0.viewed {
            return Err(failed(format!("{} keys kept of {} events viewed", count, self.0.viewed)));
        }
        Ok(())
    }
}

/// No name, in a packed key.
const NONE: u32 = u32::MAX;

const KINDS: [Kind; 7] = [Kind::TurnStarted, Kind::TurnEnded, Kind::Heard, Kind::Finished, Kind::Updated, Kind::Realtime, Kind::Other];

/// A key as the store keeps it: its time, kind, names (`NONE` for none),
/// voice, whether native and whether it stays.
const KEY_BYTES: usize = 43;

fn pack_key(key: &Key) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(KEY_BYTES);
    let (second, nanosecond) = parts(key.at);
    bytes.extend(second.to_le_bytes());
    bytes.extend(nanosecond.to_le_bytes());
    bytes.push(KINDS.iter().position(|kind| *kind == key.kind).expect("a kind") as u8);
    for name in [key.turn, key.written_turn, key.message, key.item, key.draft] {
        bytes.extend(name.unwrap_or(NONE).to_le_bytes());
    }
    match key.realtime {
        Some((complete, item, session)) => {
            bytes.push(1 | if complete { 2 } else { 0 });
            bytes.extend(item.to_le_bytes());
            bytes.extend(session.to_le_bytes());
        }
        None => bytes.extend([0; 9]),
    }
    bytes.push(u8::from(key.native) | if key.in_history { 2 } else { 0 });
    bytes
}

fn unpack_key(bytes: &[u8]) -> Result<Key, String> {
    if bytes.len() != KEY_BYTES {
        return Err(failed("a key kept of another length"));
    }
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    let name = |at: usize| Some(u32_at(at)).filter(|name| *name != NONE);
    let second = i64::from_le_bytes(bytes[0..8].try_into().expect("eight bytes"));
    let nanosecond = i32::from_le_bytes(bytes[8..12].try_into().expect("four bytes"));
    let kind = *KINDS.get(bytes[12] as usize).ok_or_else(|| failed("a key of no kind"))?;
    let realtime = (bytes[33] & 1 != 0).then(|| (bytes[33] & 2 != 0, u32_at(34), u32_at(38)));
    Ok(Key {
        at: Timestamp::new(second, nanosecond).map_err(failed)?,
        kind,
        turn: name(13),
        written_turn: name(17),
        message: name(21),
        item: name(25),
        draft: name(29),
        realtime,
        native: bytes[42] & 1 != 0,
        in_history: bytes[42] & 2 != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::talk::merging::{HostKeys, sparse};
    use serde_json::json;

    struct Choices(u64);
    impl Choices {
        fn pick(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    const TURNS: [&str; 5] = ["t0", "t1", "t2", "t3", ""];
    const TEXTS: [&str; 4] = ["hello", "hello there", "go on", "x"];

    fn at_of(choices: &mut Choices) -> String {
        at(choices)
    }

    fn at(choices: &mut Choices) -> String {
        let second = 1_800_000_000 + choices.pick(60) as i64;
        let nanosecond = if choices.pick(3) == 0 { choices.pick(1_000) as i32 * 1_000_000 } else { 0 };
        Timestamp::new(second, nanosecond).unwrap().to_string()
    }

    /// An event of the World's record of any kind the readings tell apart.
    fn event(choices: &mut Choices) -> SessionEvent {
        let at = at(choices);
        let turn = TURNS[choices.pick(TURNS.len() as u64) as usize];
        let maybe = if choices.pick(4) == 0 { json!(null) } else { json!(turn) };
        let value = match choices.pick(13) {
            0 | 1 => json!({"at": at, "event": "turn_started", "turn": turn}),
            2 => json!({"at": at, "event": "turn_ended", "turn": turn, "end": {"how": "completed", "reason": null, "duration_ms": 1, "last_message": null}}),
            3 | 4 => {
                let text = TEXTS[choices.pick(TEXTS.len() as u64) as usize];
                let message = if choices.pick(2) == 0 {
                    json!({"id": format!("m{}", choices.pick(3)), "client_id": null, "content": [], "hook_fragments": []})
                } else {
                    json!(null)
                };
                json!({"at": at, "event": "heard", "delivery": null, "turn": maybe, "from": {"kind": "user", "machine": "m"}, "text": text, "message": message})
            }
            5 => json!({"at": at, "event": "changed", "turn": maybe, "change": {"kind": "usage", "usage": {}}}),
            6 => json!({"at": at, "event": "changed", "turn": maybe, "change": {"kind": "model_request", "step": 0, "model": "m", "effort": "high"}}),
            7 => json!({"at": at, "event": "changed", "turn": maybe, "change": {"kind": "history_changed"}}),
            8 => json!({"at": at, "event": "changed", "turn": maybe,
                        "change": {"kind": "history_cut", "removed_turns": [TURNS[choices.pick(4) as usize]]}}),
            9 => json!({"at": at, "event": "changed", "turn": maybe, "change": {"kind": "request", "id": 1, "request": "r", "params": {}}}),
            10 => json!({"at": at, "event": "changed", "turn": maybe, "change": {"kind": "resolved", "id": 1, "resolution": "done"}}),
            11 => {
                let started = if choices.pick(2) == 0 { json!(at_of(choices)) } else { json!(null) };
                json!({"at": at, "event": "finished", "turn": turn, "item": {"id": format!("i{}", choices.pick(3)), "started_at": started, "kind": "message", "text": "done"}})
            }
            _ => json!({"at": at, "event": "context", "used": 1, "window": 2}),
        };
        serde_json::from_value(value).expect("an event of the World's record")
    }

    fn keyed(key: &Key) -> Vec<u8> {
        pack_key(key)
    }

    /// The World's record kept on disk gives what the record held gives:
    /// what it cut, whether what was not heard before was late or replaced
    /// the host's history, the turns and the words heard in them, each
    /// view (all, from a time, beside a Codex host), the names and keys
    /// of a view, where it is read through, and a merge laid out with a
    /// host's events (design §28.51 ⑥-1).
    #[test]
    fn a_world_record_kept_on_disk_is_the_record_held() {
        let root = std::env::temp_dir().join(format!("fastctx-world-kept-{}", std::process::id()));
        let mut choices = Choices(0x2b99_2ddf_a232_49d6);
        for round in 0..150 {
            let count = choices.pick(if round % 25 == 0 { 400 } else { 40 });
            // Sequences of all 64 bits, the highest among them in some rounds.
            let high = if round % 3 == 0 { (1u64 << 63) - count / 2 } else { 0 };
            let events: Vec<(u64, SessionEvent)> = (1..=count).map(|sequence| (high + sequence, event(&mut choices))).collect();
            let latest = high + count + choices.pick(3);
            let after = if choices.pick(3) == 0 { 0 } else { high + choices.pick(count + 1) };
            let open: Vec<SessionEvent> = if after > 0 && choices.pick(2) == 0 {
                (0..choices.pick(3)).map(|_| {
                    let turn = TURNS[choices.pick(4) as usize];
                    serde_json::from_value(json!({"at": at(&mut choices), "event": "turn_started", "turn": turn})).unwrap()
                }).collect()
            } else {
                Vec::new()
            };
            // As the record was read whole (`talk::recorded_after`, before
            // ⑥-1) and `produce` held it.
            let mut numbered: Vec<(u64, SessionEvent)> = events.iter().filter(|(sequence, _)| *sequence > after).cloned().collect();
            let removed: HashSet<String> = numbered
                .iter()
                .flat_map(|(_, event)| match &event.happened {
                    Happened::Changed { change: SessionChange::HistoryCut { removed_turns }, .. } => removed_turns.clone(),
                    _ => Vec::new(),
                })
                .collect();
            if after == 0 {
                numbered.retain(|(_, event)| crate::talk::turn_of(&event.happened).is_none_or(|turn| !removed.contains(turn)));
            }
            let recorded: Vec<SessionEvent> = open.iter().cloned().chain(numbered.iter().map(|(_, event)| event.clone())).collect();
            let given = events.iter().filter(|(sequence, _)| *sequence > after).cloned();
            let mut world = World::of(Spill::new(root.clone(), "world-test"), given, latest, after, &open).unwrap();
            assert_eq!(world.cut(), !removed.is_empty(), "round {round}");
            assert_eq!(world.latest(), latest);
            // Heard since, and late.
            let heard = (choices.pick(2) == 0).then(|| high + choices.pick(count + 1));
            let settled = (choices.pick(2) == 0).then(|| Timestamp::new(1_800_000_000 + choices.pick(60) as i64, 0).unwrap());
            let fresh = |sequence: &u64| heard.is_none_or(|heard| *sequence > heard);
            let replaced = numbered.iter().any(|(sequence, event)| fresh(sequence) && matches!(event.happened, Happened::Changed { change: SessionChange::HistoryChanged, .. }));
            let late = numbered.iter().any(|(sequence, event)| fresh(sequence) && settled.is_some_and(|settled| event.at < settled));
            assert_eq!(world.heard_since(heard, settled).unwrap(), (replaced, late), "round {round}");
            // Turns.
            assert_eq!(world.turn_spans(), crate::claude::history::turns(&recorded), "round {round}");
            let asked = |turn: &str, text: &str| world.heard_in(turn, text).unwrap();
            let kept = world.turns(&asked);
            let held = crate::claude::history::placement::Turns::of(&recorded);
            assert_eq!(kept.spans(), held.spans(), "round {round}");
            for turn in TURNS {
                for text in TEXTS {
                    assert_eq!(kept.heard(turn, text), held.heard(turn, text), "round {round}: {turn} {text}");
                }
            }
            drop(kept);
            // Views.
            let since = (choices.pick(2) == 0).then(|| Timestamp::new(1_800_000_000 + choices.pick(60) as i64, 0).unwrap());
            let shown = |events: &[SessionEvent]| events.iter().map(|event| serde_json::to_value(event).unwrap()).collect::<Vec<_>>();
            world.view(None).unwrap();
            assert_eq!(shown(&world.events().unwrap()), shown(&recorded), "round {round}");
            let from: Vec<SessionEvent> = recorded.iter().filter(|event| since.is_none_or(|since| event.at >= since)).cloned().collect();
            let hosted: HashSet<&str> = TURNS.iter().copied().filter(|_| choices.pick(2) == 0).collect();
            world.view_beside(since, &|turn| hosted.contains(turn)).unwrap();
            let beside = crate::codex::watch::recorded_beside(from.clone(), &|turn| hosted.contains(turn));
            assert_eq!(shown(&world.events().unwrap()), shown(&beside), "round {round}");
            world.view(since).unwrap();
            assert_eq!(shown(&world.events().unwrap()), shown(&from), "round {round}");
            // Names and keys of the view.
            let (mut names, sent) = world.name().unwrap();
            let mut over = merging::Names::over(&from);
            let held_sent: HashSet<String> = from
                .iter()
                .filter_map(|event| match &event.happened {
                    Happened::Heard { turn: Some(turn), .. } => Some(turn.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(sent, held_sent, "round {round}");
            for name in TURNS.iter().map(|turn| turn.to_string()).chain(["m0", "m1", "m2", "elsewhere"].map(String::from)) {
                assert_eq!(names.of(&name), over.of(&name), "round {round}: {name}");
            }
            let mut kept_keys = Vec::new();
            sparse::Keys::each(&world.keys(), &mut |at, key| {
                assert_eq!(at, kept_keys.len());
                kept_keys.push(keyed(key));
                Ok(())
            })
            .unwrap();
            let held_keys: Vec<Vec<u8>> = from.iter().map(|event| keyed(&merging::key(event, &mut over))).collect();
            assert_eq!(kept_keys, held_keys, "round {round}");
            // Where it is read through.
            let settled = Timestamp::new(1_800_000_000 + choices.pick(60) as i64, 0).unwrap();
            let before = (choices.pick(2) == 0).then(|| high + choices.pick(count + 1));
            let (kept_through, kept_open) = world.world_after(open.clone(), settled, before).unwrap();
            let (held_through, held_open) = super::super::tail::world_after(&numbered, open.clone(), latest, settled, before);
            assert_eq!((kept_through, shown(&kept_open)), (held_through, shown(&held_open)), "round {round}");
            // A merge with a host's events, laid out from the store and from
            // the events held.
            let hosted_events: Vec<SessionEvent> = (0..choices.pick(30)).map(|_| event(&mut choices)).collect();
            let host_keys = || {
                let mut names = names.clone();
                let mut keys = HostKeys::default();
                for event in &hosted_events {
                    keys.push(&merging::key(event, &mut names));
                }
                keys
            };
            let codex = choices.pick(2) == 0;
            let agent = if codex { "codex" } else { "claude" };
            let mut laid = Vec::new();
            crate::talk::assemble_each(None, agent, crate::talk::Recorded::Kept(&world), host_keys(), &mut names.clone(),
                &mut |index| Ok(hosted_events[index].clone()), &mut |event| {
                    laid.push(serde_json::to_value(&event).unwrap());
                    Ok(())
                })
            .unwrap();
            let mut was = Vec::new();
            crate::talk::assemble_each(None, agent, crate::talk::Recorded::Held(from.clone()), host_keys(), &mut names.clone(),
                &mut |index| Ok(hosted_events[index].clone()), &mut |event| {
                    was.push(serde_json::to_value(&event).unwrap());
                    Ok(())
                })
            .unwrap();
            assert_eq!(laid, was, "round {round}");
            // As it was recorded, a history of it alone.
            let mut alone = World::of_recorded(Spill::new(root.clone(), "world-test"), events.iter().cloned()).unwrap();
            alone.view_as_recorded().unwrap();
            let held = crate::talk::merged(events.iter().map(|(_, event)| event.clone()).collect(), None, agent, &fastctx_types::session::attachments::NoReceipts);
            assert_eq!(shown(&alone.events().unwrap()), shown(&held), "round {round}");
        }
        let _ = fastctx_platform::disk::remove_dir_all(&root);
    }
}
