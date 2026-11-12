//! A stream's recent window and the pages below it (design §9, §28.2): what
//! a hub gives of a stream as it stands, read in one transaction, and how a
//! light device takes it in. A device that holds whole streams takes their
//! frames in order instead (§28.8): what is judged by a record's item as it
//! stood before is judged right only after every frame before it.
//!
//! A window is the stream's newest items, by order key, as they stand at
//! the stream's head H, with H itself: its sequence, chain value, bytes and
//! where it was last indexed again. Over the keys from its lowest up it is
//! the whole truth at H. A device installs it in one transaction: its items
//! by the revision rule; an item of those keys it holds that the window has
//! not, from a frame before H, did not stand there at H — deleted, or moved
//! below the window — so its row goes and a revision floor just past its
//! own takes the place: nothing as old comes back, whatever is newer does.
//! The stream's frames then go on from H; what lies below the window comes
//! a page at a time, as it is read.
//!
//! A page is items below a key, as they stand when the hub answers, with
//! the stream's sequence then: one that stands before where the device's
//! window began says nothing of what it holds and is dropped.

use super::Library;
use super::frame::KindRecord;
use super::items::{Applying, FLOOR, IS, apply, unrefer};
use super::objects::present;
use super::reads::Entry;
use super::scopes::held;
use crate::error::invalid;
use crate::record::{Record, State, pack, unpack};
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
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
    let mut statement = db.prepare_cached(
        "SELECT item, place, revision, kind, schema, body, objects FROM items
         WHERE stream = ?1 AND holding = ?2 AND state = ?3 AND (?4 = 0 OR (place, item) < (?5, ?6))
         ORDER BY place DESC, item DESC LIMIT ?7",
    )?;
    let (started, (place, item)) = (before.is_some(), before.cloned().unwrap_or_default());
    let mut rows = statement.query(params![stream, holding, IS, started, place, item, items as i64 + 1])?;
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
/// of them. Its row goes, and a revision floor just past its own takes the
/// place: nothing as old comes back, the item as it is now does.
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
) -> fastctx_storage::Result<()> {
    let (low_place, low_item) = low.cloned().unwrap_or_default();
    let (high_place, high_item) = high.cloned().unwrap_or_default();
    // A batch at a time, in key order from where the last ended: what is
    // held in memory stays bounded however much is cached here (G09).
    let mut after: Option<(Vec<u8>, Vec<u8>)> = None;
    loop {
        let rows: Vec<Unstood> = {
            let mut statement = db.prepare_cached(
                "SELECT place, item, revision, objects FROM items
                 WHERE stream = ?1 AND holding = ?2 AND state = ?3 AND revision < ?4
                   AND (?5 = 0 OR (place, item) >= (?6, ?7)) AND (?8 = 0 OR (place, item) < (?9, ?10))
                   AND (?11 = 0 OR (place, item) > (?12, ?13))
                 ORDER BY place, item LIMIT ?14",
            )?;
            let (going, (after_place, after_item)) = (after.is_some(), after.clone().unwrap_or_default());
            let rows = statement.query_map(
                params![
                    stream,
                    holding,
                    IS,
                    through as i64,
                    low.is_some(),
                    low_place,
                    low_item,
                    high.is_some(),
                    high_place,
                    high_item,
                    going,
                    after_place,
                    after_item,
                    HIDDEN_AT_ONCE
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let Some(last) = rows.last() else { return Ok(()) };
        after = Some((last.0.clone(), last.1.clone()));
        let full = rows.len() as i64 == HIDDEN_AT_ONCE;
        for (place, item, revision, objects) in rows.into_iter().filter(|(_, item, _, _)| !standing.contains(item)) {
            if let Some(objects) = objects {
                for object in unpack(&objects).map_err(invalid)? {
                    unrefer(db, scope, holding, stream, &object.hash)?;
                }
            }
            db.prepare_cached("UPDATE items SET state = ?4, revision = ?5, body = NULL, objects = NULL WHERE stream = ?1 AND place = ?2 AND item = ?3")?
                .execute(params![stream, place, item, FLOOR, revision + 1])?;
        }
        if !full {
            return Ok(());
        }
    }
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

    /// Up to `limit` items of `stream` of `scope` below `before` (its order
    /// key and identity), newest first, as they stand now, with the
    /// sequence the stream is at; none when it is not here. On a hub's
    /// owner library.
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
            let (entries, more) = newest(db, stream_id, holding, before.as_ref(), limit.clamp(1, super::reads::PAGE), usize::MAX)?;
            Ok(Some(Older { scope: key, stream: name, through: next as u64, before, entries, more }))
        })?)
    }

    /// Installs `window`, which the hub's word stood behind, into the
    /// holding period `holding` it was asked for in, on a light device
    /// (design §9, §28.2): its items by the revision rule; each held item of
    /// its keys it has not, from a frame before its head, hidden behind a
    /// floor just past its own revision; the stream's frames going on from
    /// its head, what is below it read a page at a time. A window at the head
    /// already here is nothing to it unless `again`: the hub indexed the
    /// stream's records again, or its library is of another generation
    /// (§28.1, G01).
    pub fn install_window(&self, window: Window, holding: i64, again: bool) -> Result<Installed> {
        let bytes = window.entries.iter().map(|entry| entry.body.len() + 256).sum::<usize>() + 4096;
        let installed = self.database().write_now_at(Priority::Normal, bytes, move |db| {
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
                return Ok(Installed::Behind);
            }
            let at = Applying { scope: scope_id, stream, holding };
            want(db, scope_id, &window.scope, &window.entries)?;
            let mut standing = HashSet::new();
            for entry in &window.entries {
                standing.insert(entry.item.clone());
                apply(db, at, entry.revision, &record(entry))?;
            }
            // What stood in its keys here, from before its head, and does
            // not there.
            hide_unstood(db, scope_id, stream, holding, &standing, window.low.as_ref(), None, window.next)?;
            // Frames go on from its head; what is below it is not here.
            db.prepare_cached(
                "INSERT INTO cursors (stream, holding, sequence, head) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (stream, holding) DO UPDATE SET sequence = excluded.sequence, head = excluded.head",
            )?
            .execute(params![stream, holding, window.next as i64 - 1, &window.head[..]])?;
            db.prepare_cached("UPDATE streams SET next = MAX(next, ?2), head = ?3 WHERE id = ?1")?
                .execute(params![stream, window.next as i64, &window.head[..]])?;
            db.prepare_cached(
                "INSERT INTO retention (stream, floor, head) VALUES (?1, ?2, ?3)
                 ON CONFLICT (stream) DO UPDATE SET floor = excluded.floor, head = excluded.head",
            )?
            .execute(params![stream, window.next as i64, &window.head[..]])?;
            // What stood before its head came in: readers read anew.
            super::view::rebased(db, stream)?;
            Ok(Installed::Installed)
        })?;
        if installed != Installed::Behind {
            self.wrote();
        }
        Ok(installed)
    }

    /// Takes in `older` for the holding period `holding`: its items by the
    /// revision rule. One that stands before where this stream's window
    /// began is dropped: whether false.
    pub fn install_older(&self, older: Older, holding: i64) -> Result<bool> {
        let bytes = older.entries.iter().map(|entry| entry.body.len() + 256).sum::<usize>() + 1024;
        let taken = self.database().write_now_at(Priority::Normal, bytes, move |db| {
            let Some((scope_id, state, Some(held_in))) = held(db, &older.scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" || held_in != holding {
                return Err(invalid("the holding period the page was asked for in has ended"));
            }
            let stream = super::exchange::stream_id(db, scope_id, &older.stream)?;
            let began: Option<i64> =
                db.prepare_cached("SELECT floor FROM retention WHERE stream = ?1")?.query_row([stream], |row| row.get(0)).optional()?;
            if began.is_some_and(|began| (older.through as i64) < began) {
                return Ok(false);
            }
            want(db, scope_id, &older.scope, &older.entries)?;
            let at = Applying { scope: scope_id, stream, holding };
            let mut standing = HashSet::new();
            for entry in &older.entries {
                standing.insert(entry.item.clone());
                apply(db, at, entry.revision, &record(entry))?;
            }
            // Whole over its keys as it was answered: from its last down to
            // the first when nothing is below, up to what it is below.
            let low = if older.more { older.entries.last().map(|last| (last.order.clone(), last.item.clone())) } else { None };
            if older.more && low.is_none() {
                return Ok(true);
            }
            hide_unstood(db, scope_id, stream, holding, &standing, low.as_ref(), older.before.as_ref(), older.through)?;
            super::view::rebased(db, stream)?;
            Ok(true)
        })?;
        if taken {
            self.wrote();
        }
        Ok(taken)
    }
}

