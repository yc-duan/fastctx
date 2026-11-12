//! Records judged where their frame stands (§2.3): a record kept out of the
//! index until its kind was known, or one a replica takes in a frame the hub
//! made canonical, is judged against its item as the frame's own earlier
//! records leave it, else as the frames before its place left it — never as
//! a later frame left it (U03). Those refused are kept out for good against
//! the frame they are in (U04).

use super::frame::{Header, KindRecord, open};
use super::items::{Applying, apply};
use super::{Authorizer, Entry};
use crate::error::invalid;
use fastctx_storage::rusqlite::{Connection, params};
use std::collections::BTreeMap;

/// Frames read at a time walking back for an item's earlier state.
const FRAMES_BACK: i64 = 16;

/// What becomes of one record of a frame being judged.
pub(super) enum Fate {
    /// Judged now: in the index if let in, refused for good if not.
    Judge,
    /// In the index since the frame came: earlier for those after it.
    Stood,
    /// Out of the index (of a kind still unknown, or refused before).
    Out,
}

/// Judges the records of the frame `header` heads, in their own order:
/// `fate` says what each is. How many went into the index.
pub(super) fn frame(
    db: &Connection,
    at: Applying,
    header: &Header,
    records: &[KindRecord],
    kinds: &dyn Authorizer,
    mut fate: impl FnMut(usize, &KindRecord) -> Fate,
) -> fastctx_storage::Result<usize> {
    let head = header.chain();
    let mut earlier: BTreeMap<&[u8], Option<Entry>> = BTreeMap::new();
    let mut applied = 0;
    for (offset, record) in records.iter().enumerate() {
        let revision = header.first + offset as u64;
        let item = record.record.item.as_slice();
        match fate(offset, record) {
            Fate::Out => {}
            Fate::Stood => {
                earlier.insert(item, super::hub::after(record, revision));
            }
            Fate::Judge => {
                let prior = match earlier.get(item) {
                    Some(prior) => prior.clone(),
                    None => before_at(db, at.stream, at.holding, item, revision)?,
                };
                if kinds.authorize(header, record, prior.as_ref()).is_err() {
                    refuse(db, at.stream, header, &head, offset)?;
                    continue;
                }
                super::hub::hidden_by_floor(db, at, revision, record)?;
                apply(db, at, revision, record)?;
                earlier.insert(item, super::hub::after(record, revision));
                applied += 1;
            }
        }
    }
    Ok(applied)
}

fn refuse(db: &Connection, stream: i64, header: &Header, head: &[u8; 32], offset: usize) -> fastctx_storage::Result<()> {
    db.prepare_cached("INSERT OR IGNORE INTO refused_records (stream, epoch, first, head, offset) VALUES (?1, ?2, ?3, ?4, ?5)")?
        .execute(params![stream, header.epoch as i64, header.first as i64, &head[..], offset as i64])?;
    Ok(())
}

/// The item as it stood just before `revision` in this period: what the
/// index or the prefix state holds when that is earlier, else the latest
/// record of it in the period's frames before that place, those kept out
/// passed over; else the prefix state of an earlier period, if earlier.
pub(super) fn before_at(db: &Connection, stream: i64, holding: i64, item: &[u8], revision: u64) -> fastctx_storage::Result<Option<Entry>> {
    if let Some(state) = super::hub::before_below(db, stream, holding, item, revision)? {
        return Ok(state);
    }
    let mut below = revision as i64;
    loop {
        let frames: Vec<(i64, Vec<u8>, Vec<u8>)> = {
            let mut statement = db.prepare_cached(
                "SELECT first, header, body FROM frames WHERE stream = ?1 AND holding = ?2 AND first < ?3 ORDER BY first DESC LIMIT ?4",
            )?;
            let rows = statement.query_map(params![stream, holding, below, FRAMES_BACK], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        if frames.is_empty() {
            break;
        }
        for (first, header, body) in frames {
            below = first;
            let header = Header::decode(&header).map_err(invalid)?;
            let head = header.chain();
            let kept = super::pending::kept(db, stream, header.epoch, header.first, &head)?;
            let refused = super::pending::refused(db, stream, header.epoch, header.first, &head)?;
            let records = open(&header, &body).map_err(invalid)?;
            for (offset, record) in records.iter().enumerate().rev() {
                let at = header.first + offset as u64;
                if at >= revision || record.record.item != item {
                    continue;
                }
                if kept.contains(&(record.kind, record.schema)) || refused.contains(&(offset as u64)) {
                    continue;
                }
                return Ok(super::hub::after(record, at));
            }
        }
    }
    // Not in this period's frames: what an earlier period's prefix gave it.
    Ok(super::hub::hidden_before(db, stream, holding, item, revision)?.filter(|entry| entry.revision < revision))
}

/// The refusal of the frame at `(epoch, first)` of `stream` whose head is
/// not `head` is of a frame no longer there (U04): it goes.
pub(super) fn forget_replaced(db: &Connection, stream: i64, epoch: u64, first: u64, head: &[u8; 32]) -> fastctx_storage::Result<()> {
    db.prepare_cached("DELETE FROM refused_records WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head <> ?4")?
        .execute(params![stream, epoch as i64, first as i64, &head[..]])?;
    db.prepare_cached("DELETE FROM pending_schema WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head <> ?4")?
        .execute(params![stream, epoch as i64, first as i64, &head[..]])?;
    Ok(())
}

/// The refusals and pending kinds of `stream` from `first` on, whose frames
/// were cut away: those frames are no more.
pub(super) fn forget_from(db: &Connection, stream: i64, first: i64) -> fastctx_storage::Result<()> {
    db.prepare_cached("DELETE FROM refused_records WHERE stream = ?1 AND first >= ?2")?.execute(params![stream, first])?;
    db.prepare_cached("DELETE FROM pending_schema WHERE stream = ?1 AND first >= ?2")?.execute(params![stream, first])?;
    Ok(())
}
