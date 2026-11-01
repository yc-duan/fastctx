//! Immutable event tags support selective reads without replaying unrelated history.
use super::{SessionEvent, sql};
use fastctx_storage::{
    Database, Error, ErrorKind, Priority, Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::{
    Address,
    layout::ConversationDir,
    residents::Notice,
    session::{Happened, Speaker},
};
use std::io;

pub(super) struct Tags {
    arrival: [u8; 16],
    values: Vec<(&'static str, String)>,
}

pub(crate) fn address_key(address: &Address) -> String {
    serde_json::to_string(address).expect("address serializes")
}
pub(crate) fn turn_key(turn: Option<&str>) -> String {
    serde_json::to_string(&turn).expect("turn serializes")
}
impl Tags {
    pub(super) fn of(event: &SessionEvent) -> Self {
        let mut values = Vec::new();
        if let Happened::Heard { delivery, turn, from, notice, .. } = &event.happened {
            if let Some(delivery) = delivery {
                values.push(("delivery", delivery.to_string()));
            }
            match from {
                Some(Speaker::User { .. }) => values.push(("user", String::new())),
                Some(Speaker::Resident { address }) => values.push(("resident", address_key(address))),
                Some(Speaker::World) => {
                    values.push(("world", String::new()));
                    if let Some(turn) = turn {
                        values.push(("world-turn", turn.clone()));
                    }
                }
                _ => {}
            }
            match notice.as_deref() {
                Some(Notice::Words { goal_report: Some(report), .. }) => {
                    values.push(("reports", String::new()));
                    values.push(("report", report.id.to_string()));
                    if let Some(completion) = &report.completion {
                        values.push(("completion", completion.to_string()));
                    }
                }
                Some(Notice::Answer { .. }) => values.push(("answer-turn", turn_key(turn.as_deref()))),
                _ => {}
            }
        }
        Self { arrival: (event.at.as_nanosecond() ^ i128::MIN).to_be_bytes(), values }
    }
    pub(super) fn insert(&self, db: &Connection, scope: &str, sequence: u64) -> Result<()> {
        let mut insert = db.prepare_cached(
            "INSERT OR IGNORE INTO session_event_tags(conversation,tag,value,sequence,arrival) VALUES(?1,?2,?3,?4,?5)",
        )?;
        for (tag, value) in &self.values {
            insert.execute(params![scope, tag, value, sequence.to_be_bytes().as_slice(), self.arrival.as_slice()])?;
        }
        Ok(())
    }
}

fn covered(db: &Connection, scope: &str) -> Result<u64> {
    let bytes: Option<Vec<u8>> = db
        .prepare_cached("SELECT sequence FROM session_search_heads WHERE conversation=?1")?
        .query_row([scope], |row| row.get(0))
        .optional()?;
    bytes.map(sql::number).transpose().map(|number| number.unwrap_or(0))
}
pub(super) fn advance(db: &Connection, scope: &str, sequence: u64) -> Result<()> {
    db.prepare_cached("INSERT INTO session_search_heads(conversation,sequence) VALUES(?1,?2) ON CONFLICT(conversation) DO UPDATE SET sequence=excluded.sequence WHERE sequence<excluded.sequence")?
        .execute((scope,sequence.to_be_bytes().as_slice()))?;
    Ok(())
}
pub(super) fn appended(db: &Connection, scope: &str, previous: u64, sequence: u64, tags: &Tags) -> Result<()> {
    tags.insert(db, scope, sequence)?;
    if covered(db, scope)? >= previous {
        advance(db, scope, sequence)?;
    }
    Ok(())
}

/// Backfill an earlier SQL layout in bounded commits. Concurrent readers may
/// repeat a batch; immutable tags and a monotone fence make that harmless.
fn prepare(directory: &ConversationDir) -> io::Result<(Database, String)> {
    let (database, scope) = super::database(directory)?;
    let fence = database.read_now(|db| Ok(sql::head(db, &scope)?.1)).map_err(io::Error::other)?;
    loop {
        let page=database.read_background_now(|db| {
            let after=covered(db,&scope)?;
            if after>=fence { return Ok(Vec::new()); }
            let mut query=db.prepare_cached("SELECT sequence,body FROM session_events WHERE conversation=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT 16")?;
            Ok(query.query_map(params![scope,after.to_be_bytes().as_slice(),fence.to_be_bytes().as_slice()],|row|Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,Vec<u8>>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
        }).map_err(io::Error::other)?;
        if page.is_empty() {
            break;
        }
        let tags = page
            .into_iter()
            .map(|(sequence, body)| {
                let event: SessionEvent = serde_json::from_slice(&body)
                    .map_err(|error| Error::with_kind(ErrorKind::Corrupt, error.to_string()))?;
                Ok((sql::number(sequence)?, Tags::of(&event)))
            })
            .collect::<Result<Vec<_>>>()
            .map_err(io::Error::other)?;
        let bytes =
            tags.iter().map(|(_, tags)| tags.values.iter().map(|(_, value)| value.len() + 96).sum::<usize>()).sum();
        let conversation = scope.clone();
        database
            .write_now_at(Priority::Background, bytes, move |db| {
                for (sequence, tags) in &tags {
                    tags.insert(db, &conversation, *sequence)?;
                }
                advance(db, &conversation, tags.last().expect("nonempty event page").0)
            })
            .map_err(io::Error::other)?;
    }
    Ok((database, scope))
}

/// A fixed acceptance fence is shared by every selection in one decision.
pub(crate) struct History {
    database: Database,
    scope: String,
    fence: u64,
}
pub(crate) fn open(directory: &ConversationDir) -> io::Result<History> {
    let (database, scope) = prepare(directory)?;
    let fence = database.read_now(|db| Ok(sql::head(db, &scope)?.1)).map_err(io::Error::other)?;
    Ok(History { database, scope, fence })
}
pub(crate) fn all(directory: &ConversationDir, tag: &str, value: &str) -> io::Result<Vec<(u64, SessionEvent)>> {
    open(directory)?.all(tag, value)
}
impl History {
    pub(crate) fn all(&self, tag: &str, value: &str) -> io::Result<Vec<(u64, SessionEvent)>> {
        let (database, scope, fence) = (&self.database, &self.scope, self.fence);
        let mut after = 0u64;
        let mut result = Vec::new();
        loop {
            let page=database.read_background_now(|db| {
                let mut query=db.prepare_cached("SELECT t.sequence,e.body FROM session_event_tags t JOIN session_events e ON e.conversation=t.conversation AND e.sequence=t.sequence WHERE t.conversation=?1 AND t.tag=?2 AND t.value=?3 AND t.sequence>?4 AND t.sequence<=?5 ORDER BY t.sequence LIMIT 128")?;
                let rows=query.query_map(params![scope,tag,value,after.to_be_bytes().as_slice(),fence.to_be_bytes().as_slice()],|row|Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,Vec<u8>>(1)?)))?;
                rows.map(|row| { let (sequence,body)=row?;Ok((sql::number(sequence)?,crate::records::decode(&body)?)) }).collect::<Result<Vec<_>>>()
            }).map_err(io::Error::other)?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.0;
            result.extend(page);
        }
        Ok(result)
    }
    /// A time-ordered prefix keeps nanosecond ties in acceptance order.
    pub(crate) fn recent(
        &self,
        tag: &str,
        value: &str,
        since: jiff::Timestamp,
        limit: usize,
    ) -> io::Result<Vec<(u64, SessionEvent)>> {
        let since = (since.as_nanosecond() ^ i128::MIN).to_be_bytes();
        self.database.read_now(|db| {
            let mut query=db.prepare_cached("SELECT t.sequence,e.body FROM session_event_tags t JOIN session_events e ON e.conversation=t.conversation AND e.sequence=t.sequence WHERE t.conversation=?1 AND t.tag=?2 AND t.value=?3 AND t.arrival>=?4 AND t.sequence<=?6 ORDER BY t.arrival DESC,t.sequence DESC LIMIT ?5")?;
            let rows=query.query_map(params![self.scope,tag,value,since.as_slice(),limit.clamp(1,128) as i64,self.fence.to_be_bytes().as_slice()],|row|Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,Vec<u8>>(1)?)))?;
            rows.map(|row| {let (sequence,body)=row?;Ok((sql::number(sequence)?,crate::records::decode(&body)?))}).collect()
        }).map_err(io::Error::other)
    }
}
