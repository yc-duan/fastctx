//! Records judged where their frame stands (§2.3): a record kept out of the
//! index until its kind was known, or one a replica takes in a frame the hub
//! made canonical, is judged against its item as the frame's own earlier
//! records leave it, else as the frames before its place left it — never as
//! a later frame left it (U03). Those refused are kept out for good against
//! the frame they are in (U04).
//!
//! A record whose item has an earlier record still out of the index — its
//! kind not known here yet — is not judged at all: what it would be judged
//! against is not known. It waits (`deferred_records`), and so does every
//! later record of its item, until the earlier one is known and all are
//! judged in their order (V01). So every holder, whenever it came to know
//! which kinds, judges an item's records against the same states.

use super::frame::{Header, KindRecord, open};
use super::items::{Applying, apply};
use super::{Authorizer, Entry};
use crate::error::invalid;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, HashMap, HashSet};

/// What becomes of one record of a frame being judged.
pub(super) enum Fate {
    /// Judged now: in the index if let in, refused for good if not; waiting
    /// instead behind an earlier record of its item still out.
    Judge,
    /// In the index since the frame came: earlier for those after it.
    Stood,
    /// Of a kind not known here: out of the index, and its item's later
    /// records wait behind it.
    Unknown,
    /// Out of the index for good (refused before).
    Out,
}

/// Which items have a record before this frame still out of the index.
pub(super) enum Blocked<'a> {
    /// As the library keeps them (`pending_items`): a frame that comes.
    Stored,
    /// As a judging pass over the frames in their order finds them.
    Pass(&'a mut HashSet<(i64, Vec<u8>)>),
}

impl Blocked<'_> {
    fn holds(&self, db: &Connection, stream: i64, item: &[u8]) -> fastctx_storage::Result<bool> {
        Ok(match self {
            Blocked::Stored => db
                .prepare_cached("SELECT 1 FROM pending_items WHERE stream = ?1 AND item = ?2")?
                .query_row(params![stream, item], |_| Ok(()))
                .optional()?
                .is_some(),
            Blocked::Pass(blocked) => blocked.contains(&(stream, item.to_vec())),
        })
    }

    fn add(&mut self, db: &Connection, stream: i64, item: &[u8]) -> fastctx_storage::Result<()> {
        match self {
            Blocked::Stored => {
                db.prepare_cached("INSERT OR IGNORE INTO pending_items (stream, item) VALUES (?1, ?2)")?.execute(params![stream, item])?;
            }
            Blocked::Pass(blocked) => {
                blocked.insert((stream, item.to_vec()));
            }
        }
        Ok(())
    }
}

/// The places of each item's records in a stream's frames of this period,
/// those kept out passed over: built once when first wanted, so a judging
/// pass that looks back for many items reads the frames once (V02).
#[derive(Default)]
pub(super) struct History {
    streams: HashMap<i64, Places>,
}

/// Each item's records by revision, with the frame and place of each.
type Places = HashMap<Vec<u8>, Vec<(u64, u64, usize)>>;

impl History {
    fn places(&mut self, db: &Connection, stream: i64, holding: i64) -> fastctx_storage::Result<&mut Places> {
        if let std::collections::hash_map::Entry::Vacant(vacant) = self.streams.entry(stream) {
            let mut places: Places = HashMap::new();
            let frames: Vec<(Vec<u8>, Vec<u8>)> = {
                let mut statement = db.prepare_cached("SELECT header, body FROM frames WHERE stream = ?1 AND holding = ?2 ORDER BY first")?;
                let rows = statement.query_map(params![stream, holding], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            for (header, body) in frames {
                let header = Header::decode(&header).map_err(invalid)?;
                let out = Out::of(db, stream, &header)?;
                for (offset, record) in open(&header, &body).map_err(invalid)?.iter().enumerate() {
                    if out.holds(offset, record) {
                        continue;
                    }
                    places.entry(record.record.item.clone()).or_default().push((header.first + offset as u64, header.first, offset));
                }
            }
            vacant.insert(places);
        }
        Ok(self.streams.get_mut(&stream).expect("just built"))
    }

    /// A record let in now, for what is judged after it.
    fn let_in(&mut self, stream: i64, item: &[u8], revision: u64, first: u64, offset: usize) {
        if let Some(places) = self.streams.get_mut(&stream) {
            let list = places.entry(item.to_vec()).or_default();
            let at = list.partition_point(|(placed, _, _)| *placed < revision);
            list.insert(at, (revision, first, offset));
        }
    }
}

/// What of a frame is out of the index: its kinds not known when it came,
/// its records refused, its records waiting.
struct Out {
    kinds: Vec<(u16, u16)>,
    offsets: HashSet<u64>,
}

impl Out {
    fn of(db: &Connection, stream: i64, header: &Header) -> fastctx_storage::Result<Self> {
        let head = header.chain();
        let kinds = super::pending::kept(db, stream, header.epoch, header.first, &head)?;
        let mut offsets: HashSet<u64> = super::pending::refused(db, stream, header.epoch, header.first, &head)?.into_iter().collect();
        offsets.extend(deferred(db, stream, header.epoch, header.first, &head)?);
        Ok(Self { kinds, offsets })
    }

    fn holds(&self, offset: usize, record: &KindRecord) -> bool {
        self.offsets.contains(&(offset as u64)) || self.kinds.contains(&(record.kind, record.schema))
    }
}

/// Judges the records of the frame `header` heads, in their own order:
/// `fate` says what each is, `blocked` which items wait from before it. How
/// many went into the index.
#[allow(clippy::too_many_arguments)]
pub(super) fn frame(
    db: &Connection,
    at: Applying,
    header: &Header,
    records: &[KindRecord],
    kinds: &dyn Authorizer,
    history: &mut History,
    blocked: &mut Blocked<'_>,
    mut fate: impl FnMut(usize, &KindRecord) -> Fate,
) -> fastctx_storage::Result<usize> {
    let head = header.chain();
    let mut earlier: BTreeMap<&[u8], Option<Entry>> = BTreeMap::new();
    let mut waiting: HashSet<&[u8]> = HashSet::new();
    let mut applied = 0;
    for (offset, record) in records.iter().enumerate() {
        let revision = header.first + offset as u64;
        let item = record.record.item.as_slice();
        match fate(offset, record) {
            Fate::Out => {}
            Fate::Unknown => {
                waiting.insert(item);
                blocked.add(db, at.stream, item)?;
            }
            Fate::Stood => {
                earlier.insert(item, super::hub::after(record, revision));
            }
            Fate::Judge => {
                // Behind an earlier record of its item still out, it waits.
                if waiting.contains(item) || blocked.holds(db, at.stream, item)? {
                    defer(db, at.stream, header, &head, offset)?;
                    waiting.insert(item);
                    blocked.add(db, at.stream, item)?;
                    continue;
                }
                forget_deferred(db, at.stream, header, &head, offset)?;
                let prior = match earlier.get(item) {
                    Some(prior) => prior.clone(),
                    None => before_at(db, at.stream, at.holding, item, revision, history)?,
                };
                if kinds.authorize(header, record, prior.as_ref()).is_err() {
                    refuse(db, at.stream, header, &head, offset)?;
                    continue;
                }
                super::hub::hidden_by_floor(db, at, revision, record)?;
                apply(db, at, revision, record)?;
                history.let_in(at.stream, item, revision, header.first, offset);
                earlier.insert(item, super::hub::after(record, revision));
                applied += 1;
            }
        }
    }
    Ok(applied)
}

fn refuse(db: &Connection, stream: i64, header: &Header, head: &[u8; 32], offset: usize) -> fastctx_storage::Result<()> {
    forget_deferred(db, stream, header, head, offset)?;
    db.prepare_cached("INSERT OR IGNORE INTO refused_records (stream, epoch, first, head, offset) VALUES (?1, ?2, ?3, ?4, ?5)")?
        .execute(params![stream, header.epoch as i64, header.first as i64, &head[..], offset as i64])?;
    Ok(())
}

fn defer(db: &Connection, stream: i64, header: &Header, head: &[u8; 32], offset: usize) -> fastctx_storage::Result<()> {
    db.prepare_cached("INSERT OR IGNORE INTO deferred_records (stream, epoch, first, head, offset) VALUES (?1, ?2, ?3, ?4, ?5)")?
        .execute(params![stream, header.epoch as i64, header.first as i64, &head[..], offset as i64])?;
    Ok(())
}

fn forget_deferred(db: &Connection, stream: i64, header: &Header, head: &[u8; 32], offset: usize) -> fastctx_storage::Result<()> {
    db.prepare_cached("DELETE FROM deferred_records WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head = ?4 AND offset = ?5")?
        .execute(params![stream, header.epoch as i64, header.first as i64, &head[..], offset as i64])?;
    Ok(())
}

/// The records of the frame of `stream` at (`epoch`, `first`) whose head is
/// `head` waiting behind an earlier record of their item, by their place.
pub(super) fn deferred(db: &Connection, stream: i64, epoch: u64, first: u64, head: &[u8; 32]) -> fastctx_storage::Result<Vec<u64>> {
    let mut statement =
        db.prepare_cached("SELECT offset FROM deferred_records WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head = ?4")?;
    let rows = statement.query_map(params![stream, epoch as i64, first as i64, &head[..]], |row| row.get::<_, i64>(0))?;
    Ok(rows.map(|row| row.map(|offset| offset as u64)).collect::<std::result::Result<_, _>>()?)
}

/// The item as it stood just before `revision` in this period: what the
/// index or the prefix state holds when that is earlier, else the latest
/// record of it in the period's frames before that place, those kept out
/// passed over (`history`); else the prefix state of an earlier period, if
/// earlier.
fn before_at(db: &Connection, stream: i64, holding: i64, item: &[u8], revision: u64, history: &mut History) -> fastctx_storage::Result<Option<Entry>> {
    if let Some(state) = super::hub::before_below(db, stream, holding, item, revision)? {
        return Ok(state);
    }
    let found = history
        .places(db, stream, holding)?
        .get(item)
        .and_then(|places| places.iter().rev().find(|(placed, _, _)| *placed < revision).copied());
    if let Some((placed, first, offset)) = found {
        let frame: (Vec<u8>, Vec<u8>) = db
            .prepare_cached("SELECT header, body FROM frames WHERE stream = ?1 AND holding = ?2 AND first = ?3")?
            .query_row(params![stream, holding, first as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let header = Header::decode(&frame.0).map_err(invalid)?;
        let records = open(&header, &frame.1).map_err(invalid)?;
        let record = records.get(offset).ok_or_else(|| invalid("a record's place is past its frame"))?;
        return Ok(super::hub::after(record, placed));
    }
    // Not in this period's frames: what an earlier period's prefix gave it.
    Ok(super::hub::hidden_before(db, stream, holding, item, revision)?.filter(|entry| entry.revision < revision))
}

/// The refusals, waiting records and pending kinds of the frame at
/// `(epoch, first)` of `stream` whose head is not `head` are of a frame no
/// longer there (U04): they go.
pub(super) fn forget_replaced(db: &Connection, stream: i64, epoch: u64, first: u64, head: &[u8; 32]) -> fastctx_storage::Result<()> {
    for table in ["refused_records", "pending_schema", "deferred_records"] {
        db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head <> ?4"))?
            .execute(params![stream, epoch as i64, first as i64, &head[..]])?;
    }
    Ok(())
}

/// The refusals, waiting records and pending kinds of `stream` from `first`
/// on, whose frames were cut away: those frames are no more.
pub(super) fn forget_from(db: &Connection, stream: i64, first: i64) -> fastctx_storage::Result<()> {
    for table in ["refused_records", "pending_schema", "deferred_records"] {
        db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND first >= ?2"))?.execute(params![stream, first])?;
    }
    Ok(())
}

/// What is kept of the frame of `stream` at `(epoch, first)` whose head is
/// `head`, a frame no longer in its scope's current period: it goes.
pub(super) fn forget_from_frame(db: &Connection, stream: i64, epoch: i64, first: i64, head: &[u8; 32]) -> fastctx_storage::Result<()> {
    for table in ["pending_schema", "deferred_records"] {
        db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head = ?4"))?
            .execute(params![stream, epoch, first, &head[..]])?;
    }
    Ok(())
}
