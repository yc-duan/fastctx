//! Reading the index: one bounded range of a stream's items at their latest.
//! A read takes one snapshot and writes nothing.

use crate::record::{self, ObjectRef};
use crate::{Content, Error, Result, StreamKey};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, Row, params};

/// Where an item stands in its stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub order: Vec<u8>,
    pub item: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum Anchor {
    /// The stream's last items.
    Latest,
    /// The items just before a place, which is not among them.
    Before(Place),
    /// The items just after a place, which is not among them.
    After(Place),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub item: Vec<u8>,
    pub order: Vec<u8>,
    /// The stream sequence of the record that last set it.
    pub sequence: u64,
    pub body: Vec<u8>,
    pub objects: Vec<ObjectRef>,
}

impl Item {
    pub fn place(&self) -> Place {
        Place { order: self.order.clone(), item: self.item.clone() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    /// The stream's revision as the page was read; none when the stream is
    /// not here.
    pub revision: Option<u64>,
    /// In the stream's order, earliest first.
    pub items: Vec<Item>,
    /// Where to read on from towards the stream's start, when there may be
    /// more that way.
    pub older: Option<Place>,
    /// Where to read on from towards its end, when there may be more that way.
    pub newer: Option<Place>,
    /// How many records the stream held as the page was read: where
    /// `Content::changes` reads on from, for what comes after this page.
    pub through: u64,
}

const COLUMNS: &str = "place, item, sequence, body, objects";

impl Content {
    /// Reads at most `max_items` items of at most `max_bytes` of bodies
    /// together, one item at least. It blocks for the read: call on a thread
    /// that may block, or a task thread, which waits in place.
    pub fn page(&self, stream: &StreamKey, anchor: &Anchor, max_items: u32, max_bytes: u32) -> Result<Page> {
        stream.check()?;
        if max_items == 0 {
            return Err(Error::Invalid("a page holds one item at least".into()));
        }
        Ok(self.0.database.read_now(|db| page(db, stream, anchor, max_items as usize, max_bytes as usize))?)
    }

    /// One item at its latest, by its identity.
    pub fn item(&self, stream: &StreamKey, item: &[u8]) -> Result<Option<Item>> {
        stream.check()?;
        Ok(self.0.database.read_now(|db| {
            let snapshot = db.unchecked_transaction()?;
            let Some((id, _)) = locate(&snapshot, stream)? else { return Ok(None) };
            snapshot
                .prepare_cached(&format!("SELECT {COLUMNS} FROM items WHERE stream = ?1 AND item = ?2"))?
                .query_row(params![id, item], read)
                .optional()?
                .map(whole)
                .transpose()
        })?)
    }

    /// Where every item of a stream stands, without its state and in no
    /// order: what a producer that makes the stream from a whole source
    /// compares that source with. It reads the index by identity, never a
    /// state; for as long as the stream has items, on a thread that may block.
    pub fn places(&self, stream: &StreamKey) -> Result<Vec<Place>> {
        stream.check()?;
        Ok(self.0.database.read_background_now(|db| {
            let snapshot = db.unchecked_transaction()?;
            let Some((id, _)) = locate(&snapshot, stream)? else { return Ok(Vec::new()) };
            let mut indexed = snapshot
                .prepare_cached("SELECT place, item FROM items INDEXED BY items_by_identity WHERE stream = ?1")?;
            let places = indexed
                .query_map([id], |row| Ok(Place { order: row.get(0)?, item: row.get(1)? }))?
                .collect::<fastctx_storage::rusqlite::Result<_>>()?;
            Ok(places)
        })?)
    }

    /// Where the items of a stream stand whose identities begin with
    /// `prefix`, in the order of their identities and `most` of them at
    /// most: what a producer that names its items so asks to learn whether
    /// one of a name is in the stream already, and where, without reading
    /// the stream. It reads the index by identity, never a state.
    pub fn named(&self, stream: &StreamKey, prefix: &[u8], most: u32) -> Result<Vec<Place>> {
        stream.check()?;
        if prefix.is_empty() || prefix.len() > record::KEY_BYTES {
            return Err(Error::Invalid(format!("a prefix of identities is 1 to {} bytes", record::KEY_BYTES)));
        }
        // What begins with the prefix lies from it up to, and without, the
        // prefix with its last byte that can be raised raised by one.
        let mut past = prefix.to_vec();
        while past.last() == Some(&u8::MAX) {
            past.pop();
        }
        let past = past.split_last().map(|(last, kept)| [kept, &[last + 1][..]].concat());
        Ok(self.0.database.read_now(|db| {
            let snapshot = db.unchecked_transaction()?;
            let Some((id, _)) = locate(&snapshot, stream)? else { return Ok(Vec::new()) };
            // Two statements, not one with a bound that may be absent: an
            // absent-or-less condition keeps the index from ending the range,
            // and a prefix no item begins with was then looked for to the
            // stream's end.
            let placed = |row: &fastctx_storage::rusqlite::Row<'_>| Ok(Place { order: row.get(0)?, item: row.get(1)? });
            let places = match &past {
                Some(past) => snapshot
                    .prepare_cached(
                        "SELECT place, item FROM items INDEXED BY items_by_identity \
                         WHERE stream = ?1 AND item >= ?2 AND item < ?3 ORDER BY item LIMIT ?4",
                    )?
                    .query_map(params![id, prefix, past, most], placed)?
                    .collect::<fastctx_storage::rusqlite::Result<_>>()?,
                // Nothing lies past a prefix of the highest bytes only.
                None => snapshot
                    .prepare_cached(
                        "SELECT place, item FROM items INDEXED BY items_by_identity \
                         WHERE stream = ?1 AND item >= ?2 ORDER BY item LIMIT ?3",
                    )?
                    .query_map(params![id, prefix, most], placed)?
                    .collect::<fastctx_storage::rusqlite::Result<_>>()?,
            };
            Ok(places)
        })?)
    }

    /// The stream's revision; none when the stream is not here.
    pub fn revision(&self, stream: &StreamKey) -> Result<Option<u64>> {
        stream.check()?;
        Ok(self.0.database.read_now(|db| revision(db, stream))?)
    }

    /// Whether records of `scope` refer to an object: what lets a reader of
    /// the scope read it.
    pub fn refers(&self, scope: &str, hash: &[u8; 32]) -> Result<bool> {
        Ok(self.0.database.read_now(|db| {
            Ok(db
                .prepare_cached(
                    "SELECT 1 FROM object_refs JOIN scopes ON scopes.id = object_refs.scope \
                     WHERE scopes.key = ?1 AND object_refs.hash = ?2 AND object_refs.count > 0",
                )?
                .query_row(params![scope, &hash[..]], |_| Ok(()))
                .optional()?
                .is_some())
        })?)
    }

    /// A producer's position as it last kept it.
    pub fn checkpoint(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.database.read_now(|db| {
            Ok(db
                .prepare_cached("SELECT state FROM checkpoints WHERE name = ?1")?
                .query_row([name], |row| row.get(0))
                .optional()?)
        })?)
    }
}

pub(crate) fn revision(db: &Connection, stream: &StreamKey) -> fastctx_storage::Result<Option<u64>> {
    Ok(locate(db, stream)?.map(|(_, revision)| revision))
}

fn locate(db: &Connection, stream: &StreamKey) -> fastctx_storage::Result<Option<(i64, u64)>> {
    Ok(db
        .prepare_cached(
            "SELECT streams.id, streams.revision FROM streams JOIN scopes ON scopes.id = streams.scope \
             WHERE scopes.key = ?1 AND streams.name = ?2",
        )?
        .query_row(params![stream.scope, stream.name], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)))
        .optional()?)
}

type Stored = (Vec<u8>, Vec<u8>, i64, Vec<u8>, Vec<u8>);

fn read(row: &Row<'_>) -> fastctx_storage::rusqlite::Result<Stored> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
}

fn whole((order, item, sequence, body, objects): Stored) -> fastctx_storage::Result<Item> {
    let objects = record::unpack(&objects).map_err(fastctx_storage::Error::new)?;
    Ok(Item { item, order, sequence: sequence as u64, body, objects })
}

fn page(
    db: &Connection,
    stream: &StreamKey,
    anchor: &Anchor,
    max_items: usize,
    max_bytes: usize,
) -> fastctx_storage::Result<Page> {
    // The stream's revision and its items are read as of one moment.
    let snapshot = db.unchecked_transaction()?;
    let Some((id, revision)) = locate(&snapshot, stream)? else { return Ok(Page::default()) };
    let through: i64 =
        snapshot.prepare_cached("SELECT next FROM streams WHERE id = ?1")?.query_row([id], |row| row.get(0))?;
    let limit = max_items as i64 + 1;
    let mut items = Vec::new();
    let mut more = false;
    {
        let sql = match anchor {
            Anchor::Latest => {
                format!("SELECT {COLUMNS} FROM items WHERE stream = ?1 ORDER BY place DESC, item DESC LIMIT ?2")
            }
            Anchor::Before(_) => format!(
                "SELECT {COLUMNS} FROM items WHERE stream = ?1 AND (place, item) < (?2, ?3) \
                 ORDER BY place DESC, item DESC LIMIT ?4"
            ),
            Anchor::After(_) => format!(
                "SELECT {COLUMNS} FROM items WHERE stream = ?1 AND (place, item) > (?2, ?3) \
                 ORDER BY place ASC, item ASC LIMIT ?4"
            ),
        };
        let mut statement = snapshot.prepare_cached(&sql)?;
        let mut rows = match anchor {
            Anchor::Latest => statement.query(params![id, limit])?,
            Anchor::Before(place) | Anchor::After(place) => {
                statement.query(params![id, place.order, place.item, limit])?
            }
        };
        let mut bytes = 0usize;
        while let Some(row) = rows.next()? {
            let stored = read(row)?;
            if items.len() == max_items || (!items.is_empty() && bytes + stored.3.len() > max_bytes) {
                more = true;
                break;
            }
            bytes += stored.3.len();
            items.push(whole(stored)?);
        }
    }
    if !matches!(anchor, Anchor::After(_)) {
        items.reverse();
    }
    let first = items.first().map(Item::place);
    let last = items.last().map(Item::place);
    let (older, newer) = match anchor {
        Anchor::Latest => (first.filter(|_| more), None),
        Anchor::Before(_) => (first.filter(|_| more), last),
        Anchor::After(_) => (first, last.filter(|_| more)),
    };
    Ok(Page { revision: Some(revision), items, older, newer, through: through as u64 })
}
