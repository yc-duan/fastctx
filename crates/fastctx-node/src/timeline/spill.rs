//! A whole reading of a history kept on disk while it is placed in the
//! timeline, rather than held whole in memory (design §28.51): a private
//! directory for each reading, with one SQLite file in it that keeps no
//! promise to last — no journal, nothing waited for on the disk — taken
//! away whole when the reading ends, however it ends; what a node stopped
//! short left is taken away when the next one opens the asset library, and
//! before a new holder of the machine is admitted (`assets::session`).
//!
//! What it keeps: when each item a Codex thread listed began and was done,
//! the thread's timeline entries as they were listed, the host's events made
//! of them, the history's final events with their identities and records,
//! and the timeline's items as the library has them; the rows of a Claude
//! record as a scan takes them, its lines as the file holds them (`Lines`).
//! Rows are lined up and written a bounded batch at a time; reads come back
//! a bounded batch at a time, by count and by bytes.
//!
//! What is kept as JSON values is kept exactly (`pack`): taken back, a value
//! is the value it was, a float to its last bit — read and written again as
//! JSON text, a float may come back another (serde_json's reading is not
//! exact), and a body written from a reading kept here would differ from one
//! a reading held in memory writes.

use fastctx_content::ObjectRef;
use fastctx_hosts::claude::transcript::{RowStore, VisitLine};
use fastctx_hosts::codex::follow::Timing;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use fastctx_types::session::SessionEvent;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Rows lined up before they are written: about this many bytes, or this
/// many rows.
const LINED_BYTES: usize = 4 * 1024 * 1024;
const LINED_ROWS: usize = 1_024;
/// Bodies taken back at once: about this many bytes of them as they are held
/// once read (`unpack_sized`), at least one.
const READ_BYTES: u64 = 4 * 1024 * 1024;
/// What SQLite keeps of the file in memory, in KiB.
const CACHE_KIB: i64 = 8 * 1024;

/// The tables of a reading, made again whenever it begins again.
const TABLES: &str = "
    DROP TABLE IF EXISTS timing; DROP TABLE IF EXISTS item; DROP TABLE IF EXISTS entry;
    DROP TABLE IF EXISTS hosted; DROP TABLE IF EXISTS final; DROP TABLE IF EXISTS had; DROP TABLE IF EXISTS line;
    DROP TABLE IF EXISTS block; DROP TABLE IF EXISTS member; DROP TABLE IF EXISTS raw; DROP TABLE IF EXISTS earlier;
    DROP TABLE IF EXISTS placed; DROP TABLE IF EXISTS known; DROP TABLE IF EXISTS moved; DROP TABLE IF EXISTS before;
    DROP TABLE IF EXISTS world; DROP TABLE IF EXISTS view; DROP TABLE IF EXISTS world_key; DROP TABLE IF EXISTS slot;
    DROP TABLE IF EXISTS removed; DROP TABLE IF EXISTS began;
    CREATE TABLE timing (turn TEXT NOT NULL, id TEXT NOT NULL, started INTEGER, completed INTEGER, PRIMARY KEY (turn, id)) WITHOUT ROWID;
    CREATE TABLE item (seq INTEGER PRIMARY KEY, turn TEXT NOT NULL, body BLOB NOT NULL);
    CREATE INDEX item_turn ON item (turn, seq);
    CREATE TABLE entry (seq INTEGER PRIMARY KEY, position BLOB NOT NULL, turn TEXT NOT NULL, id TEXT, body BLOB NOT NULL);
    CREATE INDEX entry_position ON entry (position, seq);
    CREATE TABLE hosted (seq INTEGER PRIMARY KEY, body BLOB NOT NULL);
    CREATE TABLE final (seq INTEGER PRIMARY KEY, identity BLOB NOT NULL, base BLOB NOT NULL, second INTEGER NOT NULL, nanosecond INTEGER NOT NULL,
                        record BLOB NOT NULL, object BLOB, size INTEGER, encoded BLOB, known TEXT, named INTEGER);
    CREATE INDEX final_identity ON final (identity);
    CREATE TABLE had (identity BLOB PRIMARY KEY, place BLOB NOT NULL, length INTEGER NOT NULL) WITHOUT ROWID;
    CREATE TABLE line (seq INTEGER PRIMARY KEY, uuid TEXT UNIQUE, number INTEGER NOT NULL, start INTEGER NOT NULL, bytes BLOB NOT NULL);
    CREATE TABLE block (seq INTEGER PRIMARY KEY, id TEXT NOT NULL, result INTEGER NOT NULL, line INTEGER NOT NULL, place INTEGER NOT NULL);
    CREATE INDEX block_id ON block (id, result, seq);
    CREATE TABLE member (kind INTEGER NOT NULL, key TEXT NOT NULL, PRIMARY KEY (kind, key)) WITHOUT ROWID;
    CREATE TABLE raw (seq INTEGER PRIMARY KEY, body BLOB NOT NULL);
    CREATE TABLE earlier (uuid TEXT PRIMARY KEY) WITHOUT ROWID;
    CREATE TABLE placed (seq INTEGER PRIMARY KEY, event INTEGER NOT NULL, turn INTEGER);
    CREATE TABLE known (key TEXT NOT NULL, record INTEGER NOT NULL, PRIMARY KEY (key, record)) WITHOUT ROWID;
    CREATE TABLE moved (digest BLOB PRIMARY KEY) WITHOUT ROWID;
    CREATE TABLE before (identity BLOB PRIMARY KEY) WITHOUT ROWID;
    CREATE TABLE world (n INTEGER PRIMARY KEY, sequence INTEGER NOT NULL, second INTEGER NOT NULL, nanosecond INTEGER NOT NULL,
                        kind INTEGER NOT NULL, turn TEXT, cut INTEGER NOT NULL DEFAULT 0, body BLOB NOT NULL,
                        kept INTEGER NOT NULL, changes INTEGER NOT NULL, phase INTEGER NOT NULL, sort_second INTEGER NOT NULL, sort_nanosecond INTEGER NOT NULL);
    CREATE TABLE began (turn TEXT PRIMARY KEY, second INTEGER NOT NULL, nanosecond INTEGER NOT NULL) WITHOUT ROWID;
    CREATE TABLE view (pos INTEGER PRIMARY KEY, n INTEGER NOT NULL);
    CREATE TABLE world_key (n INTEGER PRIMARY KEY, key BLOB NOT NULL);
    CREATE TABLE slot (seq INTEGER PRIMARY KEY, base INTEGER NOT NULL, over BLOB);
    CREATE TABLE removed (turn TEXT PRIMARY KEY) WITHOUT ROWID;
";

/// The tables of one reading of a Claude record, cleared for each (`Spill::reading`).
const READING: &str = "DELETE FROM line; DELETE FROM block; DELETE FROM member;";

/// How many readings are kept on disk now — each work store, and each
/// reading's objects being put as it is read (`assets::Early`) — and how
/// many admissions of a new holder hold new ones off: a new holder finds
/// none on the disk (`assets::session`, design §28.51).
static KEPT: std::sync::Mutex<(usize, usize)> = std::sync::Mutex::new((0, 0));

fn kept_now() -> std::sync::MutexGuard<'static, (usize, usize)> {
    KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Held while a new holder is admitted: no reading begins to be kept on
/// disk meanwhile.
pub(crate) struct HoldOff(());

impl Drop for HoldOff {
    fn drop(&mut self) {
        kept_now().1 -= 1;
    }
}

/// Holds off new readings kept on disk until it is dropped.
pub(crate) fn hold_off() -> HoldOff {
    kept_now().1 += 1;
    HoldOff(())
}

/// How many readings are kept on disk now (`KEPT`).
pub(crate) fn kept() -> usize {
    kept_now().0
}

/// A reading counted among those kept (`KEPT`) while it is held.
pub(crate) struct Entered(());

impl Drop for Entered {
    fn drop(&mut self) {
        kept_now().0 -= 1;
    }
}

/// Counts a reading among those kept, unless a new holder is being admitted.
pub(crate) fn enter() -> Option<Entered> {
    let mut kept = kept_now();
    if kept.1 > 0 {
        return None;
    }
    kept.0 += 1;
    Some(Entered(()))
}

/// `value` as bytes that give it back exactly (`unpack`): each number as it
/// is held — unsigned, signed or a float's bits — strings and keys as they
/// are, objects in their order.
pub(crate) fn pack(value: &Value) -> Vec<u8> {
    fn length(out: &mut Vec<u8>, mut length: usize) {
        loop {
            let byte = (length & 0x7f) as u8;
            length >>= 7;
            if length == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }
    fn text(out: &mut Vec<u8>, text: &str) {
        length(out, text.len());
        out.extend_from_slice(text.as_bytes());
    }
    fn one(out: &mut Vec<u8>, value: &Value) {
        match value {
            Value::Null => out.push(0),
            Value::Bool(false) => out.push(1),
            Value::Bool(true) => out.push(2),
            Value::Number(number) => {
                if let Some(unsigned) = number.as_u64() {
                    out.push(3);
                    out.extend_from_slice(&unsigned.to_le_bytes());
                } else if let Some(signed) = number.as_i64() {
                    out.push(4);
                    out.extend_from_slice(&signed.to_le_bytes());
                } else {
                    out.push(5);
                    out.extend_from_slice(&number.as_f64().unwrap_or_default().to_bits().to_le_bytes());
                }
            }
            Value::String(string) => {
                out.push(6);
                text(out, string);
            }
            Value::Array(items) => {
                out.push(7);
                length(out, items.len());
                for item in items {
                    one(out, item);
                }
            }
            Value::Object(fields) => {
                out.push(8);
                length(out, fields.len());
                for (name, field) in fields {
                    text(out, name);
                    one(out, field);
                }
            }
        }
    }
    let mut out = Vec::new();
    one(&mut out, value);
    out
}

/// The value `pack` made `bytes` of.
#[cfg(test)]
pub(crate) fn unpack(bytes: &[u8]) -> Result<Value, String> {
    unpack_sized(bytes).map(|(value, _)| value)
}

/// What a map of `fields` fields takes held in memory beyond its fields'
/// own (`unpack_sized`'s `FIELD`): the nodes of its tree, each with room for
/// eleven fields however few it holds (a B-tree's, as `serde_json::Map`
/// keeps them) — one for a small map, at most one for each five fields of a
/// larger one, and the inner nodes above them, each with its edges.
fn map_nodes(fields: usize) -> u64 {
    const LEAF: u64 = 16 + 11 * (std::mem::size_of::<String>() + std::mem::size_of::<Value>()) as u64;
    const INNER: u64 = LEAF + 12 * std::mem::size_of::<usize>() as u64;
    match fields {
        0 => 0,
        1..=11 => LEAF,
        many => (many.div_ceil(5) + many.div_ceil(30) + 1) as u64 * INNER,
    }
}

/// The value `pack` made `bytes` of, and about how many bytes it takes held
/// in memory, no less: each value's own place, each string's bytes, each
/// field's key, and each map's nodes with their empty room (`map_nodes`) —
/// far more than its packed bytes where it holds many small values (a null
/// is one byte packed, a value's place unpacked; a map of one field, a node
/// with room for eleven).
pub(crate) fn unpack_sized(bytes: &[u8]) -> Result<(Value, u64), String> {
    const VALUE: u64 = std::mem::size_of::<Value>() as u64;
    const FIELD: u64 = std::mem::size_of::<String>() as u64 + 32;
    /// Items an array is made with room for at once; one longer grows,
    /// doubling.
    const ROOM: usize = 1 << 16;
    struct Reading<'a>(&'a [u8], u64);
    impl Reading<'_> {
        fn take(&mut self, count: usize) -> Result<&[u8], String> {
            if self.0.len() < count {
                return Err(failed("a value kept is cut short"));
            }
            let (taken, rest) = self.0.split_at(count);
            self.0 = rest;
            Ok(taken)
        }
        fn eight(&mut self) -> Result<[u8; 8], String> {
            Ok(self.take(8)?.try_into().expect("eight bytes"))
        }
        fn length(&mut self) -> Result<usize, String> {
            let (mut length, mut shift) = (0usize, 0u32);
            loop {
                let byte = self.take(1)?[0];
                length |= usize::from(byte & 0x7f).checked_shl(shift).ok_or_else(|| failed("a length kept is too long"))?;
                if byte & 0x80 == 0 {
                    return Ok(length);
                }
                shift += 7;
            }
        }
        fn text(&mut self) -> Result<String, String> {
            let length = self.length()?;
            self.1 += length as u64;
            String::from_utf8(self.take(length)?.to_vec()).map_err(failed)
        }
        fn one(&mut self) -> Result<Value, String> {
            self.1 += VALUE;
            Ok(match self.take(1)?[0] {
                0 => Value::Null,
                1 => Value::Bool(false),
                2 => Value::Bool(true),
                3 => Value::from(u64::from_le_bytes(self.eight()?)),
                4 => Value::from(i64::from_le_bytes(self.eight()?)),
                5 => serde_json::Number::from_f64(f64::from_bits(u64::from_le_bytes(self.eight()?)))
                    .map(Value::Number)
                    .ok_or_else(|| failed("a float kept is not finite"))?,
                6 => Value::String(self.text()?),
                7 => {
                    let count = self.length()?;
                    if count > ROOM {
                        // The room it doubled to and leaves unfilled.
                        self.1 += (count.next_power_of_two() - count) as u64 * VALUE;
                    }
                    let mut items = Vec::with_capacity(count.min(ROOM));
                    for _ in 0..count {
                        items.push(self.one()?);
                    }
                    Value::Array(items)
                }
                8 => {
                    let count = self.length()?;
                    self.1 += map_nodes(count);
                    let mut fields = serde_json::Map::new();
                    for _ in 0..count {
                        self.1 += FIELD;
                        let name = self.text()?;
                        fields.insert(name, self.one()?);
                    }
                    Value::Object(fields)
                }
                _ => return Err(failed("a value kept is of no kind")),
            })
        }
    }
    let mut reading = Reading(bytes, 0);
    let value = reading.one()?;
    if !reading.0.is_empty() {
        return Err(failed("a value kept runs on"));
    }
    Ok((value, reading.1))
}

/// `event` kept exactly (`pack`).
pub(crate) fn pack_event(event: &SessionEvent) -> Result<Vec<u8>, String> {
    Ok(pack(&serde_json::to_value(event).map_err(failed)?))
}

/// A row lined up to be written.
enum Row {
    Timing { turn: String, id: String, timing: Timing },
    Item { turn: String, body: Vec<u8> },
    Entry { position: Vec<u8>, turn: String, id: Option<String>, body: Vec<u8> },
    Hosted(Vec<u8>),
    Final { identity: [u8; 32], at: jiff::Timestamp, record: Vec<u8>, object: Option<(ObjectRef, Vec<u8>)>, frozen: Option<(String, bool)> },
    Had { identity: Vec<u8>, place: Vec<u8>, length: u64 },
    /// A row of a Claude record, numbered from 0 (`seq`), and a later form
    /// of one put in its place.
    Line { seq: usize, uuid: Option<String>, number: usize, start: u64, bytes: Vec<u8> },
    Relined { seq: usize, number: usize, start: u64, bytes: Vec<u8> },
    /// A tool block a projection pairs (`RowStore::note_block`).
    Block { id: String, result: bool, line: usize, place: usize },
    /// A key of a set a reading holds too many of (`RowStore::note_member`).
    Member { kind: u8, key: String },
    /// An event a Claude session's record shows, before it is placed,
    /// numbered from 0.
    Raw { seq: u32, body: Vec<u8> },
    /// Where placing put an event: its number among the history's, and the
    /// number of the World's turn it is in.
    Placed { event: u32, turn: Option<u32> },
    /// What the record stopped short numbered `record` knows an event by.
    Known { record: u32, key: String },
    /// The digest of a record id a follower's seam moved past.
    Moved(u64),
    /// An item a window's writing reads again or takes away (`push_before`).
    Before(Vec<u8>),
}

impl Row {
    fn bytes(&self) -> usize {
        64 + match self {
            Row::Line { uuid, bytes, .. } => uuid.as_ref().map_or(0, String::len) + bytes.len(),
            Row::Relined { bytes, .. } => bytes.len(),
            Row::Block { id, .. } => id.len(),
            Row::Member { key, .. } => key.len(),
            Row::Raw { body, .. } => body.len(),
            Row::Placed { .. } => 0,
            Row::Known { key, .. } => key.len(),
            Row::Moved(_) => 8,
            Row::Before(identity) => identity.len(),
            Row::Timing { turn, id, .. } => turn.len() + id.len(),
            Row::Item { turn, body } => turn.len() + body.len(),
            Row::Entry { turn, id, body, .. } => turn.len() + id.as_ref().map_or(0, String::len) + body.len(),
            Row::Hosted(body) => body.len(),
            Row::Final { record, object, .. } => record.len() + object.as_ref().map_or(0, |(_, encoded)| encoded.len()),
            Row::Had { identity, place, .. } => identity.len() + place.len(),
        }
    }
}

/// An item of the timeline no final event is: its identity and order key.
pub(crate) type Gone = (Vec<u8>, Vec<u8>);

/// A final event as kept, with the item of the same identity the timeline
/// had: its order key and the length of its body.
pub(crate) struct Final {
    pub(crate) identity: Vec<u8>,
    pub(crate) at: jiff::Timestamp,
    /// Its record's body, as the timeline holds it.
    pub(crate) record: Vec<u8>,
    /// Too large for a record: the object the whole of it is, and the event
    /// as JSON, the object's bytes.
    pub(crate) object: Option<(ObjectRef, Vec<u8>)>,
    pub(crate) had: Option<(Vec<u8>, u64)>,
}

/// One whole reading kept on disk: its directory is made when it first
/// begins, and taken away with all in it when it is dropped.
pub(crate) struct Spill {
    root: PathBuf,
    name: String,
    /// Its directory, once made.
    dir: Option<PathBuf>,
    db: Option<Connection>,
    /// Counted among those kept from when it begins until its directory is
    /// taken away.
    entered: Option<Entered>,
    lined: Vec<Row>,
    lined_bytes: usize,
    /// Where `entries` stopped: how many it gave, and the last one's place.
    mark: Option<(usize, Vec<u8>, i64)>,
    /// Where `items_of` stopped: the turn, how many of its items it gave,
    /// and the last one's number, so the next page seeks there.
    items_mark: Option<(String, usize, i64)>,
    /// When the items of the entries `entries` last gave began and were done.
    timed: HashMap<(String, String), Option<Timing>>,
    /// How many rows of a Claude record are kept; the UUIDs of those lined
    /// up and not yet written, with their numbers; and how many rows lined
    /// up write them or their later forms.
    lines: usize,
    lined_uuids: HashMap<String, usize>,
    lined_lines: usize,
    /// How many tool blocks and keys of sets are lined up, not yet written.
    lined_blocks: usize,
    /// How many events of Claude sessions are kept (`push_raw`).
    raws: u32,
}

fn failed(error: impl std::fmt::Display) -> String {
    format!("a reading kept on disk: {error}")
}

/// A place as `Keeping::entries` orders it, as bytes: none before any,
/// then each big-endian, so that bytes compare as the places do.
fn place_of(entry: &Value) -> Vec<u8> {
    entry["position"].as_u64().map(|position| position.to_be_bytes().to_vec()).unwrap_or_default()
}

impl Spill {
    /// A reading to be kept under `root` (the asset library's `spill`), in a
    /// directory named for `name` and made only when it first begins.
    pub(crate) fn new(root: PathBuf, name: &str) -> Self {
        let name = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        Self {
            root,
            name,
            dir: None,
            db: None,
            entered: None,
            lined: Vec::new(),
            lined_bytes: 0,
            mark: None,
            items_mark: None,
            timed: HashMap::new(),
            lines: 0,
            lined_uuids: HashMap::new(),
            lined_lines: 0,
            lined_blocks: 0,
            raws: 0,
        }
    }

    /// Whether a reading was kept here.
    #[cfg(feature = "bench")]
    pub(crate) fn begun(&self) -> bool {
        self.db.is_some()
    }

    /// The store begun (`begin`), for what keeps its own tables in it (the
    /// World's record, `super::world`).
    pub(super) fn connection(&self) -> Result<&Connection, String> {
        self.db.as_ref().ok_or_else(|| failed("not begun"))
    }

    /// Begins the reading, or begins it again: what was kept is let go.
    pub(crate) fn begin(&mut self) -> Result<(), String> {
        self.lined.clear();
        self.lined_bytes = 0;
        self.mark = None;
        self.items_mark = None;
        self.timed.clear();
        self.lines = 0;
        self.lined_uuids.clear();
        self.lined_lines = 0;
        self.lined_blocks = 0;
        self.raws = 0;
        if self.dir.is_none() {
            let mut random = [0u8; 8];
            getrandom::fill(&mut random).map_err(failed)?;
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let dir = self.root.join(format!("{}-{suffix}", self.name));
            self.entered = Some(enter().ok_or_else(|| failed("a new holder of this machine is being admitted; it is read again after"))?);
            // Counted, it is taken away with the reading whatever follows.
            self.dir = Some(dir.clone());
            let opened = fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Connection, String> {
                fastctx_platform::ensure_private_dir(&self.root).map_err(failed)?;
                fastctx_platform::ensure_private_dir(&dir).map_err(failed)?;
                let db = Connection::open(dir.join("reading.sqlite3")).map_err(failed)?;
                db.execute_batch(&format!(
                    "PRAGMA page_size = 16384; PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA locking_mode = EXCLUSIVE;
                     PRAGMA cache_size = -{CACHE_KIB}; PRAGMA mmap_size = 0; PRAGMA temp_store = FILE;"
                ))
                .map_err(failed)?;
                Ok(db)
            });
            self.db = Some(opened?);
        }
        let db = self.db.as_ref().ok_or_else(|| failed("not open"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || db.execute_batch(TABLES)).map_err(failed)
    }

    fn line(&mut self, row: Row) -> Result<(), String> {
        self.lined_bytes += row.bytes();
        self.lined.push(row);
        if self.lined_bytes >= LINED_BYTES || self.lined.len() >= LINED_ROWS {
            self.flush()?;
        }
        Ok(())
    }

    /// Writes the rows lined up, in one transaction.
    pub(crate) fn flush(&mut self) -> Result<(), String> {
        if self.lined.is_empty() {
            return Ok(());
        }
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        let rows = std::mem::take(&mut self.lined);
        self.lined_bytes = 0;
        // Written, or the reading fails: either way none is lined up now.
        self.lined_uuids.clear();
        self.lined_lines = 0;
        self.lined_blocks = 0;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<(), fastctx_storage::rusqlite::Error> {
            let transaction = db.unchecked_transaction()?;
            for row in rows {
                match row {
                    Row::Timing { turn, id, timing } => {
                        transaction
                            .prepare_cached("INSERT OR REPLACE INTO timing (turn, id, started, completed) VALUES (?1, ?2, ?3, ?4)")?
                            .execute(params![turn, id, timing.started, timing.completed])?;
                    }
                    Row::Item { turn, body } => {
                        transaction.prepare_cached("INSERT INTO item (turn, body) VALUES (?1, ?2)")?.execute(params![turn, body])?;
                    }
                    Row::Entry { position, turn, id, body } => {
                        transaction
                            .prepare_cached("INSERT INTO entry (position, turn, id, body) VALUES (?1, ?2, ?3, ?4)")?
                            .execute(params![position, turn, id, body])?;
                    }
                    Row::Hosted(body) => {
                        transaction.prepare_cached("INSERT INTO hosted (body) VALUES (?1)")?.execute(params![body])?;
                    }
                    Row::Final { identity, at, record, object, frozen } => {
                        let (hash, size, encoded) = match &object {
                            Some((object, encoded)) => (Some(&object.hash[..]), Some(object.size as i64), Some(encoded.as_slice())),
                            None => (None, None, None),
                        };
                        let (known, named) = match &frozen {
                            Some((known, named)) => (Some(known.as_str()), Some(*named)),
                            None => (None, None),
                        };
                        transaction
                            .prepare_cached(
                                "INSERT INTO final (identity, base, second, nanosecond, record, object, size, encoded, known, named)
                                 VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                            )?
                            .execute(params![identity.as_slice(), at.as_second(), at.subsec_nanosecond(), record, hash, size, encoded, known, named])?;
                    }
                    Row::Had { identity, place, length } => {
                        transaction
                            .prepare_cached("INSERT OR REPLACE INTO had (identity, place, length) VALUES (?1, ?2, ?3)")?
                            .execute(params![identity, place, length as i64])?;
                    }
                    Row::Line { seq, uuid, number, start, bytes } => {
                        transaction
                            .prepare_cached("INSERT INTO line (seq, uuid, number, start, bytes) VALUES (?1, ?2, ?3, ?4, ?5)")?
                            .execute(params![seq as i64, uuid, number as i64, start as i64, bytes])?;
                    }
                    Row::Relined { seq, number, start, bytes } => {
                        transaction
                            .prepare_cached("UPDATE line SET number = ?2, start = ?3, bytes = ?4 WHERE seq = ?1")?
                            .execute(params![seq as i64, number as i64, start as i64, bytes])?;
                    }
                    Row::Block { id, result, line, place } => {
                        transaction
                            .prepare_cached("INSERT INTO block (id, result, line, place) VALUES (?1, ?2, ?3, ?4)")?
                            .execute(params![id, result, line as i64, place as i64])?;
                    }
                    Row::Member { kind, key } => {
                        transaction.prepare_cached("INSERT OR IGNORE INTO member (kind, key) VALUES (?1, ?2)")?.execute(params![kind, key])?;
                    }
                    Row::Raw { seq, body } => {
                        transaction.prepare_cached("INSERT INTO raw (seq, body) VALUES (?1, ?2)")?.execute(params![seq, body])?;
                    }
                    Row::Placed { event, turn } => {
                        transaction.prepare_cached("INSERT INTO placed (event, turn) VALUES (?1, ?2)")?.execute(params![event, turn])?;
                    }
                    Row::Known { record, key } => {
                        transaction.prepare_cached("INSERT OR IGNORE INTO known (key, record) VALUES (?1, ?2)")?.execute(params![key, record])?;
                    }
                    Row::Moved(digest) => {
                        transaction.prepare_cached("INSERT OR IGNORE INTO moved (digest) VALUES (?1)")?.execute(params![digest.to_be_bytes().as_slice()])?;
                    }
                    Row::Before(identity) => {
                        transaction.prepare_cached("INSERT OR IGNORE INTO before (identity) VALUES (?1)")?.execute(params![identity])?;
                    }
                }
            }
            transaction.commit()
        })
        .map_err(failed)
    }

    fn db(&mut self) -> Result<&Connection, String> {
        self.flush()?;
        self.db.as_ref().ok_or_else(|| failed("not begun"))
    }

    /// When the item `id` of `turn` began and was done, as listed; the last
    /// listed stands.
    pub(crate) fn time(&mut self, turn: &str, id: &str, timing: Timing) -> Result<(), String> {
        self.line(Row::Timing { turn: turn.to_string(), id: id.to_string(), timing })
    }

    /// When the item `id` of `turn` began and was done, if listed: of the
    /// entries `entries` last gave, as it found them; of another, asked.
    pub(crate) fn timing(&mut self, turn: &str, id: &str) -> Result<Option<Timing>, String> {
        if let Some(timing) = self.timed.get(&(turn.to_string(), id.to_string())) {
            return Ok(*timing);
        }
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || {
            db.prepare_cached("SELECT started, completed FROM timing WHERE turn = ?1 AND id = ?2")?
                .query_row(params![turn, id], |row| Ok(Timing { started: row.get(0)?, completed: row.get(1)? }))
                .optional()
        })
        .map_err(failed)
    }

    /// An item entry kept whole, of the turn `turn` (as JSON text).
    pub(crate) fn item(&mut self, turn: &str, entry: &Value) -> Result<(), String> {
        self.line(Row::Item { turn: turn.to_string(), body: pack(entry) })
    }

    /// The item entries kept whole of the turn `turn`, in the order listed,
    /// from the `from`th on, at most `limit` and about `READ_BYTES`.
    pub(crate) fn items_of(&mut self, turn: &str, from: usize, limit: usize) -> Result<Vec<Value>, String> {
        // Going on where the last page of the same turn stopped, it seeks
        // the turn's index there: a turn's items read a page at a time are
        // read once, not again from its first at every page.
        let after = match &self.items_mark {
            Some((marked, given, seq)) if from > 0 && marked == turn && *given == from => Some(*seq),
            _ => None,
        };
        let db = self.db()?;
        let (taken, last) = fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<(Vec<Value>, Option<i64>), String> {
            let mut statement = match after {
                Some(_) => db.prepare_cached("SELECT length(body), body, seq FROM item WHERE turn = ?1 AND seq > ?3 ORDER BY seq LIMIT ?2"),
                None => db.prepare_cached("SELECT length(body), body, seq FROM item WHERE turn = ?1 ORDER BY seq LIMIT ?2 OFFSET ?3"),
            }
            .map_err(failed)?;
            let mut rows = statement.query(params![turn, limit as i64, after.unwrap_or(from as i64)]).map_err(failed)?;
            let (mut taken, mut size, mut last) = (Vec::new(), 0u64, None);
            while let Some(row) = rows.next().map_err(failed)? {
                // Its packed length, less than it takes unpacked, known
                // before its bytes are read.
                let length = row.get::<_, i64>(0).map_err(failed)?.max(0) as u64;
                if !taken.is_empty() && size + length > READ_BYTES {
                    break;
                }
                let body: Vec<u8> = row.get(1).map_err(failed)?;
                let (entry, held) = unpack_sized(&body)?;
                size += held;
                taken.push(entry);
                last = Some(row.get::<_, i64>(2).map_err(failed)?);
            }
            Ok((taken, last))
        })?;
        self.items_mark = last.map(|seq| (turn.to_string(), from + taken.len(), seq));
        Ok(taken)
    }

    /// The next timeline entry, as listed.
    pub(crate) fn entry(&mut self, entry: &Value) -> Result<(), String> {
        // Its turn and item as a projection asks for its timing.
        let turn = entry.get("turnId").and_then(Value::as_str).unwrap_or_default().to_string();
        let id = entry["item"].get("id").and_then(Value::as_str).map(str::to_string);
        self.line(Row::Entry { position: place_of(entry), turn, id, body: pack(entry) })
    }

    /// The timeline entries kept, ordered by position as a stable sort of
    /// them in the order listed has them, from the `from`th on, at most
    /// `limit` and about `READ_BYTES` (at least one while any are left);
    /// `from` goes up by what each call gave.
    pub(crate) fn entries(&mut self, from: usize, limit: usize) -> Result<Vec<Value>, String> {
        let after = match &self.mark {
            _ if from == 0 => None,
            Some((given, place, seq)) if *given == from => Some((place.clone(), *seq)),
            _ => return Err(failed(format!("entries asked for from {from}, not where they stopped"))),
        };
        let db = self.db()?;
        type Listed = (Vec<u8>, i64, Value, String, Option<String>, Option<Timing>);
        let rows = fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<Listed>, String> {
            const LISTED: &str = "SELECT e.position, e.seq, length(e.body), e.body, e.turn, e.id, t.started, t.completed, t.turn IS NOT NULL FROM entry e
                 LEFT JOIN timing t ON t.turn = e.turn AND t.id = e.id";
            // Two statements, so that the one going on from a place seeks
            // the index there rather than reading it from its start.
            let mut statement = match &after {
                None => db.prepare_cached(&format!("{LISTED} ORDER BY e.position, e.seq LIMIT ?1")),
                Some(_) => db.prepare_cached(&format!("{LISTED} WHERE (e.position, e.seq) > (?2, ?3) ORDER BY e.position, e.seq LIMIT ?1")),
            }
            .map_err(failed)?;
            let mut rows = match &after {
                None => statement.query(params![limit as i64]),
                Some((place, seq)) => statement.query(params![limit as i64, place, seq]),
            }
            .map_err(failed)?;
            let (mut taken, mut size) = (Vec::new(), 0u64);
            while let Some(row) = rows.next().map_err(failed)? {
                // Its packed length, less than it takes unpacked, known
                // before its bytes are read.
                let length = row.get::<_, i64>(2).map_err(failed)?.max(0) as u64;
                if !taken.is_empty() && size + length > READ_BYTES {
                    break;
                }
                let body: Vec<u8> = row.get(3).map_err(failed)?;
                let (entry, held) = unpack_sized(&body)?;
                size += held;
                let timed: bool = row.get(8).map_err(failed)?;
                let timing = timed
                    .then(|| Ok::<_, fastctx_storage::rusqlite::Error>(Timing { started: row.get(6)?, completed: row.get(7)? }))
                    .transpose()
                    .map_err(failed)?;
                taken.push((
                    row.get(0).map_err(failed)?,
                    row.get(1).map_err(failed)?,
                    entry,
                    row.get(4).map_err(failed)?,
                    row.get(5).map_err(failed)?,
                    timing,
                ));
            }
            Ok(taken)
        })?;
        self.timed.clear();
        let mut entries = Vec::with_capacity(rows.len());
        for (place, seq, entry, turn, id, timing) in rows {
            if let Some(id) = id {
                self.timed.insert((turn, id), timing);
            }
            entries.push(entry);
            self.mark = Some((from + entries.len(), place, seq));
        }
        Ok(entries)
    }

    /// The next of the host's events, as the history is made of them, kept
    /// exactly (`pack_event`).
    pub(crate) fn push_hosted(&mut self, packed: Vec<u8>) -> Result<(), String> {
        self.line(Row::Hosted(packed))
    }

    /// The host's `index`th event kept (from 0).
    #[cfg(test)]
    pub(crate) fn hosted(&mut self, index: usize) -> Result<SessionEvent, String> {
        let db = self.db()?;
        let body: Vec<u8> = fastctx_platform::roles::in_place("keeping a reading on disk", || {
            db.prepare_cached("SELECT body FROM hosted WHERE seq = ?1")?.query_row(params![index as i64 + 1], |row| row.get(0))
        })
        .map_err(failed)?;
        serde_json::from_value(unpack(&body)?).map_err(failed)
    }

    /// The host's events kept from the `from`th on (from 0), in their order:
    /// about `READ_BYTES` of them as they are held once read, at least one
    /// while any are left; only one, `alone`.
    pub(crate) fn hosted_from(&mut self, from: usize, alone: bool) -> Result<Vec<SessionEvent>, String> {
        let db = self.db()?;
        let values = fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<Value>, String> {
            let mut statement = db.prepare_cached("SELECT length(body), body FROM hosted WHERE seq > ?1 ORDER BY seq").map_err(failed)?;
            let mut rows = statement.query(params![from as i64]).map_err(failed)?;
            let (mut taken, mut size) = (Vec::new(), 0u64);
            while let Some(row) = rows.next().map_err(failed)? {
                let length = row.get::<_, i64>(0).map_err(failed)?.max(0) as u64;
                if !taken.is_empty() && size + length > READ_BYTES {
                    break;
                }
                let body: Vec<u8> = row.get(1).map_err(failed)?;
                let (value, held) = unpack_sized(&body)?;
                size += held;
                taken.push(value);
                if alone {
                    break;
                }
            }
            Ok(taken)
        })?;
        values.into_iter().map(|value| serde_json::from_value(value).map_err(failed)).collect()
    }

    /// The next of the history's final events, with its identity — or, to be
    /// numbered after (`number_again`), what it is known by before those
    /// alike are counted (`names::base`): its record's body and, too large
    /// for a record, its object with the event as JSON; where a record
    /// stopped short (`assets::Span::Added`), what `names::frozen` knows it
    /// by and whether it is named.
    pub(crate) fn push_final(
        &mut self,
        identity: [u8; 32],
        at: jiff::Timestamp,
        record: Vec<u8>,
        object: Option<(ObjectRef, Vec<u8>)>,
        frozen: Option<(String, bool)>,
    ) -> Result<(), String> {
        self.line(Row::Final { identity, at, record, object, frozen })
    }

    /// The final events kept by what they are known by before those alike
    /// are counted (`names::base`) given their identities, as
    /// `names::Identities` gives them one at a time: the first known by a
    /// base keeps it, each after takes its base and how many came before
    /// (`names::again`). Who comes before is decided whole first, by the
    /// bases, which are not changed; only then are identities written.
    pub(crate) fn number_again(&mut self) -> Result<(), String> {
        self.flush()?;
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<(), fastctx_storage::rusqlite::Error> {
            db.execute_batch(
                "DROP TABLE IF EXISTS temp.again; DROP TABLE IF EXISTS temp.alike;
                 CREATE TEMP TABLE alike AS SELECT base FROM final GROUP BY base HAVING COUNT(*) > 1;
                 CREATE TEMP TABLE again (seq INTEGER PRIMARY KEY, base BLOB NOT NULL, place INTEGER NOT NULL);
                 INSERT INTO temp.again SELECT seq, base, place FROM
                   (SELECT seq, base, ROW_NUMBER() OVER (PARTITION BY base ORDER BY seq) - 1 AS place FROM final
                    WHERE base IN (SELECT base FROM temp.alike)) WHERE place > 0;
                 DROP TABLE temp.alike;",
            )?;
            let mut after = 0i64;
            loop {
                let page: Vec<(i64, Vec<u8>, i64)> = {
                    let mut statement = db.prepare_cached("SELECT seq, base, place FROM temp.again WHERE seq > ?1 ORDER BY seq LIMIT 4096")?;
                    let rows = statement.query_map(params![after], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                    rows.collect::<Result<_, _>>()?
                };
                let Some(&(last, ..)) = page.last() else { break };
                let transaction = db.unchecked_transaction()?;
                for (seq, base, place) in page {
                    let base: [u8; 32] = base.as_slice().try_into().map_err(|_| fastctx_storage::rusqlite::Error::InvalidQuery)?;
                    let identity = super::names::again(base, u32::try_from(place).map_err(|_| fastctx_storage::rusqlite::Error::InvalidQuery)?);
                    transaction.prepare_cached("UPDATE final SET identity = ?1 WHERE seq = ?2")?.execute(params![identity.as_slice(), seq])?;
                }
                transaction.commit()?;
                after = last;
            }
            db.execute_batch("DROP TABLE temp.again;")
        })
        .map_err(failed)
    }

    /// The identities of the final events numbered `places` (from 0).
    pub(crate) fn identities_at(&mut self, places: &[usize]) -> Result<Vec<Vec<u8>>, String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || {
            let mut statement = db.prepare_cached("SELECT identity FROM final WHERE seq = ?1")?;
            places.iter().map(|place| statement.query_row(params![*place as i64 + 1], |row| row.get(0))).collect::<Result<Vec<Vec<u8>>, _>>()
        })
        .map_err(failed)
    }

    /// The 8-byte digest of a record id a follower's seam moved past
    /// (`claude::history::PrefixIds`), kept once however often it comes.
    pub(crate) fn push_moved(&mut self, digest: u64) -> Result<(), String> {
        self.line(Row::Moved(digest))
    }

    /// Lets go the digests kept (`push_moved`).
    pub(crate) fn clear_moved(&mut self) -> Result<(), String> {
        self.lined.retain(|row| !matches!(row, Row::Moved(_)));
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || db.execute_batch("DELETE FROM moved;")).map_err(failed)
    }

    /// The digests kept (`push_moved`), rising, each once, as the follower's
    /// checkpoint keeps them: eight little-endian bytes each.
    pub(crate) fn moved_bytes(&mut self) -> Result<Vec<u8>, String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<u8>, fastctx_storage::rusqlite::Error> {
            let mut statement = db.prepare_cached("SELECT digest FROM moved ORDER BY digest")?;
            let mut rows = statement.query([])?;
            let mut bytes = Vec::new();
            while let Some(row) = rows.next()? {
                let digest: Vec<u8> = row.get(0)?;
                let digest: [u8; 8] = digest.as_slice().try_into().map_err(|_| fastctx_storage::rusqlite::Error::InvalidQuery)?;
                bytes.extend_from_slice(&u64::from_be_bytes(digest).to_le_bytes());
            }
            Ok(bytes)
        })
        .map_err(failed)
    }

    /// What the record stopped short (§28.49) numbered `record` knows one of
    /// its events by (`claude::history::freeze`); a key known twice is kept
    /// once.
    pub(crate) fn push_known(&mut self, record: u32, key: String) -> Result<(), String> {
        self.line(Row::Known { record, key })
    }

    /// Of the final events, those some record stopped short knows
    /// (`push_known`), from the one after `after` (its `seq`, 0 for the
    /// first) on: a page of them in their order, each one's `seq` and
    /// identity. Begins `found` anew when `after` is 0.
    pub(crate) fn known_from(&mut self, after: i64) -> Result<Vec<(i64, Vec<u8>)>, String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<(i64, Vec<u8>)>, fastctx_storage::rusqlite::Error> {
            if after == 0 {
                db.execute_batch("DROP TABLE IF EXISTS temp.found; CREATE TEMP TABLE found (identity BLOB PRIMARY KEY) WITHOUT ROWID;")?;
            }
            let mut statement = db.prepare_cached(
                "SELECT seq, identity FROM final WHERE seq > ?1 AND known IS NOT NULL AND EXISTS (SELECT 1 FROM known WHERE key = final.known)
                 ORDER BY seq LIMIT 4096",
            )?;
            let rows = statement.query_map(params![after], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
        .map_err(failed)
    }

    /// Of the identities `known_from` gave, those the timeline has an item
    /// of in any state.
    pub(crate) fn push_found(&mut self, found: &HashSet<Vec<u8>>) -> Result<(), String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<(), fastctx_storage::rusqlite::Error> {
            let transaction = db.unchecked_transaction()?;
            for identity in found {
                transaction.prepare_cached("INSERT OR IGNORE INTO temp.found (identity) VALUES (?1)")?.execute(params![identity])?;
            }
            transaction.commit()
        })
        .map_err(failed)
    }

    /// The final events a writing leaves as the timeline has them where
    /// records stopped short (`assets::untouched`, decided whole here on
    /// what `push_known` and `push_found` kept), each one's place (from 0)
    /// to `each` in their order: those found, and one known only by its
    /// moment (not named) where the timeline has a named event of its
    /// record — but a moment more than one record knows, judged by its own
    /// item alone.
    pub(crate) fn left(&mut self, each: &mut dyn FnMut(usize)) -> Result<(), String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<(), fastctx_storage::rusqlite::Error> {
            // Each step walks the table it names first (`CROSS JOIN` keeps
            // SQLite to that order) and seeks the next by its key, so that
            // none goes over another whole for each row: the keys of named
            // events found are taken once each before their records are.
            db.execute_batch(
                "DROP TABLE IF EXISTS temp.shared; DROP TABLE IF EXISTS temp.named; DROP TABLE IF EXISTS temp.marked;
                 CREATE TEMP TABLE shared (key TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO temp.shared SELECT key FROM known GROUP BY key HAVING COUNT(*) > 1;
                 CREATE TEMP TABLE named (key TEXT PRIMARY KEY) WITHOUT ROWID;
                 INSERT OR IGNORE INTO temp.named SELECT f.known FROM temp.found x CROSS JOIN final f ON f.identity = x.identity
                   WHERE f.named = 1 AND f.known IS NOT NULL;
                 CREATE TEMP TABLE marked (record INTEGER PRIMARY KEY);
                 INSERT OR IGNORE INTO temp.marked SELECT k.record FROM temp.named n CROSS JOIN known k ON k.key = n.key;
                 INSERT OR IGNORE INTO temp.found SELECT f.identity FROM final f CROSS JOIN known k ON k.key = f.known CROSS JOIN temp.marked m ON m.record = k.record
                   WHERE f.named = 0 AND NOT EXISTS (SELECT 1 FROM temp.shared s WHERE s.key = f.known);",
            )?;
            {
                let mut statement =
                    db.prepare_cached("SELECT seq FROM final WHERE known IS NOT NULL AND identity IN (SELECT identity FROM temp.found) ORDER BY seq")?;
                let mut rows = statement.query([])?;
                while let Some(row) = rows.next()? {
                    each(row.get::<_, i64>(0)?.max(1) as usize - 1);
                }
            }
            db.execute_batch("DROP TABLE temp.shared; DROP TABLE temp.named; DROP TABLE temp.marked; DROP TABLE temp.found;")
        })
        .map_err(failed)
    }

    /// The sets `push_known` kept, a set for each of `records` in its
    /// number's order.
    #[cfg(test)]
    pub(crate) fn known_sets(&mut self, records: usize) -> Result<Vec<HashSet<String>>, String> {
        let db = self.db()?;
        let rows = fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<(u32, String)>, fastctx_storage::rusqlite::Error> {
            let mut statement = db.prepare_cached("SELECT record, key FROM known")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
        .map_err(failed)?;
        let mut sets: Vec<HashSet<String>> = (0..records).map(|_| HashSet::new()).collect();
        for (record, key) in rows {
            sets.get_mut(record as usize).ok_or_else(|| failed("a set of a record not counted"))?.insert(key);
        }
        Ok(sets)
    }

    /// An item the timeline has, its order key and the length of its body.
    pub(crate) fn push_had(&mut self, identity: Vec<u8>, place: Vec<u8>, length: u64) -> Result<(), String> {
        self.line(Row::Had { identity, place, length })
    }

    /// For each final event, in their order: whether the timeline has its
    /// item live, and its order key there where it is of eight bytes; and,
    /// for a window from `since` on (`early`), whether it is from before the
    /// window — earlier than `since`, live in the timeline, not among those
    /// told again (`again`) — as `assets::write` finds it.
    pub(crate) fn had_keys(&mut self, early: Option<(jiff::Timestamp, &HashSet<Vec<u8>>)>) -> Result<Keyed, String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Keyed, fastctx_storage::rusqlite::Error> {
            let mut statement =
                db.prepare_cached("SELECT h.place, f.identity, f.second, f.nanosecond FROM final f LEFT JOIN had h ON h.identity = f.identity ORDER BY f.seq")?;
            let mut rows = statement.query([])?;
            let mut keyed = Keyed::default();
            while let Some(row) = rows.next()? {
                let place: Option<Vec<u8>> = row.get(0)?;
                let key = place.as_ref().and_then(|place| place.as_slice().try_into().ok().map(u64::from_be_bytes));
                let before = match early {
                    Some((since, again)) if place.is_some() => {
                        let at = jiff::Timestamp::new(row.get(2)?, row.get(3)?).map_err(|_| fastctx_storage::rusqlite::Error::InvalidQuery)?;
                        at < since && !again.contains(&row.get::<_, Vec<u8>>(1)?)
                    }
                    _ => false,
                };
                keyed.keys.push(key.unwrap_or(0));
                keyed.flags.push(
                    if place.is_some() { LIVE } else { 0 } | if key.is_some() { KEYED } else { 0 } | if before { EARLY } else { 0 },
                );
            }
            Ok(keyed)
        })
        .map_err(failed)
    }

    /// An item a window's writing reads again in its window, or takes away
    /// where its history no longer has it (`assets::Reach::From`): one past
    /// the floor, or told at the last reading. Kept once.
    pub(crate) fn push_before(&mut self, identity: Vec<u8>) -> Result<(), String> {
        self.line(Row::Before(identity))
    }

    /// Of the final events' identities and those `push_before` kept, a page
    /// after `after` (none for the first) in the order of their bytes, each
    /// once: what a window's writing asks the timeline for.
    pub(crate) fn asked_from(&mut self, after: Option<&[u8]>, limit: usize) -> Result<Vec<Vec<u8>>, String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<Vec<u8>>, fastctx_storage::rusqlite::Error> {
            // Every identity is longer than none, so none is before them all.
            let mut statement = db.prepare_cached(
                "SELECT identity FROM (SELECT identity FROM final WHERE identity > ?1 UNION SELECT identity FROM before WHERE identity > ?1)
                 ORDER BY identity LIMIT ?2",
            )?;
            let rows = statement.query_map(params![after.unwrap_or_default(), limit as i64], |row| row.get(0))?;
            rows.collect()
        })
        .map_err(failed)
    }

    /// Of the items `push_before` kept, those the timeline has live
    /// (`push_had`) that no final event is: a page after `after` (none for
    /// the first) in the order of their bytes — what a window's writing
    /// takes away.
    pub(crate) fn gone_before(&mut self, after: Option<&[u8]>, limit: usize) -> Result<Vec<Gone>, String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<Gone>, fastctx_storage::rusqlite::Error> {
            let mut statement = db.prepare_cached(
                "SELECT b.identity, h.place FROM before b JOIN had h ON h.identity = b.identity
                 WHERE b.identity > ?1 AND NOT EXISTS (SELECT 1 FROM final f WHERE f.identity = b.identity)
                 ORDER BY b.identity LIMIT ?2",
            )?;
            let rows = statement.query_map(params![after.unwrap_or_default(), limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
        .map_err(failed)
    }

    /// The items the timeline has that no final event is, after the identity
    /// `after`, at most `limit`, in the order of their identities: each
    /// identity with its order key.
    pub(crate) fn gone(&mut self, after: Option<&[u8]>, limit: usize) -> Result<Vec<Gone>, String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<Gone>, fastctx_storage::rusqlite::Error> {
            // Every identity is longer than none, so none is before them all.
            let mut statement = db.prepare_cached(
                "SELECT h.identity, h.place FROM had h
                 WHERE h.identity > ?1 AND NOT EXISTS (SELECT 1 FROM final f WHERE f.identity = h.identity)
                 ORDER BY h.identity LIMIT ?2",
            )?;
            let rows = statement.query_map(params![after.unwrap_or_default(), limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
        .map_err(failed)
    }

    /// The final events from the `from`th on (from 0), at most `limit` and
    /// about `bytes` with the bodies the timeline has of them, at least one.
    pub(crate) fn finals(&mut self, from: usize, limit: usize, bytes: u64) -> Result<Vec<Final>, String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<Final>, String> {
            let mut statement = db
                .prepare_cached(
                    "SELECT length(f.record) + coalesce(length(f.encoded), 0), f.identity, f.second, f.nanosecond, f.record, f.object, f.size, f.encoded,
                            h.place, h.length FROM final f
                     LEFT JOIN had h ON h.identity = f.identity
                     WHERE f.seq > ?1 ORDER BY f.seq",
                )
                .map_err(failed)?;
            let mut rows = statement.query(params![from as i64]).map_err(failed)?;
            let (mut taken, mut size) = (Vec::new(), 0u64);
            while let Some(row) = rows.next().map_err(failed)? {
                let had_length = row.get::<_, Option<i64>>(9).map_err(failed)?;
                // Its length and the old body's known before its bytes are read.
                let more = row.get::<_, i64>(0).map_err(failed)?.max(0) as u64 + had_length.unwrap_or(0).max(0) as u64;
                if !taken.is_empty() && size + more > bytes {
                    break;
                }
                size += more;
                let at = jiff::Timestamp::new(row.get(2).map_err(failed)?, row.get(3).map_err(failed)?).map_err(failed)?;
                let object = match (row.get::<_, Option<Vec<u8>>>(5).map_err(failed)?, row.get::<_, Option<i64>>(6).map_err(failed)?) {
                    (Some(hash), Some(size)) => {
                        let hash: [u8; 32] = hash.as_slice().try_into().map_err(|_| failed("an object's hash kept is not 32 bytes"))?;
                        let encoded: Vec<u8> = row.get::<_, Option<Vec<u8>>>(7).map_err(failed)?.unwrap_or_default();
                        Some((ObjectRef { hash, size: size.max(0) as u64 }, encoded))
                    }
                    _ => None,
                };
                let place: Option<Vec<u8>> = row.get(8).map_err(failed)?;
                taken.push(Final {
                    identity: row.get(1).map_err(failed)?,
                    at,
                    record: row.get(4).map_err(failed)?,
                    object,
                    had: place.map(|place| (place, had_length.unwrap_or(0).max(0) as u64)),
                });
                if taken.len() >= limit {
                    break;
                }
            }
            Ok(taken)
        })
    }
}

/// For each final event, in their order (`Spill::had_keys`): whether the
/// timeline has its item live, its order key there where that is of eight
/// bytes, and whether it is left as the timeline has it (`leave`) — a key
/// and a byte each.
#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct Keyed {
    keys: Vec<u64>,
    flags: Vec<u8>,
}

const LIVE: u8 = 1;
const KEYED: u8 = 2;
const LEFT: u8 = 4;
/// From before a window that the timeline has live (`Keyed::early`).
const EARLY: u8 = 8;

impl Keyed {
    pub(crate) fn len(&self) -> usize {
        self.flags.len()
    }

    pub(crate) fn live(&self, index: usize) -> bool {
        self.flags[index] & LIVE != 0
    }

    pub(crate) fn key(&self, index: usize) -> Option<u64> {
        (self.flags[index] & KEYED != 0).then_some(self.keys[index])
    }

    /// The final event numbered `index` is left as the timeline has it.
    pub(crate) fn leave(&mut self, index: usize) {
        self.flags[index] |= LEFT;
    }

    pub(crate) fn left(&self, index: usize) -> bool {
        self.flags[index] & LEFT != 0
    }

    /// The final event numbered `index` is from before the window and the
    /// timeline has it live: it keeps its place (`assets::Reach::From`).
    pub(crate) fn early(&self, index: usize) -> bool {
        self.flags[index] & EARLY != 0
    }

    /// The keys the timeline had are let go, the new ones made: what each
    /// event is (`live`, `left`) is all that is asked after.
    pub(crate) fn keys_made(&mut self) {
        self.keys = Vec::new();
        self.flags.iter_mut().for_each(|flags| *flags &= !KEYED);
    }
}

/// Events kept in a store, read back a bounded batch at a time ahead of
/// where they are asked for (`take`), each taken once: for reading them in
/// an order that mostly rises, without asking the store for each.
#[derive(Default)]
pub(crate) struct Ahead {
    from: usize,
    batch: Vec<Option<SessionEvent>>,
}

impl Ahead {
    /// The event numbered `index`: from the batch read, or read with those
    /// after it (`fetch` of a number: the events from it on) where it is
    /// past the batch; one before the batch is read alone (`fetch` asked
    /// for one), the batch kept — an order that turns back reads each it
    /// turns back to once, not a batch again for each.
    pub(crate) fn take(&mut self, index: usize, fetch: &mut dyn FnMut(usize, bool) -> Result<Vec<SessionEvent>, String>) -> Result<SessionEvent, String> {
        if index < self.from {
            return fetch(index, true)?.into_iter().next().ok_or_else(|| failed(format!("event {index} kept was not read back")));
        }
        let held = self.batch.get(index - self.from).is_some_and(Option::is_some);
        if !held {
            self.batch = fetch(index, false)?.into_iter().map(Some).collect();
            self.from = index;
        }
        self.batch.get_mut(index - self.from).and_then(Option::take).ok_or_else(|| failed(format!("event {index} kept was not read back")))
    }
}

/// A row of a Claude record as kept: its number (from 0), its line's
/// number, where its line starts, the line.
pub(crate) type Line = (usize, usize, u64, Vec<u8>);

impl Spill {
    /// The next row of a Claude record as a scan takes it (`Lines`): its
    /// line as the file holds it, bearing `uuid` when it bears one.
    pub(crate) fn push_line(&mut self, uuid: Option<&str>, number: usize, start: u64, bytes: &[u8]) -> Result<(), String> {
        let seq = self.lines;
        self.lines += 1;
        if let Some(uuid) = uuid {
            self.lined_uuids.insert(uuid.to_owned(), seq);
        }
        self.lined_lines += 1;
        self.line(Row::Line { seq, uuid: uuid.map(str::to_owned), number, start, bytes: bytes.to_vec() })
    }

    /// A later form of the row numbered `seq`, put in its place.
    pub(crate) fn reline(&mut self, seq: usize, number: usize, start: u64, bytes: &[u8]) -> Result<(), String> {
        if seq >= self.lines {
            return Err(failed(format!("row {seq} put again, of {} kept", self.lines)));
        }
        self.lined_lines += 1;
        self.line(Row::Relined { seq, number, start, bytes: bytes.to_vec() })
    }

    /// The number of the row bearing `uuid`, written or lined up.
    pub(crate) fn line_of(&self, uuid: &str) -> Result<Option<usize>, String> {
        if let Some(&seq) = self.lined_uuids.get(uuid) {
            return Ok(Some(seq));
        }
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || {
            db.prepare_cached("SELECT seq FROM line WHERE uuid = ?1")?.query_row(params![uuid], |row| row.get::<_, i64>(0)).optional()
        })
        .map(|seq| seq.map(|seq| seq.max(0) as usize))
        .map_err(failed)
    }

    /// The store, the rows of a record all written.
    fn lines_written(&self) -> Result<&Connection, String> {
        if self.lined_lines > 0 {
            return Err(failed("rows read back before they were written"));
        }
        self.db.as_ref().ok_or_else(|| failed("not begun"))
    }

    /// The row numbered `seq`.
    pub(crate) fn line_at(&self, seq: usize) -> Result<Line, String> {
        let db = self.lines_written()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || {
            db.prepare_cached("SELECT number, start, bytes FROM line WHERE seq = ?1")?.query_row(params![seq as i64], |row| {
                Ok((seq, row.get::<_, i64>(0)?.max(0) as usize, row.get::<_, i64>(1)?.max(0) as u64, row.get(2)?))
            })
        })
        .map_err(failed)
    }

    /// A tool block of the rows a projection pairs (`RowStore::note_block`).
    pub(crate) fn push_block(&mut self, id: &str, result: bool, line: usize, place: usize) -> Result<(), String> {
        self.lined_blocks += 1;
        self.line(Row::Block { id: id.to_owned(), result, line, place })
    }

    /// The first tool blocks noted of `id` (`RowStore::blocks`): at most two
    /// calls, then at most two results, each in the order noted; each
    /// whether it is a result, its row and its place in the row. What is
    /// lined up is written first.
    pub(crate) fn blocks_of(&mut self, id: &str) -> Result<Vec<(bool, usize, usize)>, String> {
        if self.lined_blocks > 0 {
            self.flush()?;
        }
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<(bool, usize, usize)>, fastctx_storage::rusqlite::Error> {
            // Each side sought in the index where it begins: however many
            // share the id, only these are read.
            let mut statement = db.prepare_cached("SELECT line, place FROM block WHERE id = ?1 AND result = ?2 ORDER BY seq LIMIT 2")?;
            let mut blocks = Vec::with_capacity(4);
            for result in [false, true] {
                let rows = statement.query_map(params![id, result], |row| Ok((result, row.get::<_, i64>(0)?.max(0) as usize, row.get::<_, i64>(1)?.max(0) as usize)))?;
                for block in rows {
                    blocks.push(block?);
                }
            }
            Ok(blocks)
        })
        .map_err(failed)
    }

    /// A key of the set numbered `kind` (`RowStore::note_member`).
    pub(crate) fn push_member(&mut self, kind: u8, key: &str) -> Result<(), String> {
        self.lined_blocks += 1;
        self.line(Row::Member { kind, key: key.to_owned() })
    }

    /// Whether `key` was noted in the set numbered `kind`. What is lined up
    /// is written first.
    pub(crate) fn is_member(&mut self, kind: u8, key: &str) -> Result<bool, String> {
        if self.lined_blocks > 0 {
            self.flush()?;
        }
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || {
            db.prepare_cached("SELECT 1 FROM member WHERE kind = ?1 AND key = ?2")?.query_row(params![kind, key], |_| Ok(())).optional()
        })
        .map(|found| found.is_some())
        .map_err(failed)
    }

    /// Begins reading a Claude record again, or the next one: the rows,
    /// tool blocks and sets of the reading before are let go.
    pub(crate) fn reading(&mut self) -> Result<(), String> {
        self.flush()?;
        self.lines = 0;
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || db.execute_batch(READING)).map_err(failed)
    }

    /// The next event of a Claude session as its record shows it, kept
    /// exactly (`pack_event`), and the number it is kept by.
    pub(crate) fn push_raw(&mut self, body: Vec<u8>) -> Result<u32, String> {
        let seq = self.raws;
        self.raws = seq.checked_add(1).ok_or_else(|| failed("more events than a reading counts"))?;
        self.line(Row::Raw { seq, body })?;
        Ok(seq)
    }

    /// How many events of Claude sessions are kept.
    pub(crate) fn raws(&self) -> u32 {
        self.raws
    }

    /// Lets go the events of Claude sessions numbered `to` and after.
    pub(crate) fn truncate_raw(&mut self, to: u32) -> Result<(), String> {
        if to >= self.raws {
            return Ok(());
        }
        self.flush()?;
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || db.prepare_cached("DELETE FROM raw WHERE seq >= ?1")?.execute(params![to]))
            .map_err(failed)?;
        self.raws = to;
        Ok(())
    }

    /// Where placing put the next event of the history (`push_placed`, in
    /// the order placed): its number, and its World turn's.
    pub(crate) fn push_placed(&mut self, event: u32, turn: Option<u32>) -> Result<(), String> {
        self.line(Row::Placed { event, turn })
    }

    /// The events placed, from the `from`th on (from 0), a batch of them in
    /// the order placed.
    pub(crate) fn placed_from(&mut self, from: usize) -> Result<Vec<(u32, Option<u32>)>, String> {
        let db = self.db()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<(u32, Option<u32>)>, fastctx_storage::rusqlite::Error> {
            let mut statement = db.prepare_cached("SELECT event, turn FROM placed WHERE seq > ?1 ORDER BY seq LIMIT 65536")?;
            let rows = statement.query_map(params![from as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
        .map_err(failed)
    }

    /// The events of Claude sessions from the one numbered `from`, before
    /// `end`, in their order: about `READ_BYTES` of them as they are held
    /// once read, at least one while any are left.
    pub(crate) fn raws_from(&mut self, from: u32, end: u32) -> Result<Vec<(u32, SessionEvent)>, String> {
        let db = self.db()?;
        let bodies = fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<(u32, Value)>, String> {
            let mut statement = db.prepare_cached("SELECT seq, length(body), body FROM raw WHERE seq >= ?1 AND seq < ?2 ORDER BY seq").map_err(failed)?;
            let mut rows = statement.query(params![from, end]).map_err(failed)?;
            let (mut taken, mut size) = (Vec::new(), 0u64);
            while let Some(row) = rows.next().map_err(failed)? {
                // Its packed length, less than it takes unpacked.
                let length = row.get::<_, i64>(1).map_err(failed)?.max(0) as u64;
                if !taken.is_empty() && size + length > READ_BYTES {
                    break;
                }
                let body: Vec<u8> = row.get(2).map_err(failed)?;
                let (value, held) = unpack_sized(&body)?;
                size += held;
                taken.push((row.get(0).map_err(failed)?, value));
            }
            Ok(taken)
        })?;
        bodies.into_iter().map(|(seq, value)| Ok((seq, serde_json::from_value(value).map_err(failed)?))).collect()
    }

    /// The Claude record read last is accepted: the UUIDs its rows bear, the
    /// queued messages' sources it noted (`member` of `SET_QUEUED`) and
    /// those it holds in memory (`queued`) join those a later session's
    /// reading sees as earlier (`is_earlier`).
    pub(crate) fn accept_earlier<'q>(&mut self, queued: impl Iterator<Item = &'q str>) -> Result<(), String> {
        self.flush()?;
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        let set = i64::from(fastctx_hosts::claude::transcript::SET_QUEUED);
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<(), fastctx_storage::rusqlite::Error> {
            let transaction = db.unchecked_transaction()?;
            transaction.execute("INSERT OR IGNORE INTO earlier (uuid) SELECT uuid FROM line WHERE uuid IS NOT NULL", [])?;
            transaction.execute("INSERT OR IGNORE INTO earlier (uuid) SELECT key FROM member WHERE kind = ?1", params![set])?;
            for uuid in queued {
                transaction.prepare_cached("INSERT OR IGNORE INTO earlier (uuid) VALUES (?1)")?.execute(params![uuid])?;
            }
            transaction.commit()
        })
        .map_err(failed)
    }

    /// The record ids the Claude sessions accepted before hold
    /// (`accept_earlier`), after `after` (none for the first): a page of
    /// them, in the order of their bytes.
    pub(crate) fn earlier_from(&self, after: Option<&str>) -> Result<Vec<String>, String> {
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<String>, fastctx_storage::rusqlite::Error> {
            // A plain bound, sought in the key's index (`reads::seek`).
            match after {
                Some(after) => db
                    .prepare_cached("SELECT uuid FROM earlier WHERE uuid > ?1 ORDER BY uuid LIMIT 4096")?
                    .query_map(params![after], |row| row.get(0))?
                    .collect(),
                None => db.prepare_cached("SELECT uuid FROM earlier ORDER BY uuid LIMIT 4096")?.query_map([], |row| row.get(0))?.collect(),
            }
        })
        .map_err(failed)
    }

    /// Whether a Claude session accepted before holds `uuid` (`accept_earlier`).
    pub(crate) fn is_earlier(&self, uuid: &str) -> Result<bool, String> {
        let db = self.db.as_ref().ok_or_else(|| failed("not begun"))?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || {
            db.prepare_cached("SELECT 1 FROM earlier WHERE uuid = ?1")?.query_row(params![uuid], |_| Ok(())).optional()
        })
        .map(|found| found.is_some())
        .map_err(failed)
    }

    /// Of the rows numbered after `after` (from 0; none for the first), a
    /// page of those that start before `to` and bear a UUID, in their
    /// order: each one's number and UUID.
    pub(crate) fn ids_before(&self, to: u64, after: Option<usize>) -> Result<Vec<(usize, String)>, String> {
        let db = self.lines_written()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<(usize, String)>, fastctx_storage::rusqlite::Error> {
            let mut statement =
                db.prepare_cached("SELECT seq, uuid FROM line WHERE seq > ?1 AND start < ?2 AND uuid IS NOT NULL ORDER BY seq LIMIT 4096")?;
            // Every row is before "the end" (`u64::MAX`), which SQLite's
            // integers hold only as their largest.
            let to = i64::try_from(to).unwrap_or(i64::MAX);
            let rows = statement.query_map(params![after.map_or(-1, |after| after as i64), to], |row| {
                Ok((row.get::<_, i64>(0)?.max(0) as usize, row.get(1)?))
            })?;
            rows.collect()
        })
        .map_err(failed)
    }

    /// The rows from the one numbered `from` on, in their order: about
    /// `READ_BYTES` of lines, at least one while any are left.
    pub(crate) fn lines_from(&self, from: usize) -> Result<Vec<Line>, String> {
        let db = self.lines_written()?;
        fastctx_platform::roles::in_place("keeping a reading on disk", || -> Result<Vec<Line>, fastctx_storage::rusqlite::Error> {
            let mut statement = db.prepare_cached("SELECT seq, number, start, length(bytes), bytes FROM line WHERE seq >= ?1 ORDER BY seq")?;
            let mut rows = statement.query(params![from as i64])?;
            let (mut taken, mut size) = (Vec::new(), 0u64);
            while let Some(row) = rows.next()? {
                // Its length known before its bytes are read.
                let length = row.get::<_, i64>(3)?.max(0) as u64;
                if !taken.is_empty() && size + length > READ_BYTES {
                    break;
                }
                size += length;
                taken.push((
                    row.get::<_, i64>(0)?.max(0) as usize,
                    row.get::<_, i64>(1)?.max(0) as usize,
                    row.get::<_, i64>(2)?.max(0) as u64,
                    row.get(4)?,
                ));
            }
            Ok(taken)
        })
        .map_err(failed)
    }
}

/// A Claude record's rows kept in a reading's work store as a scan takes
/// them, and read back as its projection asks (`RowStore`, design §28.51
/// ⑤-3). The store is shared with what keeps the events made of them as
/// they are made: this holds it only while it reads or writes, never while
/// a row read is looked at, and reads in order a bounded batch at a time.
pub(crate) struct Lines<'s>(pub(crate) &'s RefCell<Spill>);

impl RowStore for Lines<'_> {
    fn count(&self) -> usize {
        self.0.borrow().lines
    }
    fn push(&mut self, uuid: Option<&str>, line: usize, start: u64, bytes: &[u8]) -> std::io::Result<()> {
        self.0.borrow_mut().push_line(uuid, line, start, bytes).map_err(std::io::Error::other)
    }
    fn replace(&mut self, index: usize, line: usize, start: u64, bytes: &[u8]) -> std::io::Result<()> {
        self.0.borrow_mut().reline(index, line, start, bytes).map_err(std::io::Error::other)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().flush().map_err(std::io::Error::other)
    }
    fn note_block(&self, id: &str, result: bool, row: usize, index: usize) -> std::io::Result<()> {
        self.0.borrow_mut().push_block(id, result, row, index).map_err(std::io::Error::other)
    }
    fn blocks(&self, id: &str) -> std::io::Result<Vec<(bool, usize, usize)>> {
        self.0.borrow_mut().blocks_of(id).map_err(std::io::Error::other)
    }
    fn note_member(&self, set: u8, key: &str) -> std::io::Result<()> {
        self.0.borrow_mut().push_member(set, key).map_err(std::io::Error::other)
    }
    fn is_member(&self, set: u8, key: &str) -> std::io::Result<bool> {
        self.0.borrow_mut().is_member(set, key).map_err(std::io::Error::other)
    }
    fn index_of(&self, uuid: &str) -> std::io::Result<Option<usize>> {
        self.0.borrow().line_of(uuid).map_err(std::io::Error::other)
    }
    fn row(&self, index: usize) -> std::io::Result<(usize, u64, Vec<u8>)> {
        let (_, number, start, bytes) = self.0.borrow().line_at(index).map_err(std::io::Error::other)?;
        Ok((number, start, bytes))
    }
    fn each(&self, visit: VisitLine<'_>) -> std::io::Result<()> {
        let mut from = 0;
        loop {
            let batch = self.0.borrow().lines_from(from).map_err(std::io::Error::other)?;
            let Some(&(last, ..)) = batch.last() else { return Ok(()) };
            for (seq, number, start, bytes) in batch {
                visit(seq, number, start, &bytes)?;
            }
            from = last + 1;
        }
    }
    fn ids_before(&self, to: u64, each: &mut dyn FnMut(&str) -> std::io::Result<()>) -> std::io::Result<()> {
        let mut after = None;
        loop {
            let page = self.0.borrow().ids_before(to, after).map_err(std::io::Error::other)?;
            let Some(&(last, _)) = page.last() else { return Ok(()) };
            for (_, uuid) in &page {
                each(uuid)?;
            }
            after = Some(last);
        }
    }
    fn clear(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().reading().map_err(std::io::Error::other)
    }
}

impl Drop for Spill {
    fn drop(&mut self) {
        let (db, dir) = (self.db.take(), self.dir.take());
        fastctx_platform::roles::in_place("taking away a reading kept on disk", || {
            // Closed before its directory goes: an open file is not removed
            // on every system. One left behind is taken away before a new
            // holder is admitted (`clear`).
            drop(db);
            if let Some(dir) = dir {
                let _ = fastctx_platform::disk::remove_dir_all(&dir);
            }
        });
        drop(self.entered.take());
    }
}

/// Takes away what readings kept on disk under `root` left: when the asset
/// library is opened here, and before a new holder is admitted, once no
/// reading is kept. Whether all is gone.
pub(crate) fn clear(root: &std::path::Path) -> std::io::Result<()> {
    match fastctx_platform::disk::remove_dir_all(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        done => done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One test at a time: holding off is the whole process's.
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn spill(name: &str) -> (PathBuf, Spill, std::sync::MutexGuard<'static, ()>) {
        let one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = std::env::temp_dir().join(format!("fastctx-spill-{name}-{}", std::process::id()));
        let _ = clear(&root);
        (root.clone(), Spill::new(root, "codex-a0a0"), one)
    }

    /// What is kept comes back as it was, a float to its last bit — which
    /// JSON text read again does not promise.
    #[test]
    fn what_is_kept_comes_back_exactly() {
        let float: Value = serde_json::from_str("0.00000021675604049888438").unwrap();
        let value = json!({"a": [null, true, false, 0, -1, u64::MAX, i64::MIN, 0.5, -0.0, float.clone()], "b": {"": "x\u{0}y", "é": [[], {}]}, "c": "long ".repeat(1000)});
        assert_eq!(unpack(&pack(&value)).unwrap(), value);
        let kept = unpack(&pack(&float)).unwrap();
        assert_eq!(kept.as_f64().map(f64::to_bits), float.as_f64().map(f64::to_bits));
        assert!(unpack(&pack(&value)[..10]).is_err());
    }

    /// A turn's items read a page at a time come back in their order, each
    /// once, whether a page goes on where the last stopped or not.
    #[test]
    fn a_turns_items_come_back_a_page_at_a_time() {
        let (root, mut spill, _one) = spill("items");
        spill.begin().unwrap();
        let item = |turn: &str, n: usize| json!({"item": {"id": format!("{turn}{n}"), "pad": "x".repeat(n % 50)}});
        for n in 0..700 {
            spill.item("a", &item("a", n)).unwrap();
            if n % 3 == 0 {
                spill.item("b", &item("b", n)).unwrap();
            }
        }
        for turn in ["a", "b"] {
            let all: Vec<Value> = (0..700).filter(|n| turn == "a" || n % 3 == 0).map(|n| item(turn, n)).collect();
            let (mut got, mut from) = (Vec::new(), 0);
            loop {
                let page = spill.items_of(turn, from, 64).unwrap();
                if page.is_empty() {
                    break;
                }
                from += page.len();
                got.extend(page);
            }
            assert_eq!(got, all);
            // Asked elsewhere, then on from there; another turn between.
            assert_eq!(spill.items_of(turn, 5, 3).unwrap(), all[5..8]);
            assert_eq!(spill.items_of(turn, 8, 2).unwrap(), all[8..10]);
            spill.items_of(if turn == "a" { "b" } else { "a" }, 0, 4).unwrap();
            assert_eq!(spill.items_of(turn, 10, 2).unwrap(), all[10..12]);
        }
        drop(spill);
        let _ = clear(&root);
    }

    /// Entries come back as a stable sort of them by position has them,
    /// one without a position first, a batch at a time; an item listed
    /// twice is timed by its last listing, one listed with no times is
    /// timed so, one never listed not at all.
    #[test]
    fn entries_come_back_as_a_stable_sort_has_them() {
        let (root, mut spill, _one) = spill("entries");
        spill.begin().unwrap();
        let listed = [
            json!({"type": "item", "turnId": "t", "item": {"id": "a"}, "position": 3}),
            json!({"type": "item", "turnId": "t", "item": {"id": "b"}, "position": 1}),
            json!({"type": "turnStarted", "turnId": "t"}),
            json!({"type": "item", "turnId": "t", "item": {"id": "c"}, "position": 3}),
            json!({"type": "item", "turnId": "t", "item": {"id": "d"}, "position": u64::MAX}),
            json!({"type": "item", "turnId": "t", "item": {"id": "e"}, "position": 2}),
            json!({"type": "item", "turnId": "t", "item": {"id": "f"}, "position": -1}),
        ];
        spill.time("t", "a", Timing { started: Some(1), completed: Some(2) }).unwrap();
        spill.time("t", "a", Timing { started: Some(5), completed: Some(6) }).unwrap();
        spill.time("t", "b", Timing::default()).unwrap();
        for entry in &listed {
            spill.entry(entry).unwrap();
        }
        let mut sorted = listed.to_vec();
        sorted.sort_by_key(|entry| entry["position"].as_u64());
        let mut got = Vec::new();
        loop {
            let batch = spill.entries(got.len(), 2).unwrap();
            if batch.is_empty() {
                break;
            }
            for entry in &batch {
                let (turn, id) = (entry["turnId"].as_str().unwrap_or_default(), entry["item"]["id"].as_str());
                if let Some(id) = id {
                    let timed = spill.timing(turn, id).unwrap();
                    match id {
                        "a" => assert_eq!(timed, Some(Timing { started: Some(5), completed: Some(6) })),
                        "b" => assert_eq!(timed, Some(Timing::default())),
                        _ => assert_eq!(timed, None),
                    }
                }
            }
            got.extend(batch);
        }
        assert_eq!(got, sorted);
        assert!(spill.entries(3, 2).is_err(), "only from where they stopped");
        drop(spill);
        assert!(!fastctx_platform::disk::is_there(root.join("codex-a0a0")), "taken away");
        let _ = clear(&root);
    }

    /// Entries large together come back a few at a time, one alone when it
    /// is larger than a batch.
    #[test]
    fn large_entries_come_back_a_few_at_a_time() {
        let (root, mut spill, _one) = spill("large");
        spill.begin().unwrap();
        let big = "x".repeat(READ_BYTES as usize / 3);
        for position in 0..5u64 {
            spill.entry(&json!({"type": "item", "turnId": "t", "item": {"id": position.to_string(), "text": big}, "position": position})).unwrap();
        }
        spill.entry(&json!({"type": "item", "turnId": "t", "item": {"id": "huge", "text": "y".repeat(READ_BYTES as usize + 1)}, "position": 9})).unwrap();
        let mut sizes = Vec::new();
        let mut from = 0;
        loop {
            let batch = spill.entries(from, 256).unwrap();
            if batch.is_empty() {
                break;
            }
            from += batch.len();
            sizes.push(batch.len());
        }
        assert_eq!(sizes, vec![2, 2, 1, 1]);
        drop(spill);
        let _ = clear(&root);
    }

    /// Entries of many small values, small packed, come back as few as
    /// their values take unpacked.
    #[test]
    fn entries_of_many_small_values_come_back_by_what_they_take_unpacked() {
        let (root, mut spill, _one) = spill("small-values");
        spill.begin().unwrap();
        let nulls = Value::Array(vec![Value::Null; 16_000]);
        for position in 0..64u64 {
            spill.entry(&json!({"type": "item", "turnId": "t", "item": {"id": position.to_string(), "arguments": nulls}, "position": position})).unwrap();
        }
        let first = spill.entries(0, 256).unwrap();
        let (_, held) = unpack_sized(&pack(&first[0])).unwrap();
        assert!(held > 16_000 * 32, "{held}");
        assert!(first.len() <= (READ_BYTES / held) as usize + 1, "{} entries of {held} bytes", first.len());
        assert!(first.len() < 64);
        // A map of one field takes a node with room for eleven: small maps
        // are counted so, not by their fields alone.
        let small = Value::Array(vec![json!({"x": null}); 4_000]);
        let (_, held) = unpack_sized(&pack(&small)).unwrap();
        assert!(held >= 4_000 * (map_nodes(1) + 32), "{held}");
        assert!(map_nodes(1) >= 11 * 56 && map_nodes(1_000) >= 1_000 / 11 * map_nodes(1));
        drop(spill);
        let _ = clear(&root);
    }

    /// What records stopped short leave as the timeline has them is decided
    /// without one going over another for each of its rows (`left`): many
    /// records each with a named event found and a moment of its own, and
    /// many events of one name that many records know, take time as they
    /// grow, not as they grow squared.
    #[test]
    fn what_records_stopped_short_leave_is_decided_in_time_as_they_grow() {
        const MANY: u32 = 16_384;
        let at = jiff::Timestamp::from_second(1_800_000_000).unwrap();
        let identity = |n: u32| {
            let mut identity = [0u8; 32];
            identity[..4].copy_from_slice(&n.to_be_bytes());
            identity
        };
        for shape in 0..2 {
            let (root, mut spill, _one) = spill(&format!("left-{shape}"));
            spill.begin().unwrap();
            let mut found = HashSet::new();
            for n in 0..MANY {
                // Each record's named event, found, and its moment; or each
                // event named alike, found, in every record.
                let key = if shape == 0 { format!("name/{n}") } else { "name/all".to_string() };
                spill.push_final(identity(2 * n), at, vec![b'x'], None, Some((key.clone(), true))).unwrap();
                found.insert(identity(2 * n).to_vec());
                spill.push_known(n, key).unwrap();
                if shape == 0 {
                    let moment = format!("moment/{n}/x");
                    spill.push_final(identity(2 * n + 1), at, vec![b'x'], None, Some((moment.clone(), false))).unwrap();
                    spill.push_known(n, moment).unwrap();
                }
            }
            let started = std::time::Instant::now();
            assert!(!spill.known_from(0).unwrap().is_empty());
            spill.push_found(&found).unwrap();
            let mut left = 0;
            spill.left(&mut |_| left += 1).unwrap();
            let took = started.elapsed();
            assert_eq!(left, if shape == 0 { 2 * MANY } else { MANY } as usize, "shape {shape}");
            assert!(took < std::time::Duration::from_secs(5), "shape {shape} took {took:?}");
            drop(spill);
            let _ = clear(&root);
        }
    }

    /// Final events come back in their order, a batch within its bytes but
    /// at least one, each with the item of its identity the timeline had;
    /// the items no final event is are gone; beginning again lets all go.
    #[test]
    fn finals_and_what_went() {
        let (root, mut spill, _one) = spill("finals");
        spill.begin().unwrap();
        let at = jiff::Timestamp::from_second(1_800_000_000).unwrap();
        for (n, size) in [10usize, 10, 100, 10].iter().enumerate() {
            let object = (n == 2).then(|| (ObjectRef { hash: [7; 32], size: 3 }, b"abc".to_vec()));
            let frozen = (n == 1).then(|| ("moment/1/x".to_string(), false));
            spill.push_final([n as u8; 32], at, vec![b'x'; *size], object, frozen).unwrap();
        }
        spill.push_had(vec![1; 32], 7u64.to_be_bytes().to_vec(), 50).unwrap();
        spill.push_had(vec![9; 32], vec![1, 2], 5).unwrap();
        spill.push_had(vec![8; 32], 3u64.to_be_bytes().to_vec(), 5).unwrap();
        let keyed = spill.had_keys(None).unwrap();
        assert_eq!((0..keyed.len()).map(|index| keyed.live(index)).collect::<Vec<_>>(), vec![false, true, false, false]);
        assert_eq!((0..keyed.len()).map(|index| keyed.key(index)).collect::<Vec<_>>(), vec![None, Some(7), None, None]);
        spill.push_known(0, "moment/1/x".to_string()).unwrap();
        assert_eq!(spill.known_from(0).unwrap(), vec![(2, vec![1; 32])]);
        spill.push_found(&HashSet::from([vec![1; 32]])).unwrap();
        let mut left = Vec::new();
        spill.left(&mut |place| left.push(place)).unwrap();
        assert_eq!(left, vec![1]);
        let first = spill.finals(0, 10, 70).unwrap();
        assert_eq!(first.len(), 2, "10 and 10 with the 50 it had");
        assert_eq!(first[1].had, Some((7u64.to_be_bytes().to_vec(), 50)));
        assert_eq!(first[0].at, at);
        assert!(first[0].object.is_none());
        let second = spill.finals(2, 10, 50).unwrap();
        assert_eq!(second.len(), 1, "one over its bytes still comes, alone");
        assert_eq!(second[0].object, Some((ObjectRef { hash: [7; 32], size: 3 }, b"abc".to_vec())));
        let gone = spill.gone(None, 1).unwrap();
        assert_eq!(gone, vec![(vec![8; 32], 3u64.to_be_bytes().to_vec())]);
        assert_eq!(spill.gone(Some(&gone[0].0), 10).unwrap(), vec![(vec![9; 32], vec![1, 2])]);
        spill.begin().unwrap();
        assert!(spill.finals(0, 10, 70).unwrap().is_empty());
        drop(spill);
        let _ = clear(&root);
    }

    /// The rows of a Claude record come back as the scan last put them, in
    /// their order, a bounded batch at a time: by UUID while they are still
    /// lined up, by number and in order once written; and the store may be
    /// read and written again while a row read is looked at.
    #[test]
    fn rows_come_back_as_they_were_put() {
        let (root, mut spill, _one) = spill("lines");
        spill.begin().unwrap();
        let cell = RefCell::new(spill);
        let mut lines = Lines(&cell);
        let line = |n: usize, size: usize| format!("{{\"n\":{n},\"pad\":\"{}\"}}\n", "x".repeat(size)).into_bytes();
        let mut expected: Vec<(usize, u64, Vec<u8>)> = Vec::new();
        for n in 0..3_000 {
            let size = if n % 500 == 7 { READ_BYTES as usize / 2 + 1 } else { n % 300 };
            let uuid = (n % 3 != 0).then(|| format!("u{n}"));
            lines.push(uuid.as_deref(), n + 1, n as u64 * 10, &line(n, size)).unwrap();
            expected.push((n + 1, n as u64 * 10, line(n, size)));
            assert_eq!(lines.index_of(&format!("u{n}")).unwrap(), uuid.as_ref().map(|_| n), "answered while lined up");
        }
        assert!(lines.row(5).is_err() && lines.each(&mut |_, _, _, _| Ok(())).is_err(), "only once written");
        // A later form of one written already, and of one still lined up.
        for seq in [1, 2_999] {
            lines.replace(seq, 9_000 + seq, 1, b"{\"again\":true}\n").unwrap();
            expected[seq] = (9_000 + seq, 1, b"{\"again\":true}\n".to_vec());
        }
        assert!(lines.replace(3_000, 1, 1, b"{}").is_err());
        lines.flush().unwrap();
        assert_eq!(lines.count(), 3_000);
        assert_eq!(lines.index_of("u2999").unwrap(), Some(2_999));
        assert_eq!(lines.index_of("u3").unwrap(), None);
        assert_eq!(lines.row(2_999).unwrap(), expected[2_999]);
        let mut got = Vec::new();
        lines
            .each(&mut |seq, number, start, bytes| {
                assert_eq!(seq, got.len());
                // Read again, and written to, while a row is looked at.
                assert_eq!(lines.row(seq / 2).unwrap(), expected[seq / 2]);
                cell.borrow_mut().push_hosted(vec![1]).unwrap();
                got.push((number, start, bytes.to_vec()));
                Ok(())
            })
            .unwrap();
        assert_eq!(got, expected);
        // The ids before a place, and before the end, every one of them
        // (the follower asks the latter, past the seam).
        let ids = |to: u64| {
            let mut ids = Vec::new();
            lines
                .ids_before(to, &mut |uuid| {
                    ids.push(uuid.to_string());
                    Ok(())
                })
                .unwrap();
            ids
        };
        let wanted = |to: u64| -> Vec<String> {
            (0..3_000usize)
                .filter(|n| n % 3 != 0 && expected[*n].1 < to)
                .map(|n| format!("u{n}"))
                .collect()
        };
        assert_eq!(ids(u64::MAX), wanted(u64::MAX));
        assert_eq!(ids(i64::MAX as u64 + 1), wanted(u64::MAX));
        assert_eq!(ids(500), wanted(500));
        let spill = cell.into_inner();
        drop(spill);
        let _ = clear(&root);
    }

    /// A Claude history read with its rows kept here shows what one read
    /// with them held shows — while the events made of them are written
    /// here as they are made.
    #[test]
    fn a_history_kept_is_the_history_held() {
        use fastctx_hosts::claude::transcript;
        use std::collections::HashSet;
        let (root, mut spill, _one) = spill("claude");
        fastctx_platform::disk::create_dir_all(&root).unwrap();
        let path = root.join("record.jsonl");
        let at = |n: usize| format!("2026-11-14T00:{:02}:{:02}Z", n / 60 % 60, n % 60);
        let mut lines: Vec<String> = Vec::new();
        let row = |lines: &mut Vec<String>, value: Value| lines.push(value.to_string());
        row(&mut lines, json!({"type": "user", "uuid": "u0", "parentUuid": null, "timestamp": at(0), "message": {"role": "user", "content": "begin"}}));
        row(&mut lines, json!({"type": "assistant", "uuid": "a1", "parentUuid": "u0", "timestamp": at(1), "message": {"id": "m1", "role": "assistant", "model": "m", "content": [{"type": "text", "text": "first form"}]}}));
        for n in 2..1_600 {
            row(&mut lines, json!({"type": "user", "uuid": format!("f{n}"), "parentUuid": "a1", "timestamp": at(n), "message": {"role": "user", "content": format!("words {n}")}}));
        }
        row(&mut lines, json!({"type": "assistant", "uuid": "t1", "parentUuid": "f1599", "timestamp": at(1_600), "message": {"id": "m2", "role": "assistant", "model": "m", "content": [{"type": "tool_use", "id": "call", "name": "Bash", "input": {"command": "ls"}}]}}));
        row(&mut lines, json!({"type": "user", "uuid": "r1", "parentUuid": "t1", "sourceToolAssistantUUID": "t1", "timestamp": at(1_601), "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call", "content": "y".repeat(READ_BYTES as usize + 10)}]}}));
        lines.push("not json".into());
        // A later form of a row written long before: in the first's place.
        row(&mut lines, json!({"type": "assistant", "uuid": "a1", "parentUuid": "u0", "timestamp": at(1), "message": {"id": "m1", "role": "assistant", "model": "m", "content": [{"type": "text", "text": "last form"}]}}));
        row(&mut lines, json!({"type": "assistant", "uuid": "z", "parentUuid": "r1", "timestamp": at(1_602), "durationSeconds": 0.000_000_216_756_040_498_884_38, "message": {"id": "m3", "role": "assistant", "model": "m", "content": [{"type": "text", "text": "end"}]}}));
        fastctx_platform::disk::write(&path, lines.join("\n") + "\n").unwrap();

        let mut held_events = Vec::new();
        let held = transcript::read_history(&path, &[], &HashSet::new(), &mut |event| {
            held_events.push(event);
            Ok(())
        })
        .unwrap();
        spill.begin().unwrap();
        let cell = RefCell::new(spill);
        let mut kept_events = Vec::new();
        let kept = transcript::read_history_kept(&path, &[], &|_| Ok(false), &mut Lines(&cell), &mut |event| {
            cell.borrow_mut().push_hosted(pack_event(&event).map_err(std::io::Error::other)?).map_err(std::io::Error::other)?;
            kept_events.push(event);
            Ok(())
        })
        .unwrap();
        assert!(held_events.len() > 1_600);
        assert_eq!(kept_events, held_events);
        assert_eq!((&kept.title, &kept.problems, &kept.refused, &kept.tail_uuid), (&held.title, &held.problems, &held.refused, &held.tail_uuid));
        assert_eq!((&kept.context_tail, &kept.context_parent, kept.synthetic_tail), (&held.context_tail, &held.context_parent, held.synthetic_tail));
        assert!(held.problems.iter().any(|problem| problem.contains("invalid JSON")));
        // The rows' UUIDs are the store's, not the result's.
        assert!(kept.uuids.is_empty() && held.uuids.len() == 1_603);
        let lines = Lines(&cell);
        assert!(held.uuids.iter().all(|uuid| lines.index_of(uuid).unwrap().is_some()));
        assert_eq!(lines.count(), 1_603);
        assert_eq!(cell.borrow_mut().hosted(1_000).unwrap(), held_events[1_000]);
        drop(cell);
        let _ = clear(&root);
    }

    /// Final events kept by their bases are numbered as `names::Identities`
    /// numbers them one at a time: the first of a base keeps it, each after
    /// takes its base and how many came before — over many alike, and over
    /// more than a page of them.
    #[test]
    fn finals_are_numbered_as_one_at_a_time() {
        let (root, mut spill, _one) = spill("numbered");
        spill.begin().unwrap();
        let at = jiff::Timestamp::from_second(1_800_000_000).unwrap();
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut bases = Vec::new();
        for n in 0..12_000u32 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Half of a handful of bases, many alike; the rest each apart.
            let mut base = [0u8; 32];
            if state.is_multiple_of(2) {
                base[0] = (state % 5) as u8 + 1;
            } else {
                base[1..5].copy_from_slice(&n.to_le_bytes());
            }
            bases.push(base);
            spill.push_final(base, at, vec![1], None, None).unwrap();
        }
        spill.number_again().unwrap();
        let mut seen: HashMap<[u8; 32], u32> = HashMap::new();
        let expected: Vec<Vec<u8>> = bases
            .iter()
            .map(|base| {
                let before = seen.entry(*base).or_insert(0);
                let identity = super::super::names::again(*base, *before);
                *before += 1;
                identity.to_vec()
            })
            .collect();
        let repeats: u32 = seen.values().map(|count| count - 1).sum();
        assert!(repeats > 4_096, "more than a page numbered again: {repeats}");
        let places: Vec<usize> = (0..bases.len()).collect();
        assert_eq!(spill.identities_at(&places).unwrap(), expected);
        drop(spill);
        let _ = clear(&root);
    }

    /// Events asked for in an order that turns back are read once each, not
    /// a batch again for each turn back; in an order that rises, a batch at
    /// a time.
    #[test]
    fn an_order_turning_back_reads_each_once() {
        use fastctx_types::session::{Happened, Health, HealthKind};
        let at = jiff::Timestamp::from_second(1_800_000_000).unwrap();
        let events: Vec<SessionEvent> = (0..3_000)
            .map(|n| SessionEvent { at, happened: Happened::Health { turn: None, health: Health { kind: HealthKind::Warning, text: n.to_string() } } })
            .collect();
        let read = |order: &mut dyn Iterator<Item = usize>| {
            let (mut ahead, mut fetched) = (Ahead::default(), 0usize);
            for index in order {
                let event = ahead
                    .take(index, &mut |from, alone| {
                        let end = if alone { from + 1 } else { (from + 500).min(events.len()) };
                        fetched += end - from;
                        Ok(events[from..end].to_vec())
                    })
                    .unwrap();
                assert_eq!(event, events[index]);
            }
            fetched
        };
        assert!(read(&mut (0..3_000).rev()) <= 3_000 + 500);
        assert_eq!(read(&mut (0..3_000)), 3_000);
    }

    /// While a new holder is admitted, no reading begins to be kept.
    #[test]
    fn held_off_while_a_holder_is_admitted() {
        let (root, mut spill, _one) = spill("held-off");
        let held = hold_off();
        assert!(spill.begin().is_err());
        assert!(enter().is_none());
        drop(held);
        spill.begin().unwrap();
        assert!(kept() >= 1);
        drop(spill);
        let _ = clear(&root);
    }
}
