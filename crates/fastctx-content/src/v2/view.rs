//! What a reader on this device reads of a stream (design §28.3, plan 3-5):
//! a bounded page of its items by order key, what changed after where it
//! read, and a wait until it moves. Reads only read.
//!
//! A reader's place in the stream is one number: in its low 40 bits the last
//! of the library's write numbers a row of the stream took (`items.rs`, §29
//! 3.3 PH04), and above them the stream's era (`streams.rebased`): a number
//! the library gives from one counter that only grows, each time the stream
//! is made, its scope is held anew, or its index is made otherwise than by
//! writing on — cut back, rebuilt as the hub refused a frame, records indexed
//! again (JJ01, JJ02). Every row written takes a new write number, a frame's,
//! a window's, an older page's or a backfill's alike, so what changed after
//! a number is the rows written past it, in whatever order of revisions they
//! came; another era, and the reader reads anew. No two states of a stream
//! share a number, and the number only grows; the era has 23 bits, a library
//! making that many is not one this counts on, and a place's top bit is
//! never set here: a node marks with it the places another machine gave.
//! Nothing written yet is a place whose low bits are naught.

use super::Library;
use super::items::IS;
use super::reads::Entry;
use super::scopes::held;
use crate::error::invalid;
use crate::read::{Anchor, Place};
use crate::record::unpack;
use crate::Result;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, Row, params};
use std::time::Duration;

const SEQUENCE_BITS: u32 = 40;
/// The stamp the library's latest era is kept under.
const ERA: &str = "view_era";

/// A page of a stream as a reader on this device reads it.
#[derive(Clone, Debug, Default)]
pub struct ViewPage {
    /// Where the stream stood as the page was read; none when it is not
    /// held here.
    pub revision: Option<u64>,
    /// In the stream's order, earliest first; each entry's revision is the
    /// sequence of the record that last set it.
    pub entries: Vec<Entry>,
    pub older: Option<Place>,
    pub newer: Option<Place>,
    /// Where changes read on from, for what comes after this page.
    pub through: u64,
}

/// What a stream's index changed to after a reader's place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewChange {
    Is(Entry),
    Gone { item: Vec<u8> },
}

#[derive(Clone, Debug, Default)]
pub struct ViewChanges {
    pub revision: Option<u64>,
    /// In the order of the records that made them.
    pub changes: Vec<ViewChange>,
    pub through: u64,
    pub more: bool,
}

/// The stream `name` of `scope` held here: its id, holding period and where
/// a reader of it stands now.
fn standing(db: &Connection, scope: &str, name: &str) -> fastctx_storage::Result<Option<(i64, i64, u64)>> {
    let Some((scope_id, _, Some(holding))) = held(db, scope)? else { return Ok(None) };
    let Some((stream, rebased, written)): Option<(i64, i64, i64)> = db
        .prepare_cached("SELECT id, rebased, written FROM streams WHERE scope = ?1 AND name = ?2")?
        .query_row(params![scope_id, name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?
    else {
        return Ok(None);
    };
    Ok(Some((stream, holding, place(tag(rebased), written as u64))))
}

/// The era as a place carries it: 23 bits, the top bit of a place being
/// the mark a node puts on places read from another machine (AU07).
fn tag(era: i64) -> u64 {
    (era as u64) & 0x7F_FFFF
}

/// What a reader at `at` knows an entry written here as `local` (its write
/// number, `Entry::local`) as: that number in the era it read in, so an
/// entry is told again as it is written again, and after the stream was
/// made anew (JJ04).
pub fn view_sequence(at: u64, local: u64) -> u64 {
    place(split(at).0, local)
}

fn place(tag: u64, sequence: u64) -> u64 {
    (tag << SEQUENCE_BITS) | (sequence & ((1 << SEQUENCE_BITS) - 1))
}

fn split(at: u64) -> (u64, u64) {
    (at >> SEQUENCE_BITS, at & ((1 << SEQUENCE_BITS) - 1))
}

fn entry(row: &Row<'_>) -> fastctx_storage::rusqlite::Result<(Entry, Vec<u8>, i64)> {
    Ok((
        Entry {
            item: row.get(0)?,
            order: row.get(1)?,
            revision: row.get::<_, i64>(2)? as u64,
            kind: row.get(3)?,
            schema: row.get(4)?,
            body: row.get::<_, Option<Vec<u8>>>(5)?.unwrap_or_default(),
            objects: Vec::new(),
            local: row.get::<_, i64>(8)? as u64,
        },
        row.get::<_, Option<Vec<u8>>>(6)?.unwrap_or_default(),
        row.get(7)?,
    ))
}

const COLUMNS: &str = "item, place, revision, kind, schema, body, objects, state, local";

/// The library's next era.
fn next_era(db: &Connection) -> fastctx_storage::Result<i64> {
    let latest = db
        .prepare_cached("SELECT value FROM stamps WHERE name = ?1")?
        .query_row([ERA], |row| row.get::<_, Vec<u8>>(0))
        .optional()?
        .and_then(|value| <[u8; 8]>::try_from(value.as_slice()).ok().map(i64::from_le_bytes))
        .unwrap_or(0);
    let era = latest + 1;
    db.prepare_cached("INSERT INTO stamps (name, value) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET value = excluded.value")?
        .execute(params![ERA, era.to_le_bytes().to_vec()])?;
    Ok(era)
}

/// Notes that `stream` was made, or its index made otherwise than by going
/// on from where readers stood: they read it anew. In the transaction that
/// did.
pub(super) fn rebased(db: &Connection, stream: i64) -> fastctx_storage::Result<()> {
    let era = next_era(db)?;
    db.prepare_cached(&format!("UPDATE streams SET rebased = ?2, written = {WRITTEN_NOW} WHERE id = ?1"))?.execute(params![stream, era])?;
    Ok(())
}

/// Notes that `scope` is held anew: readers of each of its streams read it
/// anew. In the transaction that did.
pub(super) fn held_anew(db: &Connection, scope: i64) -> fastctx_storage::Result<()> {
    let era = next_era(db)?;
    db.prepare_cached(&format!("UPDATE streams SET rebased = ?2, written = {WRITTEN_NOW} WHERE scope = ?1"))?.execute(params![scope, era])?;
    Ok(())
}

/// Where a stream stands in its era made anew: the last write number of a
/// row of it in its scope's holding period now, naught where none is — a
/// stream held anew, or cut back to nothing, holds nothing yet.
const WRITTEN_NOW: &str = "COALESCE((SELECT MAX(i.local) FROM items i WHERE i.stream = streams.id
    AND i.holding = (SELECT holding FROM scopes WHERE id = streams.scope)), 0)";

impl Library {
    /// Where a reader of `stream` of `scope` stands now; none when it is not
    /// held here.
    pub fn view_revision(&self, scope: &str, stream: &str) -> Result<Option<u64>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| Ok(standing(db, &scope, &stream)?.map(|(_, _, at)| at)))?)
    }

    /// At most `max_items` live items of `stream` of `scope` of at most
    /// `max_bytes` of bodies together, one at least, from `anchor`, read as of
    /// one moment. Call on a thread that may block.
    pub fn view_page(&self, scope: &str, stream: &str, anchor: &Anchor, max_items: u32, max_bytes: u32) -> Result<ViewPage> {
        let (scope, stream, anchor) = (scope.to_string(), stream.to_string(), anchor.clone());
        let (max_items, max_bytes) = (max_items.max(1) as usize, max_bytes as usize);
        Ok(self.database().read_now(move |db| {
            let snapshot = db.unchecked_transaction()?;
            let Some((id, holding, at)) = standing(&snapshot, &scope, &stream)? else { return Ok(ViewPage::default()) };
            let sql = match anchor {
                Anchor::Latest => format!(
                    "SELECT {COLUMNS} FROM items WHERE stream = ?1 AND holding = ?2 AND state = ?3 ORDER BY place DESC, item DESC LIMIT ?4"
                ),
                Anchor::Before(_) => format!(
                    "SELECT {COLUMNS} FROM items WHERE stream = ?1 AND holding = ?2 AND state = ?3 AND (place, item) < (?5, ?6)
                     ORDER BY place DESC, item DESC LIMIT ?4"
                ),
                Anchor::After(_) => format!(
                    "SELECT {COLUMNS} FROM items WHERE stream = ?1 AND holding = ?2 AND state = ?3 AND (place, item) > (?5, ?6)
                     ORDER BY place ASC, item ASC LIMIT ?4"
                ),
            };
            let limit = max_items as i64 + 1;
            let (mut entries, mut more) = (Vec::new(), false);
            {
                let mut statement = snapshot.prepare_cached(&sql)?;
                let mut rows = match &anchor {
                    Anchor::Latest => statement.query(params![id, holding, IS, limit])?,
                    Anchor::Before(place) | Anchor::After(place) => {
                        statement.query(params![id, holding, IS, limit, place.order, place.item])?
                    }
                };
                let mut bytes = 0usize;
                while let Some(row) = rows.next()? {
                    let (mut found, objects, _) = entry(row)?;
                    if entries.len() == max_items || (!entries.is_empty() && bytes + found.body.len() > max_bytes) {
                        more = true;
                        break;
                    }
                    bytes += found.body.len();
                    found.objects = unpack(&objects).map_err(invalid)?;
                    entries.push(found);
                }
            }
            if !matches!(anchor, Anchor::After(_)) {
                entries.reverse();
            }
            let place_of = |entry: &Entry| Place { order: entry.order.clone(), item: entry.item.clone() };
            let (first, last) = (entries.first().map(place_of), entries.last().map(place_of));
            let (older, newer) = match anchor {
                Anchor::Latest => (first.filter(|_| more), None),
                Anchor::Before(_) => (first.filter(|_| more), last),
                Anchor::After(_) => (first, last.filter(|_| more)),
            };
            Ok(ViewPage { revision: Some(at), entries, older, newer, through: at })
        })?)
    }

    /// What the index of `stream` of `scope` changed to after `from`, a
    /// reader's place, at most `max_items` of at most `max_bytes` of bodies,
    /// one at least, in the order they were written here: none when the
    /// reader is to read anew — another holding period, the index made
    /// otherwise since, or a place the stream never reached. Call on a
    /// thread that may block.
    pub fn view_changes(&self, scope: &str, stream: &str, from: u64, max_items: u32, max_bytes: u32) -> Result<Option<ViewChanges>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        let (max_items, max_bytes) = (max_items.max(1) as usize, max_bytes as usize);
        Ok(self.database().read_now(move |db| {
            let snapshot = db.unchecked_transaction()?;
            let Some((id, holding, at)) = standing(&snapshot, &scope, &stream)? else { return Ok(None) };
            let ((tag, next), (asked, after)) = (split(at), split(from));
            if asked != tag || after > next {
                return Ok(None);
            }
            let mut read = ViewChanges { revision: Some(at), changes: Vec::new(), through: from, more: false };
            if after == next {
                return Ok(Some(read));
            }
            // The last row taken, its write number and revision.
            let mut last = None::<(u64, u64)>;
            {
                let mut statement = snapshot.prepare_cached(&format!(
                    "SELECT {COLUMNS} FROM items WHERE stream = ?1 AND holding = ?2 AND local > ?3 ORDER BY local"
                ))?;
                let mut rows = statement.query(params![id, holding, after as i64])?;
                let mut bytes = 0usize;
                while let Some(row) = rows.next()? {
                    let (mut found, objects, state) = entry(row)?;
                    // A batch ends between records, never within one
                    // revision: a frame's records are written together. A
                    // window's floors share theirs, so they end a batch, and
                    // none goes far past its bounds.
                    let full = read.changes.len() >= max_items || (!read.changes.is_empty() && bytes + found.body.len() > max_bytes);
                    let splits = last.is_none_or(|(_, revision)| revision != found.revision)
                        || state == super::items::FLOOR
                        || read.changes.len() >= max_items.saturating_mul(4);
                    if full && splits {
                        read.more = true;
                        break;
                    }
                    bytes += found.body.len();
                    last = Some((found.local, found.revision));
                    if state == IS {
                        found.objects = unpack(&objects).map_err(invalid)?;
                        read.changes.push(ViewChange::Is(found));
                    } else {
                        read.changes.push(ViewChange::Gone { item: found.item });
                    }
                }
            }
            read.through = match (read.more, last) {
                (true, Some((local, _))) => place(tag, local),
                _ => at,
            };
            Ok(Some(read))
        })?)
    }

    /// Whether what `scope` holds in its holding period refers to the
    /// object `hash`: a reader of the scope may read it.
    pub fn refers(&self, scope: &str, hash: &[u8; 32]) -> Result<bool> {
        let (scope, hash) = (scope.to_string(), *hash);
        Ok(self.database().read_now(move |db| {
            Ok(db
                .prepare_cached(
                    "SELECT 1 FROM object_refs r JOIN scopes s ON s.id = r.scope
                     WHERE s.key = ?1 AND r.holding = s.holding AND r.hash = ?2 LIMIT 1",
                )?
                .query_row(params![scope, &hash[..]], |_| Ok(()))
                .optional()?
                .is_some())
        })?)
    }

    /// Waits until a reader of `stream` of `scope` would stand past
    /// `after`, `within` at most, and answers where it stands then; none
    /// while the stream is not held here.
    pub async fn view_moved(&self, scope: &str, stream: &str, after: u64, within: Duration) -> Result<Option<u64>> {
        // Watched before it is read, so a write between the two is not missed.
        let mut watching = self.watch_moves();
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let (key, name) = (scope.to_string(), stream.to_string());
            let at = self.database().read(move |db| Ok(standing(db, &key, &name)?.map(|(_, _, at)| at))).await?;
            if at.is_none_or(|at| at > after) {
                return Ok(at);
            }
            match tokio::time::timeout_at(deadline, watching.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => return Ok(at),
            }
        }
    }
}
