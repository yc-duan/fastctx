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
use super::items::Applying;
use super::{Authorizer, Entry};
use crate::error::invalid;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Frames read at a time building an item history.
const FRAMES_AT_ONCE: i64 = 64;

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

/// Which items have a record before this frame, of its holding period,
/// still out of the index — another period's never holds it back (X01).
pub(super) enum Blocked<'a> {
    /// As the library keeps them (`pending_items`): a frame that comes.
    Stored,
    /// As a judging pass over the frames in their order finds them: by
    /// stream, period and item.
    Pass(&'a mut HashSet<(i64, i64, Vec<u8>)>),
}

impl Blocked<'_> {
    fn holds(&self, db: &Connection, at: Applying, item: &[u8]) -> fastctx_storage::Result<bool> {
        Ok(match self {
            Blocked::Stored => db
                .prepare_cached("SELECT 1 FROM pending_items WHERE stream = ?1 AND holding = ?2 AND item = ?3")?
                .query_row(params![at.stream, at.holding, item], |_| Ok(()))
                .optional()?
                .is_some(),
            Blocked::Pass(blocked) => blocked.contains(&(at.stream, at.holding, item.to_vec())),
        })
    }

    fn add(&mut self, db: &Connection, at: Applying, item: &[u8]) -> fastctx_storage::Result<()> {
        match self {
            Blocked::Stored => {
                db.prepare_cached("INSERT OR IGNORE INTO pending_items (stream, holding, item) VALUES (?1, ?2, ?3)")?
                    .execute(params![at.stream, at.holding, item])?;
            }
            Blocked::Pass(blocked) => {
                blocked.insert((at.stream, at.holding, item.to_vec()));
            }
        }
        Ok(())
    }
}

/// Where each record a pass judges stood in one stream's frames of one
/// period: for each (item, revision) asked, the latest record of the item
/// before that revision, those kept out passed over. Built once when first
/// wanted, a few frames at a time in their order, so a pass that looks back
/// for many records reads the frames once (V02); only the answers are kept,
/// one for each record judged, never the item's history, and only one
/// stream's at a time (X02, Y01). One asked that was not foreseen — its
/// waiting mark gone before it was judged, say — builds them again with it
/// (Y02).
#[derive(Default)]
pub(super) struct History {
    /// Records judged besides those waiting in the frames — of a frame that
    /// comes — by item and revision.
    also: HashMap<Vec<u8>, BTreeSet<u64>>,
    kept: Option<Kept>,
}

/// The answers for one stream and period.
struct Kept {
    at: (i64, i64),
    /// By item: each revision asked, in order, with the revision, frame and
    /// place of the latest record before it.
    answers: HashMap<Vec<u8>, Vec<(u64, Option<Place>)>>,
}

/// A record's revision, its frame's first revision and its place there.
type Place = (u64, u64, usize);

/// An item's revisions asked in order, the answers so far, and the latest
/// record read so far.
type Sweep = (Vec<u64>, Vec<Option<Place>>, Option<Place>);

impl History {
    /// For judging the records `asked`, by item and revision, besides those
    /// waiting.
    pub(super) fn of(asked: impl IntoIterator<Item = (Vec<u8>, u64)>) -> Self {
        let mut also: HashMap<Vec<u8>, BTreeSet<u64>> = HashMap::new();
        for (item, revision) in asked {
            also.entry(item).or_default().insert(revision);
        }
        Self { also, kept: None }
    }

    /// The latest record of `item` before `revision` in the frames of
    /// `stream` of the period `holding`, those kept out passed over.
    fn before(&mut self, db: &Connection, stream: i64, holding: i64, item: &[u8], revision: u64) -> fastctx_storage::Result<Option<Place>> {
        let known = self.kept.as_ref().filter(|kept| kept.at == (stream, holding)).and_then(|kept| {
            let answers = kept.answers.get(item)?;
            let at = answers.binary_search_by_key(&revision, |(asked, _)| *asked).ok()?;
            Some(answers[at].1)
        });
        if let Some(answer) = known {
            return Ok(answer);
        }
        // Built again with it, and with all asked before of this stream.
        let mut asked: HashMap<Vec<u8>, BTreeSet<u64>> = self.also.clone();
        if let Some(kept) = self.kept.take().filter(|kept| kept.at == (stream, holding)) {
            for (item, answers) in kept.answers {
                asked.entry(item).or_default().extend(answers.into_iter().map(|(revision, _)| revision));
            }
        }
        asked.entry(item.to_vec()).or_default().insert(revision);
        waiting_records(db, stream, holding, |record, revision| {
            asked.entry(record.record.item.clone()).or_default().insert(revision);
        })?;
        let kept = build(db, stream, holding, asked)?;
        let answer = kept
            .answers
            .get(item)
            .and_then(|answers| answers.binary_search_by_key(&revision, |(asked, _)| *asked).ok().map(|at| answers[at].1))
            .flatten();
        self.kept = Some(kept);
        Ok(answer)
    }

    /// A record let in now, for what is judged after it.
    fn let_in(&mut self, at: Applying, item: &[u8], revision: u64, first: u64, offset: usize) {
        if let Some(kept) = self.kept.as_mut()
            && kept.at == (at.stream, at.holding)
            && let Some(answers) = kept.answers.get_mut(item)
        {
            for (asked, latest) in answers.iter_mut() {
                if *asked > revision && latest.is_none_or(|(placed, _, _)| placed < revision) {
                    *latest = Some((revision, first, offset));
                }
            }
        }
    }
}

/// The answers for `asked` in the frames of `stream` of the period
/// `holding`: read in their order, each record kept in answers the
/// revisions of its item asked past it, until a later one does.
fn build(db: &Connection, stream: i64, holding: i64, asked: HashMap<Vec<u8>, BTreeSet<u64>>) -> fastctx_storage::Result<Kept> {
    let mut sweep: HashMap<Vec<u8>, Sweep> =
        asked.into_iter().map(|(item, revisions)| (item, (revisions.into_iter().collect(), Vec::new(), None))).collect();
    // A few frames at a time: never the stream's bodies all at once (W02).
    let mut after = -1i64;
    loop {
        let frames: Vec<(i64, Vec<u8>, Vec<u8>)> = {
            let mut statement = db.prepare_cached(
                "SELECT first, header, body FROM frames WHERE stream = ?1 AND holding = ?2 AND first > ?3 ORDER BY first LIMIT ?4",
            )?;
            let rows = statement.query_map(params![stream, holding, after, FRAMES_AT_ONCE], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        if frames.is_empty() {
            break;
        }
        for (first, header, body) in frames {
            after = first;
            let header = Header::decode(&header).map_err(invalid)?;
            let out = Out::of(db, stream, &header)?;
            for (offset, record) in open(&header, &body).map_err(invalid)?.iter().enumerate() {
                if out.holds(offset, record) {
                    continue;
                }
                let Some((revisions, answered, latest)) = sweep.get_mut(&record.record.item) else { continue };
                let placed = header.first + offset as u64;
                while answered.len() < revisions.len() && revisions[answered.len()] <= placed {
                    answered.push(*latest);
                }
                *latest = Some((placed, header.first, offset));
            }
        }
    }
    let answers = sweep
        .into_iter()
        .map(|(item, (revisions, mut answered, latest))| {
            answered.resize(revisions.len(), latest);
            (item, revisions.into_iter().zip(answered).collect())
        })
        .collect();
    Ok(Kept { at: (stream, holding), answers })
}

/// Visits each record out of the index, not refused — of a kind kept out
/// or waiting behind one — in the frames of `stream` of the period
/// `holding`.
fn waiting_records(db: &Connection, stream: i64, holding: i64, mut visit: impl FnMut(&KindRecord, u64)) -> fastctx_storage::Result<()> {
    let frames: Vec<(i64, i64, Vec<u8>)> = {
        let mut statement = db.prepare_cached(
            "SELECT DISTINCT epoch, first, head FROM (
                 SELECT epoch, first, head FROM pending_schema WHERE stream = ?1
                 UNION SELECT epoch, first, head FROM deferred_records WHERE stream = ?1)",
        )?;
        let rows = statement.query_map([stream], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for (epoch, first, head) in frames {
        let found: Option<(Vec<u8>, Vec<u8>)> = db
            .prepare_cached("SELECT header, body FROM frames WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
            .query_row(params![stream, epoch, first, holding], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let Some((header, body)) = found else { continue };
        let header = Header::decode(&header).map_err(invalid)?;
        if header.chain()[..] != head[..] {
            continue;
        }
        let out = Out::of(db, stream, &header)?;
        let refused = super::pending::refused(db, stream, header.epoch, header.first, &header.chain())?;
        for (offset, record) in open(&header, &body).map_err(invalid)?.iter().enumerate() {
            if out.holds(offset, record) && !refused.contains(&(offset as u64)) {
                visit(record, header.first + offset as u64);
            }
        }
    }
    Ok(())
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

/// What a frame's earlier record left its item as, for those after it in
/// the frame: the record itself where it was let in — made into the item's
/// state only if another record of the item asks (§18 E1: most items come
/// once in a frame, and copying each one's body was most of what keeping
/// them cost) — or the state found where it was refused.
enum Earlier {
    At(usize),
    Found(Option<Entry>),
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
    let mut earlier: HashMap<&[u8], Earlier> = HashMap::with_capacity(records.len());
    let mut waiting: HashSet<&[u8]> = HashSet::new();
    let mut applied = 0;
    // Looked up once for the frame, not for each record (§28.43, E1): which
    // of its records waited already, and whether anything of its stream and
    // period waits at all as the library keeps it — an item that comes to
    // wait during the frame is in `waiting` from then on.
    let deferred_here: HashSet<u64> = deferred(db, at.stream, header.epoch, header.first, &head)?.into_iter().collect();
    let nothing_waits = matches!(blocked, Blocked::Stored)
        && db
            .prepare_cached("SELECT 1 FROM pending_items WHERE stream = ?1 AND holding = ?2 LIMIT 1")?
            .query_row(params![at.stream, at.holding], |_| Ok(()))
            .optional()?
            .is_none();
    // The rows of the frame's items, looked up at once (§18 E1): what each
    // item's first record judged here is judged against and written over.
    // Only that record's own judging changes its item's row, and those
    // after it go by `earlier`, so the row found now is the row then; an
    // item asked and not found has none.
    let mut rows = {
        let mut items: Vec<&[u8]> = records.iter().map(|record| record.record.item.as_slice()).collect();
        items.sort_unstable();
        items.dedup();
        super::items::indexed_many(db, at.stream, &items)?
    };
    let mut numbers = super::items::Numbers::new(at.stream, records.len());
    let mut inserts = super::items::Inserts::default();
    // Whether anything of the stream and period is kept aside for its
    // prefix: a record kept aside during the frame is of an item `earlier`
    // has from then on, so what the frame found stands for the others.
    let aside = db
        .prepare_cached("SELECT 1 FROM prefix_items WHERE stream = ?1 AND holding = ?2 LIMIT 1")?
        .query_row(params![at.stream, at.holding], |_| Ok(()))
        .optional()?
        .is_some();
    for (offset, record) in records.iter().enumerate() {
        let revision = header.first + offset as u64;
        let item = record.record.item.as_slice();
        match fate(offset, record) {
            Fate::Out => {}
            Fate::Unknown => {
                waiting.insert(item);
                blocked.add(db, at, item)?;
            }
            Fate::Stood => {
                earlier.insert(item, Earlier::At(offset));
            }
            Fate::Judge => {
                // Behind an earlier record of its item still out, it waits.
                if waiting.contains(item) || (!nothing_waits && blocked.holds(db, at, item)?) {
                    defer(db, at.stream, header, &head, offset)?;
                    waiting.insert(item);
                    blocked.add(db, at, item)?;
                    continue;
                }
                // Its waiting mark goes once it is judged, not before: what
                // it is judged against is found with it (Y02).
                // An item's first record here: its row looked up once, for
                // judging it and for writing it (§18 E1).
                let (prior, indexed) = match earlier.get(item) {
                    Some(Earlier::At(before)) => (super::hub::after(&records[*before], header.first + *before as u64), None),
                    Some(Earlier::Found(prior)) => (prior.clone(), None),
                    None => {
                        let indexed = rows.remove(item);
                        (before_at(db, at.stream, at.holding, item, revision, history, indexed.as_ref(), aside)?, Some(indexed))
                    }
                };
                if kinds.authorize(header, record, prior.as_ref()).is_err() {
                    refuse(db, at.stream, header, &head, offset)?;
                    // Refused, it leaves its item as it was for those after.
                    earlier.insert(item, Earlier::Found(prior));
                    continue;
                }
                if deferred_here.contains(&(offset as u64)) {
                    forget_deferred(db, at.stream, header, &head, offset)?;
                }
                match indexed {
                    Some(indexed) => super::items::apply_judged_known(db, at, revision, record, &mut numbers, indexed, &mut inserts)?,
                    None => super::items::apply_judged(db, at, revision, record, &mut numbers, &mut inserts)?,
                };
                history.let_in(at, item, revision, header.first, offset);
                earlier.insert(item, Earlier::At(offset));
                applied += 1;
            }
        }
    }
    inserts.flush(db)?;
    numbers.finish(db)?;
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
#[allow(clippy::too_many_arguments)]
fn before_at(
    db: &Connection,
    stream: i64,
    holding: i64,
    item: &[u8],
    revision: u64,
    history: &mut History,
    indexed: Option<&super::items::Indexed>,
    aside: bool,
) -> fastctx_storage::Result<Option<Entry>> {
    if let Some(state) = super::hub::before_below_known(db, stream, holding, item, revision, indexed, aside)? {
        return Ok(state);
    }
    if let Some((placed, first, offset)) = history.before(db, stream, holding, item, revision)? {
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

/// The items of `stream` with a record still out of the index, found again
/// from the frames of its scope's current holding period that keep any
/// (W01): once frames were cut away, put in another's place or purged,
/// nothing waits behind a record no longer there, nor behind one of a
/// period that ended (X01).
pub(super) fn rebuild_waiting(db: &Connection, stream: i64) -> fastctx_storage::Result<()> {
    db.prepare_cached("DELETE FROM pending_items WHERE stream = ?1")?.execute([stream])?;
    let holding: Option<i64> = db
        .prepare_cached("SELECT c.holding FROM streams s JOIN scopes c ON c.id = s.scope WHERE s.id = ?1")?
        .query_row([stream], |row| row.get::<_, Option<i64>>(0))
        .optional()?
        .flatten();
    let Some(holding) = holding else { return Ok(()) };
    let mut items: HashSet<Vec<u8>> = HashSet::new();
    waiting_records(db, stream, holding, |record, _| {
        items.insert(record.record.item.clone());
    })?;
    let mut keep = db.prepare_cached("INSERT OR IGNORE INTO pending_items (stream, holding, item) VALUES (?1, ?2, ?3)")?;
    for item in items {
        keep.execute(params![stream, holding, item])?;
    }
    Ok(())
}

/// The refusals, waiting records and pending kinds of the frame at
/// `(epoch, first)` of `stream` whose head is not `head` are of a frame no
/// longer there (U04): they go.
pub(super) fn forget_replaced(db: &Connection, stream: i64, epoch: u64, first: u64, head: &[u8; 32]) -> fastctx_storage::Result<()> {
    let mut gone = 0;
    for table in ["refused_records", "pending_schema", "deferred_records"] {
        gone += db
            .prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND head <> ?4"))?
            .execute(params![stream, epoch as i64, first as i64, &head[..]])?;
    }
    if gone > 0 {
        rebuild_waiting(db, stream)?;
    }
    Ok(())
}

/// The refusals, waiting records and pending kinds of `stream` from `first`
/// on, whose frames were cut away: those frames are no more.
pub(super) fn forget_from(db: &Connection, stream: i64, first: i64) -> fastctx_storage::Result<()> {
    for table in ["refused_records", "pending_schema", "deferred_records"] {
        db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND first >= ?2"))?.execute(params![stream, first])?;
    }
    rebuild_waiting(db, stream)
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
