//! The one way into `items` (§2.4): every path that changes the index —
//! a producer's commit, an imported frame, a re-application after an
//! upgrade, a snapshot, a page, a rebuild — goes through `apply`.
//!
//! A record's revision is its stream sequence number. A row of the current
//! holding period is replaced only by a strictly greater revision; a row of
//! an ended period, or a revision floor its purge left, by a revision at
//! least as great, which moves it into the current period. Lower revisions
//! leave it: a new period refilling from the stream's start does not bring
//! back an item a later revision deleted. A deletion stays as a tombstone.
//!
//! A row from a window or an older page goes by when the hub answered it
//! instead (`apply_answered`, §29 3.3, PH01): it is written only where that
//! moment is past the row here, so a page answered earlier never undoes
//! what a later answer or a frame made; and what an answer shows absent is
//! a floor at that moment (`snapshot.rs`).
//!
//! Every row written takes the library's next write number (`writes`), and
//! its stream notes it: readers on this device read by those, whatever
//! order the rows came in (PH04).

use super::frame::KindRecord;
use crate::record::{State, pack, unpack};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

pub(super) const IS: i64 = 0;
pub(super) const GONE: i64 = 1;
pub(super) const FLOOR: i64 = 2;

/// Where records are applied: their scope, stream and the scope's current
/// holding period.
#[derive(Clone, Copy, Debug)]
pub(super) struct Applying {
    pub scope: i64,
    pub stream: i64,
    pub holding: i64,
}

/// An index row as it stands: its order key, revision, state, packed
/// object references and holding period.
struct Row {
    place: Vec<u8>,
    revision: i64,
    state: i64,
    objects: Option<Vec<u8>>,
    holding: i64,
}

/// The library's next write number, in the transaction `db` is in.
///
/// The single-row writes each record makes say `OR FAIL`: a constraint of
/// one row is checked before anything of it is written, so failing leaves
/// what aborting would, and SQLite then keeps no statement journal for
/// them, where it would copy each page they touch once more for every
/// record of a frame (§28.43: a replica's catch-up took 17% longer).
pub(super) fn next_write(db: &Connection) -> fastctx_storage::Result<i64> {
    if let Some(last) = db
        .prepare_cached("UPDATE OR FAIL writes SET last = last + 1 WHERE id = 1 RETURNING last")?
        .query_row([], |row| row.get(0))
        .optional()?
    {
        return Ok(last);
    }
    Ok(db
        .prepare_cached("INSERT INTO writes (id, last) VALUES (1, 1) RETURNING last")?
        .query_row([], |row| row.get(0))?)
}

/// The library's last write number.
pub(super) fn last_write(db: &Connection) -> fastctx_storage::Result<i64> {
    Ok(db.prepare_cached("SELECT last FROM writes WHERE id = 1")?.query_row([], |row| row.get(0)).optional()?.unwrap_or(0))
}

/// A write number for a row of `stream`, which its stream notes.
pub(super) fn written(db: &Connection, stream: i64) -> fastctx_storage::Result<i64> {
    let local = next_write(db)?;
    db.prepare_cached("UPDATE OR FAIL streams SET written = ?2 WHERE id = ?1")?.execute(params![stream, local])?;
    Ok(local)
}

/// The write numbers of the rows one frame writes into `stream`, taken a
/// block at a time and the stream noted once (§28.43, E1): the very numbers,
/// in the very order, taking them one at a time would give; what is left of
/// the last block goes back where nothing took a number after it.
pub(super) struct Numbers {
    stream: i64,
    next: i64,
    end: i64,
    last: Option<i64>,
    left: usize,
}

impl Numbers {
    /// For at most `rows` rows of `stream`.
    pub(super) fn new(stream: i64, rows: usize) -> Self {
        Self { stream, next: 1, end: 0, last: None, left: rows.max(1) }
    }

    fn take(&mut self, db: &Connection) -> fastctx_storage::Result<i64> {
        if self.next > self.end {
            let block = self.left.max(1) as i64;
            let last = match db
                .prepare_cached("UPDATE OR FAIL writes SET last = last + ?1 WHERE id = 1 RETURNING last")?
                .query_row([block], |row| row.get::<_, i64>(0))
                .optional()?
            {
                Some(last) => last,
                None => db
                    .prepare_cached("INSERT INTO writes (id, last) VALUES (1, ?1) RETURNING last")?
                    .query_row([block], |row| row.get(0))?,
            };
            self.next = last - block + 1;
            self.end = last;
        }
        let local = self.next;
        self.next += 1;
        self.left = self.left.saturating_sub(1);
        self.last = Some(local);
        Ok(local)
    }

    /// Notes the last number taken on the stream, and gives back what the
    /// block left where it is the library's last still.
    pub(super) fn finish(self, db: &Connection) -> fastctx_storage::Result<()> {
        if self.next <= self.end {
            let kept = self.next - 1;
            db.prepare_cached("UPDATE OR FAIL writes SET last = ?1 WHERE id = 1 AND last = ?2")?.execute(params![kept, self.end])?;
        }
        if let Some(last) = self.last {
            db.prepare_cached("UPDATE OR FAIL streams SET written = ?2 WHERE id = ?1")?.execute(params![self.stream, last])?;
        }
        Ok(())
    }
}

/// An item's row in the index, whatever its period: what judging a record
/// against its item and writing the record over it both need, looked up
/// once for both (§18 E1, E2).
pub(super) struct Indexed {
    pub(super) place: Vec<u8>,
    pub(super) revision: i64,
    pub(super) state: i64,
    pub(super) objects: Option<Vec<u8>>,
    pub(super) holding: i64,
    pub(super) kind: u16,
    pub(super) schema: u16,
    pub(super) body: Option<Vec<u8>>,
}

/// The row of `item` in `stream`'s index, whatever its period.
pub(super) fn indexed(db: &Connection, stream: i64, item: &[u8]) -> fastctx_storage::Result<Option<Indexed>> {
    Ok(db
        .prepare_cached("SELECT place, revision, state, objects, holding, kind, schema, body FROM items WHERE stream = ?1 AND item = ?2")?
        .query_row(params![stream, item], |row| {
            Ok(Indexed {
                place: row.get(0)?,
                revision: row.get(1)?,
                state: row.get(2)?,
                objects: row.get(3)?,
                holding: row.get(4)?,
                kind: row.get(5)?,
                schema: row.get(6)?,
                body: row.get(7)?,
            })
        })
        .optional()?)
}

/// How many items one look at the index asks after (`indexed_many`).
const LOOKED_AT_ONCE: usize = 64;

/// One look at `LOOKED_AT_ONCE` items' rows, through the index of
/// identities: an `IN` the planner is left to plan scans the stream instead
/// (§18 E1, codex-865aeb9b's probe: 15–22 μs a row against 0.16).
static INDEXED_MANY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let items = vec!["?"; LOOKED_AT_ONCE].join(", ");
    format!(
        "SELECT item, place, revision, state, objects, holding, kind, schema, body FROM items INDEXED BY items_by_identity
         WHERE stream = ? AND item IN ({items})"
    )
});

/// The rows of `items` in `stream`'s index, whatever their period, by item:
/// an item with no row is not in what comes back. A frame's items looked up
/// at once (`judged::frame`), not one statement each (§18 E1).
pub(super) fn indexed_many(
    db: &Connection,
    stream: i64,
    items: &[&[u8]],
) -> fastctx_storage::Result<std::collections::HashMap<Vec<u8>, Indexed>> {
    let mut found = std::collections::HashMap::with_capacity(items.len());
    let mut statement = db.prepare_cached(&INDEXED_MANY)?;
    for batch in items.chunks(LOOKED_AT_ONCE) {
        let mut values: Vec<&dyn fastctx_storage::rusqlite::ToSql> = Vec::with_capacity(LOOKED_AT_ONCE + 1);
        values.push(&stream);
        // A short batch asks after its last again: an item asked twice comes
        // back once.
        for at in 0..LOOKED_AT_ONCE {
            values.push(&batch[at.min(batch.len() - 1)]);
        }
        let mut rows = statement.query(values.as_slice())?;
        while let Some(row) = rows.next()? {
            found.insert(
                row.get::<_, Vec<u8>>(0)?,
                Indexed {
                    place: row.get(1)?,
                    revision: row.get(2)?,
                    state: row.get(3)?,
                    objects: row.get(4)?,
                    holding: row.get(5)?,
                    kind: row.get(6)?,
                    schema: row.get(7)?,
                    body: row.get(8)?,
                },
            );
        }
    }
    Ok(found)
}

fn existing(db: &Connection, at: Applying, item: &[u8]) -> fastctx_storage::Result<Option<Row>> {
    Ok(db
        .prepare_cached("SELECT place, revision, state, objects, holding FROM items WHERE stream = ?1 AND item = ?2")?
        .query_row(params![at.stream, item], |row| {
            Ok(Row { place: row.get(0)?, revision: row.get(1)?, state: row.get(2)?, objects: row.get(3)?, holding: row.get(4)? })
        })
        .optional()?)
}

/// Applies `record` at `revision`; whether it changed the index.
pub(super) fn apply(db: &Connection, at: Applying, revision: u64, record: &KindRecord) -> fastctx_storage::Result<bool> {
    let Some(existing) = replaced(db, at, revision, record)? else { return Ok(false) };
    replace(db, at, existing, revision, record)?;
    Ok(true)
}

/// As `apply`, its write number from the commit's block (`Numbers`).
pub(super) fn apply_numbered(
    db: &Connection,
    at: Applying,
    revision: u64,
    record: &KindRecord,
    numbers: &mut Numbers,
) -> fastctx_storage::Result<bool> {
    let Some(existing) = replaced(db, at, revision, record)? else { return Ok(false) };
    let local = numbers.take(db)?;
    replace_at(db, at, existing, revision, record, local)?;
    Ok(true)
}

/// The row `record` at `revision` replaces, as `apply` judges it: none
/// where it does not replace what is there.
#[allow(clippy::option_option)]
fn replaced(db: &Connection, at: Applying, revision: u64, record: &KindRecord) -> fastctx_storage::Result<Option<Option<Row>>> {
    let existing = existing(db, at, &record.record.item)?;
    if let Some(Row { revision: known, state, holding, .. }) = &existing {
        let current = *holding == at.holding && *state != FLOOR;
        let replaces = if current { revision as i64 > *known } else { revision as i64 >= *known };
        if !replaces {
            return Ok(None);
        }
    }
    Ok(Some(existing))
}

/// Applies `record`, of `revision`, as the hub answered it at `through` (the
/// sequence its stream was to take next then): only where that is past the
/// row here — a floor at most as late, a row of an earlier revision — so a
/// page answered before what is here undoes nothing; whether it changed the
/// index.
pub(super) fn apply_answered(
    db: &Connection,
    at: Applying,
    revision: u64,
    through: u64,
    record: &KindRecord,
) -> fastctx_storage::Result<bool> {
    let existing = existing(db, at, &record.record.item)?;
    if let Some(Row { revision: known, state, holding, .. }) = &existing {
        let (known, through, revision) = (*known, through as i64, revision as i64);
        let replaces = if *state == FLOOR {
            through >= known
        } else {
            // The same revision of the same period is the row here already.
            known < through && revision >= known && !(revision == known && *holding == at.holding)
        };
        if !replaces {
            return Ok(false);
        }
    }
    replace(db, at, existing, revision, record)?;
    Ok(true)
}

/// As `apply`, for a record judged in a frame (`judged::frame`): what an
/// ended period or a floor shows of its item later than it is kept aside
/// first (`hub::kept_aside`), from the same look at the row, and its write
/// number comes from the frame's block.
pub(super) fn apply_judged<'r>(
    db: &Connection,
    at: Applying,
    revision: u64,
    record: &'r KindRecord,
    numbers: &mut Numbers,
    inserts: &mut Inserts<'r>,
) -> fastctx_storage::Result<bool> {
    // Its row read as it stands, any of it lined up written first.
    if inserts.holds(&record.record.item) {
        inserts.flush(db)?;
    }
    let existing = existing(db, at, &record.record.item)?;
    apply_judged_over(db, at, revision, record, numbers, existing, inserts)
}

/// As `apply_judged`, over the row `indexed` of its item as it stands now,
/// looked up already.
pub(super) fn apply_judged_known<'r>(
    db: &Connection,
    at: Applying,
    revision: u64,
    record: &'r KindRecord,
    numbers: &mut Numbers,
    indexed: Option<Indexed>,
    inserts: &mut Inserts<'r>,
) -> fastctx_storage::Result<bool> {
    let existing = indexed.map(|Indexed { place, revision, state, objects, holding, .. }| Row { place, revision, state, objects, holding });
    apply_judged_over(db, at, revision, record, numbers, existing, inserts)
}

fn apply_judged_over<'r>(
    db: &Connection,
    at: Applying,
    revision: u64,
    record: &'r KindRecord,
    numbers: &mut Numbers,
    existing: Option<Row>,
    inserts: &mut Inserts<'r>,
) -> fastctx_storage::Result<bool> {
    if let Some(Row { revision: known, state, holding, .. }) = &existing {
        if (*holding != at.holding || *state == FLOOR) && *known > revision as i64 {
            super::hub::kept_aside(db, at, revision, record)?;
        }
        let current = *holding == at.holding && *state != FLOOR;
        let replaces = if current { revision as i64 > *known } else { revision as i64 >= *known };
        if !replaces {
            return Ok(false);
        }
    }
    let local = numbers.take(db)?;
    match existing {
        // A new item's row lined up, written a batch at a time (§18 E1).
        None => inserts.push(db, at, revision, record, local)?,
        existing => replace_at(db, at, existing, revision, record, local)?,
    }
    Ok(true)
}

/// How many rows of new items one statement writes (`Inserts`).
const INSERTED_AT_ONCE: usize = 32;

/// One `INSERT` of `INSERTED_AT_ONCE` rows of `items`.
static INSERT_MANY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let rows = vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"; INSERTED_AT_ONCE].join(", ");
    format!("INSERT OR FAIL INTO items (stream, place, item, revision, state, kind, schema, body, objects, holding, local) VALUES {rows}")
});

/// The rows of new items a frame writes (`judged::frame`), each as
/// `replace_at` writes one over no row — its objects referred to at once —
/// lined up and inserted `INSERTED_AT_ONCE` to a statement (§18 E1): an item
/// lined up is written before its row is read again (`holds`), and all are
/// written before the frame is done (`flush`). One that fails fails the
/// whole application, as one written alone does.
#[derive(Default)]
pub(super) struct Inserts<'r> {
    rows: Vec<NewRow<'r>>,
    items: std::collections::HashSet<&'r [u8]>,
}

struct NewRow<'r> {
    stream: i64,
    place: &'r [u8],
    item: &'r [u8],
    revision: i64,
    state: i64,
    kind: u16,
    schema: u16,
    body: Option<&'r [u8]>,
    objects: Option<Vec<u8>>,
    holding: i64,
    local: i64,
}

impl<'r> Inserts<'r> {
    fn push(&mut self, db: &Connection, at: Applying, revision: u64, record: &'r KindRecord, local: i64) -> fastctx_storage::Result<()> {
        let (state, body, objects) = match &record.record.state {
            State::Is { body, objects } => {
                for object in objects {
                    refer(db, at.scope, at.holding, at.stream, &object.hash)?;
                }
                (IS, Some(body.as_slice()), Some(pack(objects)))
            }
            State::Gone => (GONE, None, None),
        };
        let item = record.record.item.as_slice();
        self.items.insert(item);
        self.rows.push(NewRow {
            stream: at.stream,
            place: &record.record.order,
            item,
            revision: revision as i64,
            state,
            kind: record.kind,
            schema: record.schema,
            body,
            objects,
            holding: at.holding,
            local,
        });
        if self.rows.len() >= INSERTED_AT_ONCE {
            self.flush(db)?;
        }
        Ok(())
    }

    /// Whether `item`'s row is lined up, not written yet.
    pub(super) fn holds(&self, item: &[u8]) -> bool {
        self.items.contains(item)
    }

    /// Writes the rows lined up, in their order.
    pub(super) fn flush(&mut self, db: &Connection) -> fastctx_storage::Result<()> {
        let rows = std::mem::take(&mut self.rows);
        self.items.clear();
        let mut rows = rows.as_slice();
        while rows.len() >= INSERTED_AT_ONCE {
            let (batch, rest) = rows.split_at(INSERTED_AT_ONCE);
            let mut values: Vec<&dyn fastctx_storage::rusqlite::ToSql> = Vec::with_capacity(INSERTED_AT_ONCE * 11);
            for row in batch {
                values.extend([
                    &row.stream as &dyn fastctx_storage::rusqlite::ToSql,
                    &row.place,
                    &row.item,
                    &row.revision,
                    &row.state,
                    &row.kind,
                    &row.schema,
                    &row.body,
                    &row.objects,
                    &row.holding,
                    &row.local,
                ]);
            }
            db.prepare_cached(&INSERT_MANY)?.execute(values.as_slice())?;
            rows = rest;
        }
        for row in rows {
            db.prepare_cached(
                "INSERT OR FAIL INTO items (stream, place, item, revision, state, kind, schema, body, objects, holding, local)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?
            .execute(params![row.stream, row.place, row.item, row.revision, row.state, row.kind, row.schema, row.body, row.objects, row.holding, row.local])?;
        }
        Ok(())
    }
}

/// Writes `record` at `revision` over the row `existing`, if any.
fn replace(db: &Connection, at: Applying, existing: Option<Row>, revision: u64, record: &KindRecord) -> fastctx_storage::Result<()> {
    let local = written(db, at.stream)?;
    replace_at(db, at, existing, revision, record, local)
}

/// As `replace`, under the write number `local`.
fn replace_at(
    db: &Connection,
    at: Applying,
    existing: Option<Row>,
    revision: u64,
    record: &KindRecord,
    local: i64,
) -> fastctx_storage::Result<()> {
    let record_item = &record.record.item;
    if let Some(Row { place, objects, holding, .. }) = existing {
        db.prepare_cached("DELETE FROM items WHERE stream = ?1 AND place = ?2 AND item = ?3")?
            .execute(params![at.stream, place, record_item])?;
        if let Some(objects) = objects {
            let objects = unpack(&objects).map_err(crate::error::invalid)?;
            for object in objects {
                unrefer(db, at.scope, holding, at.stream, &object.hash)?;
            }
        }
    }
    let (state, body, objects) = match &record.record.state {
        State::Is { body, objects } => {
            for object in objects {
                refer(db, at.scope, at.holding, at.stream, &object.hash)?;
            }
            (IS, Some(body.as_slice()), Some(pack(objects)))
        }
        State::Gone => (GONE, None, None),
    };
    db.prepare_cached(
        "INSERT OR FAIL INTO items (stream, place, item, revision, state, kind, schema, body, objects, holding, local)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?
    .execute(params![
        at.stream,
        record.record.order,
        record_item,
        revision as i64,
        state,
        record.kind,
        record.schema,
        body,
        objects,
        at.holding,
        local
    ])?;
    Ok(())
}

fn refer(db: &Connection, scope: i64, holding: i64, stream: i64, hash: &[u8; 32]) -> fastctx_storage::Result<()> {
    db.prepare_cached(
        "INSERT INTO object_refs (scope, holding, stream, hash, count) VALUES (?1, ?2, ?3, ?4, 1)
         ON CONFLICT (scope, holding, stream, hash) DO UPDATE SET count = count + 1",
    )?
    .execute(params![scope, holding, stream, &hash[..]])?;
    Ok(())
}

pub(super) fn unrefer(db: &Connection, scope: i64, holding: i64, stream: i64, hash: &[u8; 32]) -> fastctx_storage::Result<()> {
    db.prepare_cached("UPDATE object_refs SET count = count - 1 WHERE scope = ?1 AND holding = ?2 AND stream = ?3 AND hash = ?4")?
        .execute(params![scope, holding, stream, &hash[..]])?;
    let gone = db
        .prepare_cached("DELETE FROM object_refs WHERE scope = ?1 AND holding = ?2 AND stream = ?3 AND hash = ?4 AND count <= 0")?
        .execute(params![scope, holding, stream, &hash[..]])?;
    if gone > 0 {
        // Collection waits a grace after the last reference went (I2).
        db.prepare_cached("UPDATE objects SET unreferenced = ?2 WHERE hash = ?1 AND state = 1")?
            .execute(params![&hash[..], super::objects::now()])?;
    }
    Ok(())
}

/// Drops the floors of `stream` the current period has caught up with:
/// every record up to `through` is applied, so the item stands as it is.
/// A page asked before they went is asked again (`reclaimed`, PH01); a
/// reader that had not read them yet reads the stream anew, so none misses
/// what a floor told gone.
pub(super) fn floors_passed(db: &Connection, stream: i64, through: u64) -> fastctx_storage::Result<()> {
    let gone = db
        .prepare_cached("DELETE FROM items WHERE stream = ?1 AND state = 2 AND revision <= ?2")?
        .execute(params![stream, through as i64])?;
    if gone > 0 {
        let at = next_write(db)?;
        db.prepare_cached("UPDATE streams SET reclaimed = ?2 WHERE id = ?1")?.execute(params![stream, at])?;
        super::view::rebased(db, stream)?;
    }
    Ok(())
}
