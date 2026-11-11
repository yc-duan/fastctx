//! Query-local metadata on temporary disk. No conversation text is retained.

use fastctx_types::timeline_pages::TimelinePlace;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) struct Index {
    connection: Connection,
    _directory: tempfile::TempDir,
    total: u64,
    examined: u64,
    previews: u64,
}
#[derive(Clone)]
pub(crate) struct Count {
    pub place: TimelinePlace,
    pub sequence: u64,
    pub count: u32,
    pub preview: bool,
}
#[derive(Deserialize, Serialize)]
pub(crate) struct Seeking {
    after: TimelinePlace,
    left: String,
}
#[derive(Serialize)]
pub(crate) struct Found {
    pub place: TimelinePlace,
    pub ordinal: u32,
}
#[derive(Serialize)]
pub(crate) struct Hit {
    found: Option<Found>,
    next: Option<Seeking>,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Ranking {
    after: TimelinePlace,
    before: String,
}
#[derive(Serialize)]
pub(crate) struct Rank {
    found: Option<(TimelinePlace, String)>,
    next: Option<Ranking>,
}

impl Index {
    pub fn new() -> Result<Self, String> {
        let directory = tempfile::Builder::new().prefix("fastctx-search-").tempdir().map_err(|e| e.to_string())?;
        let connection = Connection::open(directory.path().join("matches.sqlite3")).map_err(|e| e.to_string())?;
        connection.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-1024;
            CREATE TABLE events(item TEXT PRIMARY KEY, position TEXT NOT NULL, sequence TEXT NOT NULL, count INTEGER NOT NULL, preview INTEGER NOT NULL);
            CREATE INDEX matching_order ON events(position,item) WHERE count>0;").map_err(|e| e.to_string())?;
        Ok(Self { connection, _directory: directory, total: 0, examined: 0, previews: 0 })
    }
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.total, self.examined, self.previews)
    }
    pub fn apply(&mut self, events: Vec<Count>) -> Result<(), String> {
        let transaction = self.connection.transaction().map_err(|e| e.to_string())?;
        let (mut total, mut examined, mut previews) = (self.total, self.examined, self.previews);
        for event in events {
            let sequence = format!("{:020}", event.sequence);
            let old: Option<(String, u32, bool)> = transaction
                .query_row("SELECT sequence,count,preview FROM events WHERE item=?1", [&event.place.item], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|e| e.to_string())?;
            if old.as_ref().is_some_and(|(known, _, _)| known > &sequence) {
                continue;
            }
            total = total - old.as_ref().map_or(0, |(_, count, _)| u64::from(*count)) + u64::from(event.count);
            previews =
                previews - old.as_ref().map_or(0, |(_, _, preview)| u64::from(*preview)) + u64::from(event.preview);
            if old.is_none() {
                examined += 1;
            }
            transaction.execute("INSERT INTO events VALUES(?1,?2,?3,?4,?5) ON CONFLICT(item) DO UPDATE SET position=excluded.position,sequence=excluded.sequence,count=excluded.count,preview=excluded.preview", params![event.place.item,event.place.order,sequence,event.count,event.preview]).map_err(|e| e.to_string())?;
        }
        transaction.commit().map_err(|e| e.to_string())?;
        (self.total, self.examined, self.previews) = (total, examined, previews);
        Ok(())
    }
    pub fn rank(&self, item: &str, ordinal: u32, cursor: Option<Ranking>) -> Result<Rank, String> {
        let target: Option<(String, u32)> = self
            .connection
            .query_row("SELECT position,count FROM events WHERE item=?1", [item], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((order, count)) = target.filter(|(_, count)| ordinal < *count) else {
            return Ok(Rank { found: None, next: None });
        };
        let _ = count;
        let target = TimelinePlace { order, item: item.into() };
        let (after, mut before) = cursor
            .map_or((TimelinePlace { order: String::new(), item: String::new() }, 0), |cursor| {
                (cursor.after, cursor.before.parse::<u64>().unwrap_or(0))
            });
        let mut statement=self.connection.prepare("SELECT position,item,count FROM events WHERE count>0 AND (position,item)>(?1,?2) AND (position,item)<=(?3,?4) ORDER BY position,item LIMIT 400").map_err(|e|e.to_string())?;
        let mut rows =
            statement.query(params![after.order, after.item, target.order, target.item]).map_err(|e| e.to_string())?;
        let mut last = None;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let place = TimelinePlace {
                order: row.get(0).map_err(|e| e.to_string())?,
                item: row.get(1).map_err(|e| e.to_string())?,
            };
            if place.item == item {
                return Ok(Rank { found: Some((place, (before + u64::from(ordinal)).to_string())), next: None });
            }
            before += u64::from(row.get::<_, u32>(2).map_err(|e| e.to_string())?);
            last = Some(place);
        }
        Ok(Rank { found: None, next: last.map(|after| Ranking { after, before: before.to_string() }) })
    }

    /// At most 400 metadata rows per call; the caller can cancel between calls.
    pub fn hit(&self, ordinal: u64, cursor: Option<Seeking>) -> Result<Hit, String> {
        let (after, mut left) = cursor
            .map_or((TimelinePlace { order: String::new(), item: String::new() }, ordinal), |cursor| {
                (cursor.after, cursor.left.parse().unwrap_or(u64::MAX))
            });
        let mut statement = self.connection.prepare("SELECT position,item,count FROM events WHERE count>0 AND (position,item)>(?1,?2) ORDER BY position,item LIMIT 400").map_err(|e| e.to_string())?;
        let mut rows = statement.query(params![after.order, after.item]).map_err(|e| e.to_string())?;
        let mut last = None;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let place = TimelinePlace {
                order: row.get(0).map_err(|e| e.to_string())?,
                item: row.get(1).map_err(|e| e.to_string())?,
            };
            let count: u32 = row.get(2).map_err(|e| e.to_string())?;
            if left < u64::from(count) {
                return Ok(Hit { found: Some(Found { place, ordinal: left as u32 }), next: None });
            }
            left -= u64::from(count);
            last = Some(place);
        }
        Ok(Hit { found: None, next: last.map(|after| Seeking { after, left: left.to_string() }) })
    }
}
