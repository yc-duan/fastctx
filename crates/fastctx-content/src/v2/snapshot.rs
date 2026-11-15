//! A stream's recent window and the pages below it (design §9, §28.2): what
//! a hub gives of a stream as it stands, read in one transaction, and how a
//! light device takes it in. A device that holds whole streams takes their
//! frames in order instead (§28.8): what is judged by a record's item as it
//! stood before is judged right only after every frame before it.
//!
//! A window is the stream's newest items, by order key, as they stand at
//! the stream's head H, with H itself: its sequence, chain value, bytes and
//! where it was last indexed again. Over the keys from its lowest up it is
//! the whole truth at H. A device installs it: its items as answered at H
//! (`items::apply_answered`); an item of those keys it holds that the window
//! has not, from a frame before H, did not stand there at H — deleted, or
//! moved below the window — so its row goes and a revision floor at H takes
//! the place: nothing answered before H brings it back, whatever is answered
//! at H or later does (§29 3.3, PH01). Its items go in first, what it has
//! not a batch at a time, the tail last, what is left to do kept as it goes
//! (P12). The stream's frames then go on from H; what lies below the window
//! comes a page at a time, as it is read, and what is held whole is noted
//! (`coverage`, PH02).
//!
//! A page is items below a key, as they stand when the hub answers, with
//! the stream's sequence then: one that stands before where the device's
//! window began says nothing of what it holds and is dropped, and one asked
//! for before floors it could undo went is asked for again.

use super::Library;
use super::frame::KindRecord;
use super::items::{Applying, FLOOR, IS, apply_answered, unrefer, written};
use super::objects::present;
use super::reads::Entry;
use super::scopes::held;
use crate::error::invalid;
use crate::record::{Record, State, pack, unpack};
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::types::Value;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// How much a window holds at most: items, and bytes of their bodies; at
/// least one item whatever its size.
pub const WINDOW_ITEMS: usize = 200;
pub const WINDOW_BYTES: usize = 1 << 20;

/// A stream's newest items as they stand at its head (design §9 item 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub scope: String,
    pub stream: String,
    /// The owner library's generation it was read in.
    pub generation: String,
    /// At H: the sequence the stream's next frame takes, its chain value,
    /// the bytes of its frames and the change it was last indexed again at.
    pub next: u64,
    pub head: [u8; 32],
    pub bytes: u64,
    pub remade: u64,
    /// The lowest (order key, identity) it holds, the hub holding more
    /// below; none when it holds every item of the stream.
    pub low: Option<(Vec<u8>, Vec<u8>)>,
    /// Its items, newest first.
    pub entries: Vec<Entry>,
}

/// Items below a key as they stood when the hub answered (§9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Older {
    pub scope: String,
    pub stream: String,
    /// The sequence the stream's next frame took then.
    pub through: u64,
    /// What it is below: (order key, identity), the top when none.
    pub before: Option<(Vec<u8>, Vec<u8>)>,
    /// Newest first.
    pub entries: Vec<Entry>,
    /// Whether the hub holds more below the last.
    pub more: bool,
    /// How many times the hub's stream was indexed again as it answered.
    pub remade: u64,
}

/// What installing a window did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installed {
    /// The stream here is at its head or past it: nothing to do.
    Behind,
    /// Installed; frames go on from its head.
    Installed,
}

/// Rows of what a window lacks taken at once, a batch at a time (G09).
const HIDDEN_AT_ONCE: i64 = 1_000;

/// A held row a window has not: order key, identity, revision and packed
/// objects.
type Unstood = (Vec<u8>, Vec<u8>, i64, Option<Vec<u8>>);

fn put(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

impl Window {
    /// What the hub's word on this window says: everything a device takes
    /// from it.
    pub fn statement(&self) -> Vec<u8> {
        let mut digest = Sha256::new();
        put(&mut digest, b"fastctx-window/1");
        put(&mut digest, self.scope.as_bytes());
        put(&mut digest, self.stream.as_bytes());
        put(&mut digest, self.generation.as_bytes());
        digest.update(self.next.to_le_bytes());
        digest.update(self.head);
        digest.update(self.bytes.to_le_bytes());
        digest.update(self.remade.to_le_bytes());
        match &self.low {
            Some((place, item)) => {
                digest.update([1]);
                put(&mut digest, place);
                put(&mut digest, item);
            }
            None => digest.update([0]),
        }
        for entry in &self.entries {
            put(&mut digest, &entry.item);
            put(&mut digest, &entry.order);
            digest.update(entry.revision.to_le_bytes());
            digest.update(entry.kind.to_le_bytes());
            digest.update(entry.schema.to_le_bytes());
            put(&mut digest, &entry.body);
            put(&mut digest, &pack(&entry.objects));
        }
        let mut statement = b"fastctx-window/1\0".to_vec();
        statement.extend_from_slice(&digest.finalize());
        statement
    }
}

/// Up to `items` newest items of `stream` holding at most about `bytes`,
/// newest first, below (`before`) when given, and whether more are below.
fn newest(
    db: &Connection,
    stream: i64,
    holding: i64,
    before: Option<&(Vec<u8>, Vec<u8>)>,
    items: usize,
    bytes: usize,
) -> fastctx_storage::Result<(Vec<Entry>, bool)> {
    // Below `before` as a plain bound, which SQLite seeks in the index, not
    // one it walks to from the last item (`reads::seek`).
    let mut values = vec![Value::Integer(stream), Value::Integer(holding), Value::Integer(IS), Value::Integer(items as i64 + 1)];
    let below = match before {
        Some((place, item)) => {
            values.extend([Value::Blob(place.clone()), Value::Blob(item.clone())]);
            "AND (place, item) < (?5, ?6)"
        }
        None => "",
    };
    let mut statement = db.prepare_cached(&format!(
        "SELECT item, place, revision, kind, schema, body, objects, local FROM items
         WHERE stream = ?1 AND holding = ?2 AND state = ?3 {below}
         ORDER BY place DESC, item DESC LIMIT ?4"
    ))?;
    let mut rows = statement.query(params_from_iter(values))?;
    let (mut entries, mut taken) = (Vec::new(), 0usize);
    while let Some(row) = rows.next()? {
        let body: Vec<u8> = row.get::<_, Option<Vec<u8>>>(5)?.unwrap_or_default();
        if entries.len() == items || (!entries.is_empty() && taken + body.len() > bytes) {
            return Ok((entries, true));
        }
        taken += body.len();
        let objects: Vec<u8> = row.get::<_, Option<Vec<u8>>>(6)?.unwrap_or_default();
        entries.push(Entry {
            item: row.get(0)?,
            order: row.get(1)?,
            revision: row.get::<_, i64>(2)? as u64,
            kind: row.get(3)?,
            schema: row.get(4)?,
            body,
            objects: unpack(&objects).map_err(invalid)?,
            local: row.get::<_, i64>(7)? as u64,
        });
    }
    Ok((entries, false))
}

/// The objects `entries` refer to that are not here, wanted for `scope`
/// as an imported frame's are.
fn want(db: &Connection, scope: i64, key: &str, entries: &[Entry]) -> fastctx_storage::Result<()> {
    for entry in entries {
        for object in &entry.objects {
            if present(db, &object.hash)?.is_none() {
                db.prepare_cached("INSERT OR IGNORE INTO wanted (hash, size, role, priority, hint) VALUES (?1, ?2, 'record', 1, ?3)")?
                    .execute(params![&object.hash[..], object.size as i64, key])?;
                db.prepare_cached("INSERT OR IGNORE INTO wanted_in (scope, hash) VALUES (?1, ?2)")?
                    .execute(params![scope, &object.hash[..]])?;
            }
        }
    }
    Ok(())
}

/// Hides what `stream` holds between `low` and `high` (order key and
/// identity, unbounded when none; `high` excluded) from a frame before
/// `through` that is not among `standing`: an answer that is whole over
/// those keys at `through` does not have it there — deleted, or moved out
/// of them. Its row goes, and a revision floor at `through` takes the place
/// (PH01): nothing answered before that moment brings it back, what is
/// answered at it or later does, and a frame from then on. A batch of at
/// most `limit` rows from after `after`: where the batch ended when it was
/// full, none when nothing is left.
#[allow(clippy::too_many_arguments)]
fn hide_unstood(
    db: &Connection,
    scope: i64,
    stream: i64,
    holding: i64,
    standing: &HashSet<Vec<u8>>,
    low: Option<&(Vec<u8>, Vec<u8>)>,
    high: Option<&(Vec<u8>, Vec<u8>)>,
    through: u64,
    after: Option<&(Vec<u8>, Vec<u8>)>,
    limit: i64,
) -> fastctx_storage::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let rows: Vec<Unstood> = {
        // Each bound a plain one, which SQLite seeks in the index rather
        // than walking to it from the first item for every batch
        // (`reads::seek`): past where the batch before ended — which was at
        // `low` or after it — else from `low`; before `high`.
        let mut values = vec![Value::Integer(stream), Value::Integer(holding), Value::Integer(IS), Value::Integer(through as i64), Value::Integer(limit)];
        let mut bounds = String::new();
        let mut bound = |values: &mut Vec<Value>, relation: &str, (place, item): &(Vec<u8>, Vec<u8>)| {
            values.extend([Value::Blob(place.clone()), Value::Blob(item.clone())]);
            bounds.push_str(&format!(" AND (place, item) {relation} (?{}, ?{})", values.len() - 1, values.len()));
        };
        match (after, low) {
            (Some(after), _) => bound(&mut values, ">", after),
            (None, Some(low)) => bound(&mut values, ">=", low),
            (None, None) => {}
        }
        if let Some(high) = high {
            bound(&mut values, "<", high);
        }
        let mut statement = db.prepare_cached(&format!(
            "SELECT place, item, revision, objects FROM items
             WHERE stream = ?1 AND holding = ?2 AND state = ?3 AND revision < ?4{bounds}
             ORDER BY place, item LIMIT ?5"
        ))?;
        let rows = statement.query_map(params_from_iter(values), |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let full = rows.len() as i64 == limit;
    let last = rows.last().map(|last| (last.0.clone(), last.1.clone()));
    for (place, item, _, objects) in rows.into_iter().filter(|(_, item, _, _)| !standing.contains(item)) {
        if let Some(objects) = objects {
            for object in unpack(&objects).map_err(invalid)? {
                unrefer(db, scope, holding, stream, &object.hash)?;
            }
        }
        let local = written(db, stream)?;
        db.prepare_cached(
            "UPDATE items SET state = ?4, revision = ?5, body = NULL, objects = NULL, local = ?6
             WHERE stream = ?1 AND place = ?2 AND item = ?3",
        )?
        .execute(params![stream, place, item, FLOOR, through as i64, local])?;
    }
    Ok(if full { last } else { None })
}

fn record(entry: &Entry) -> KindRecord {
    KindRecord {
        kind: entry.kind,
        schema: entry.schema,
        record: Record {
            item: entry.item.clone(),
            order: entry.order.clone(),
            state: State::Is { body: entry.body.clone(), objects: entry.objects.clone() },
        },
    }
}

/// The identities of a window's items, as `installing.standing` keeps them.
fn pack_standing(standing: &HashSet<Vec<u8>>) -> Vec<u8> {
    let mut packed = Vec::new();
    for item in standing {
        packed.extend_from_slice(&(item.len() as u16).to_be_bytes());
        packed.extend_from_slice(item);
    }
    packed
}

fn unpack_standing(mut packed: &[u8]) -> fastctx_storage::Result<HashSet<Vec<u8>>> {
    let mut standing = HashSet::new();
    while !packed.is_empty() {
        let [high, low, rest @ ..] = packed else { return Err(invalid("a malformed install")) };
        let length = u16::from_be_bytes([*high, *low]) as usize;
        if rest.len() < length {
            return Err(invalid("a malformed install"));
        }
        standing.insert(rest[..length].to_vec());
        packed = &rest[length..];
    }
    Ok(standing)
}

/// An order key and identity of two columns that may be empty.
fn key(place: Option<Vec<u8>>, item: Option<Vec<u8>>) -> Option<(Vec<u8>, Vec<u8>)> {
    Some((place?, item?))
}

/// A window being installed, as `installing` keeps it.
struct Installing {
    scope: i64,
    holding: i64,
    next: u64,
    head: Vec<u8>,
    remade: i64,
    low: Option<(Vec<u8>, Vec<u8>)>,
    standing: HashSet<Vec<u8>>,
    after: Option<(Vec<u8>, Vec<u8>)>,
}

fn installing(db: &Connection, stream: i64) -> fastctx_storage::Result<Option<Installing>> {
    let row = db
        .prepare_cached(
            "SELECT s.scope, i.holding, i.next, i.head, i.remade, i.low_place, i.low_item, i.standing, i.after_place, i.after_item
             FROM installing i JOIN streams s ON s.id = i.stream WHERE i.stream = ?1",
        )?
        .query_row([stream], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, i64>(4)?,
                key(row.get(5)?, row.get(6)?),
                row.get::<_, Vec<u8>>(7)?,
                key(row.get(8)?, row.get(9)?),
            ))
        })
        .optional()?;
    let Some((scope, holding, next, head, remade, low, standing, after)) = row else { return Ok(None) };
    Ok(Some(Installing { scope, holding, next: next as u64, head, remade, low, standing: unpack_standing(&standing)?, after }))
}

/// One covered run of a stream a light device holds (`coverage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Covered {
    /// From this (order key, identity); empty below everything.
    pub low: (Vec<u8>, Vec<u8>),
    /// Up to this one, excluded; none above everything.
    pub high: Option<(Vec<u8>, Vec<u8>)>,
    /// The sequence the hub's stream was to take next as it answered.
    pub through: u64,
    /// How many times the hub's stream was made as it answered.
    pub remade: u64,
}

/// What became of an older page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OlderTaken {
    Taken,
    /// It stands before where the stream's window here began: it says
    /// nothing of what is held.
    Before,
    /// Floors it could have undone went after it was asked for (PH01): ask
    /// for it again.
    Stale,
}

/// Notes `low` up to `high` of `stream` covered as answered at `through`.
#[allow(clippy::too_many_arguments)]
fn cover(
    db: &Connection,
    stream: i64,
    holding: i64,
    remade: i64,
    low: Option<&(Vec<u8>, Vec<u8>)>,
    high: Option<&(Vec<u8>, Vec<u8>)>,
    through: u64,
) -> fastctx_storage::Result<()> {
    let (low_place, low_item) = low.cloned().unwrap_or_default();
    let (high_place, high_item) = match high {
        Some((place, item)) => (Some(place.clone()), Some(item.clone())),
        None => (None, None),
    };
    db.prepare_cached(
        "INSERT INTO coverage (stream, holding, remade, low_place, low_item, high_place, high_item, through)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (stream, holding, low_place, low_item) DO UPDATE SET
           remade = excluded.remade, high_place = excluded.high_place, high_item = excluded.high_item, through = excluded.through",
    )?
    .execute(params![stream, holding, remade, low_place, low_item, high_place, high_item, through as i64])?;
    Ok(())
}

/// The bytes of bodies an older page holds at most, at least one item
/// whatever its size (PH05).
pub const OLDER_BYTES: usize = 4 << 20;

impl Library {
    /// `stream` of `scope` as it stands now, its newest items up to `items`
    /// and about `bytes`, read in one transaction; none when the scope or
    /// stream is not here. On a hub's owner library.
    pub fn window(&self, scope: &str, stream: &str, items: usize, bytes: usize) -> Result<Option<Window>> {
        let generation = self.generation()?;
        let (key, name) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            // The head and the items as of one moment (AL05).
            let snapshot = db.unchecked_transaction()?;
            let db: &Connection = &snapshot;
            let Some((scope_id, _, Some(holding))) = held(db, &key)? else { return Ok(None) };
            let Some((stream_id, next, head)): Option<(i64, i64, Option<Vec<u8>>)> = db
                .prepare_cached("SELECT id, next, head FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .optional()?
            else {
                return Ok(None);
            };
            let head: [u8; 32] = head.unwrap_or_else(|| vec![0; 32]).try_into().map_err(|_| invalid("a malformed stream head"))?;
            let (total, remade): (i64, i64) = db
                .prepare_cached("SELECT bytes, remade FROM stream_changes WHERE stream = ?1")?
                .query_row([stream_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?
                .unwrap_or((0, 0));
            let (entries, more) = newest(db, stream_id, holding, None, items.max(1), bytes)?;
            let low = more.then(|| entries.last().map(|last| (last.order.clone(), last.item.clone()))).flatten();
            Ok(Some(Window {
                scope: key,
                stream: name,
                generation,
                next: next as u64,
                head,
                bytes: total as u64,
                remade: remade as u64,
                low,
                entries,
            }))
        })?)
    }

    /// How many times each stream of `scope` was made again, as its changes
    /// say (`stream_changes.remade`); a stream never made again is absent.
    /// On a hub's owner library.
    pub fn remade_of(&self, scope: &str) -> Result<std::collections::BTreeMap<String, u64>> {
        let key = scope.to_string();
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(_))) = held(db, &key)? else { return Ok(Default::default()) };
            let mut statement = db.prepare_cached(
                "SELECT s.name, c.remade FROM streams s JOIN stream_changes c ON c.stream = s.id WHERE s.scope = ?1 AND c.remade > 0",
            )?;
            let rows = statement.query_map([scope_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Up to `limit` items of `stream` of `scope` below `before` (its order
    /// key and identity), newest first, of at most `OLDER_BYTES` of bodies,
    /// as they stand now, with the sequence the stream is at; none when it
    /// is not here. On a hub's owner library.
    pub fn older(&self, scope: &str, stream: &str, before: Option<(Vec<u8>, Vec<u8>)>, limit: usize) -> Result<Option<Older>> {
        let (key, name) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            // The head and the items as of one moment (AL05).
            let snapshot = db.unchecked_transaction()?;
            let db: &Connection = &snapshot;
            let Some((scope_id, _, Some(holding))) = held(db, &key)? else { return Ok(None) };
            let Some((stream_id, next)): Option<(i64, i64)> = db
                .prepare_cached("SELECT id, next FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, name], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?
            else {
                return Ok(None);
            };
            let remade: i64 = db
                .prepare_cached("SELECT remade FROM stream_changes WHERE stream = ?1")?
                .query_row([stream_id], |row| row.get(0))
                .optional()?
                .unwrap_or(0);
            let (entries, more) =
                newest(db, stream_id, holding, before.as_ref(), limit.clamp(1, super::reads::PAGE), OLDER_BYTES)?;
            Ok(Some(Older { scope: key, stream: name, through: next as u64, before, entries, more, remade: remade as u64 }))
        })?)
    }

    /// Installs `window`, which the hub's word stood behind, into the
    /// holding period `holding` it was asked for in, on a light device
    /// (design §9, §28.2, §29 3.3): its items as answered at its head; each
    /// held item of its keys it has not, from a frame before its head,
    /// hidden behind a floor at its head; the stream's frames going on from
    /// its head, what is below it read a page at a time. Its items go in
    /// first with what is left to do (`installing`), what it has not a batch
    /// at a time with other writes between, the tail last (P12): a device
    /// stopped meanwhile goes on with `resume_installs`. A window at the head
    /// already here is nothing to it unless `again`: the hub indexed the
    /// stream's records again, or its library is of another generation
    /// (§28.1, G01).
    pub fn install_window(&self, window: Window, holding: i64, again: bool) -> Result<Installed> {
        self.install_window_until(window, holding, again, None)
    }

    /// As `install_window`, stopping after `batches` of what it hides as a
    /// device stopped then would: for the drills, which go on with
    /// `resume_installs`.
    #[cfg(feature = "drill")]
    pub fn install_window_stopping(&self, window: Window, holding: i64, batches: usize) -> Result<Installed> {
        self.install_window_until(window, holding, false, Some(batches))
    }

    fn install_window_until(&self, window: Window, holding: i64, again: bool, stop: Option<usize>) -> Result<Installed> {
        let bytes = window.entries.iter().map(|entry| entry.body.len() + 256).sum::<usize>() + 4096;
        let begun = self.database().write_now_at(Priority::Normal, bytes, move |db| {
            let Some((scope_id, state, Some(held_in))) = held(db, &window.scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" || held_in != holding {
                return Err(invalid("the holding period the window was asked for in has ended"));
            }
            let stream = super::exchange::stream_id(db, scope_id, &window.stream)?;
            let here: u64 = db
                .prepare_cached("SELECT sequence FROM cursors WHERE stream = ?1 AND holding = ?2")?
                .query_row(params![stream, holding], |row| row.get::<_, i64>(0))
                .optional()?
                .map_or(0, |sequence| sequence as u64 + 1);
            if here > window.next || (here == window.next && !again) {
                return Ok(None);
            }
            let at = Applying { scope: scope_id, stream, holding };
            want(db, scope_id, &window.scope, &window.entries)?;
            // A backfill below an earlier window goes: below this one is
            // taken from its own head, if at all.
            db.prepare_cached("DELETE FROM backfills WHERE stream = ?1")?.execute([stream])?;
            let mut standing = HashSet::new();
            for entry in &window.entries {
                standing.insert(entry.item.clone());
                apply_answered(db, at, entry.revision, window.next, &record(entry))?;
            }
            // A window installed meanwhile gives way to this one.
            let (low_place, low_item) = match &window.low {
                Some((place, item)) => (Some(place.clone()), Some(item.clone())),
                None => (None, None),
            };
            db.prepare_cached(
                "INSERT OR REPLACE INTO installing
                 (stream, holding, next, head, remade, low_place, low_item, standing, after_place, after_item)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL)",
            )?
            .execute(params![
                stream,
                holding,
                window.next as i64,
                &window.head[..],
                window.remade as i64,
                low_place,
                low_item,
                pack_standing(&standing)
            ])?;
            Ok(Some(stream))
        })?;
        let Some(stream) = begun else { return Ok(Installed::Behind) };
        self.wrote();
        self.finish_install(stream, stop)?;
        Ok(Installed::Installed)
    }

    /// Goes on with the windows being installed when the device stopped.
    pub fn resume_installs(&self) -> Result<usize> {
        let streams: Vec<i64> = self.database().read_now(|db| {
            let mut statement = db.prepare_cached("SELECT stream FROM installing ORDER BY stream")?;
            let streams = statement.query_map([], |row| row.get(0))?;
            Ok(streams.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        for stream in &streams {
            self.finish_install(*stream, None)?;
        }
        Ok(streams.len())
    }

    /// Hides what the window being installed into `stream` has not, a batch
    /// a transaction, then moves the tail to its head; stopping after `stop`
    /// batches, if given, with what is left kept.
    fn finish_install(&self, stream: i64, stop: Option<usize>) -> Result<()> {
        let mut batches = 0usize;
        loop {
            if stop.is_some_and(|stop| batches >= stop) {
                return Ok(());
            }
            batches += 1;
            let done = self.database().write_now_at(Priority::Normal, 64 * 1024, move |db| {
                let Some(installing) = installing(db, stream)? else { return Ok(true) };
                let holding: Option<(String, Option<i64>)> = db
                    .prepare_cached("SELECT state, holding FROM scopes WHERE id = ?1")?
                    .query_row([installing.scope], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?;
                // Its holding period ended: what it would install goes with it.
                if !matches!(&holding, Some((state, Some(held))) if state == "live" && *held == installing.holding) {
                    db.prepare_cached("DELETE FROM installing WHERE stream = ?1")?.execute([stream])?;
                    return Ok(true);
                }
                let left = hide_unstood(
                    db,
                    installing.scope,
                    stream,
                    installing.holding,
                    &installing.standing,
                    installing.low.as_ref(),
                    None,
                    installing.next,
                    installing.after.as_ref(),
                    HIDDEN_AT_ONCE,
                )?;
                if let Some((place, item)) = left {
                    db.prepare_cached("UPDATE installing SET after_place = ?2, after_item = ?3 WHERE stream = ?1")?
                        .execute(params![stream, place, item])?;
                    return Ok(false);
                }
                // All of it in: frames go on from its head; what is below it
                // is not here.
                let (next, head) = (installing.next as i64, installing.head.clone());
                // Frames do not come into the stream while it is installed
                // (`exchange.rs`); a cursor past the head is never moved back.
                db.prepare_cached(
                    "INSERT INTO cursors (stream, holding, sequence, head) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (stream, holding) DO UPDATE SET sequence = excluded.sequence, head = excluded.head
                     WHERE excluded.sequence > cursors.sequence",
                )?
                .execute(params![stream, installing.holding, next - 1, head])?;
                db.prepare_cached("UPDATE streams SET next = MAX(next, ?2), head = ?3 WHERE id = ?1")?
                    .execute(params![stream, next, head])?;
                db.prepare_cached(
                    "INSERT INTO retention (stream, floor, head) VALUES (?1, ?2, ?3)
                     ON CONFLICT (stream) DO UPDATE SET floor = excluded.floor, head = excluded.head",
                )?
                .execute(params![stream, next, head])?;
                // What is covered of another making of the hub's stream is
                // not any more (PH07); what is below the window shows as it
                // is until it is read again.
                db.prepare_cached("DELETE FROM coverage WHERE stream = ?1 AND (holding != ?2 OR remade != ?3)")?
                    .execute(params![stream, installing.holding, installing.remade])?;
                cover(db, stream, installing.holding, installing.remade, installing.low.as_ref(), None, installing.next)?;
                db.prepare_cached("DELETE FROM installing WHERE stream = ?1")?.execute([stream])?;
                Ok(true)
            })?;
            self.wrote();
            if done {
                return Ok(());
            }
        }
    }

    /// What an older page asked for now is to be judged by (`install_older`):
    /// the library's last write number.
    pub fn page_token(&self) -> Result<u64> {
        Ok(self.database().read_now(|db| Ok(super::items::last_write(db)? as u64))?)
    }

    /// Takes in `older` for the holding period `holding`, asked for when
    /// `page_token` was `asked`: its items as answered at its `through`, and
    /// what it has not over its keys behind a floor there (PH01). One that
    /// stands before where this stream's window began is dropped; one asked
    /// for before floors of the stream went is to be asked for again.
    pub fn install_older(&self, older: Older, holding: i64, asked: u64) -> Result<OlderTaken> {
        let bytes = older.entries.iter().map(|entry| entry.body.len() + 256).sum::<usize>() + 1024;
        let taken = self.database().write_now_at(Priority::Normal, bytes, move |db| {
            let Some((scope_id, state, Some(held_in))) = held(db, &older.scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" || held_in != holding {
                return Err(invalid("the holding period the page was asked for in has ended"));
            }
            let stream = super::exchange::stream_id(db, scope_id, &older.stream)?;
            // A stream withheld since the page was asked takes none of it.
            let withheld: bool = db
                .prepare_cached("SELECT EXISTS (SELECT 1 FROM withheld WHERE stream = ?1 AND holding = ?2)")?
                .query_row(params![stream, holding], |row| row.get(0))?;
            if withheld {
                return Ok(OlderTaken::Stale);
            }
            let reclaimed: i64 =
                db.prepare_cached("SELECT reclaimed FROM streams WHERE id = ?1")?.query_row([stream], |row| row.get(0))?;
            if reclaimed as u64 > asked {
                return Ok(OlderTaken::Stale);
            }
            // Of another making of the hub's stream than what is covered
            // here: what it shows absent or present says nothing of this
            // index (§29 3.3).
            let other: bool = db
                .prepare_cached("SELECT EXISTS (SELECT 1 FROM coverage WHERE stream = ?1 AND holding = ?2 AND remade != ?3)")?
                .query_row(params![stream, holding, older.remade as i64], |row| row.get(0))?;
            if other {
                return Ok(OlderTaken::Stale);
            }
            let began: Option<i64> =
                db.prepare_cached("SELECT floor FROM retention WHERE stream = ?1")?.query_row([stream], |row| row.get(0)).optional()?;
            if began.is_some_and(|began| (older.through as i64) < began) {
                return Ok(OlderTaken::Before);
            }
            want(db, scope_id, &older.scope, &older.entries)?;
            let at = Applying { scope: scope_id, stream, holding };
            let mut standing = HashSet::new();
            for entry in &older.entries {
                standing.insert(entry.item.clone());
                apply_answered(db, at, entry.revision, older.through, &record(entry))?;
            }
            // Whole over its keys as it was answered: from its last down to
            // the first when nothing is below, up to what it is below.
            let low = if older.more { older.entries.last().map(|last| (last.order.clone(), last.item.clone())) } else { None };
            if older.more && low.is_none() {
                return Ok(OlderTaken::Taken);
            }
            let mut after = None;
            loop {
                after = hide_unstood(
                    db,
                    scope_id,
                    stream,
                    holding,
                    &standing,
                    low.as_ref(),
                    older.before.as_ref(),
                    older.through,
                    after.as_ref(),
                    HIDDEN_AT_ONCE,
                )?;
                if after.is_none() {
                    break;
                }
            }
            cover(db, stream, holding, older.remade as i64, low.as_ref(), older.before.as_ref(), older.through)?;
            Ok(OlderTaken::Taken)
        })?;
        if taken == OlderTaken::Taken {
            self.wrote();
        }
        Ok(taken)
    }

    /// What of `stream` of `scope` is held whole here, in its holding period
    /// now, lowest first.
    pub fn coverage(&self, scope: &str, stream: &str) -> Result<Vec<Covered>> {
        let (scope_key, name) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope_key)? else { return Ok(Vec::new()) };
            let mut statement = db.prepare_cached(
                "SELECT c.low_place, c.low_item, c.high_place, c.high_item, c.through, c.remade FROM coverage c
                 JOIN streams s ON s.id = c.stream WHERE s.scope = ?1 AND s.name = ?2 AND c.holding = ?3
                 ORDER BY c.low_place, c.low_item",
            )?;
            let rows = statement.query_map(params![scope_id, name, holding], |row| {
                Ok(Covered {
                    low: (row.get(0)?, row.get(1)?),
                    high: key(row.get(2)?, row.get(3)?),
                    through: row.get::<_, i64>(4)? as u64,
                    remade: row.get::<_, i64>(5)? as u64,
                })
            })?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?)
    }
}
