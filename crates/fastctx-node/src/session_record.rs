//! Indexed session events, with a durable per-conversation acceptance sequence.

#[cfg(feature = "storage-drill")]
pub(crate) mod drill;
mod legacy;
mod migrate;
pub(crate) mod search;
mod sql;
pub(crate) mod upgrade;
pub(crate) use legacy::SCHEMA as MIGRATION_SCHEMA;
#[cfg(feature = "storage-drill")]
pub(crate) use sql::SCHEMA;

use fastctx_storage::{
    Database,
    rusqlite::{OptionalExtension, params},
};
use fastctx_types::{layout::ConversationDir, session::SessionEvent};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

#[derive(Serialize, Deserialize)]
struct Recorded {
    #[serde(default)]
    sequence: Option<u64>,
    #[serde(flatten)]
    event: SessionEvent,
}

fn database(directory: &ConversationDir) -> io::Result<(Database, String)> {
    let (database, conversation) = crate::record_database::get(directory)?;
    migrate::ensure(directory, &database, &conversation)?;
    Ok((database, conversation))
}

pub fn append(directory: &ConversationDir, key: &str, event: &SessionEvent) -> io::Result<bool> {
    let (database, conversation) = database(directory)?;
    let (key, body) = (key.to_string(), serde_json::to_vec(event)?);
    let tags = search::Tags::of(event);
    // An event is told live before it is kept and the host keeps its own
    // record: its row does not wait for the disk (`records::write_relaxed_at`).
    database
        .write_relaxed_now_at(fastctx_storage::Priority::Interactive, body.len() + key.len(), move |db| {
            sql::append(db, &conversation, &key, &body, &tags)
        })
        .map_err(io::Error::other)
}

pub(crate) fn contains(directory: &ConversationDir, key: &str) -> io::Result<bool> {
    let (database, conversation) = database(directory)?;
    database
        .read_now(|db| {
            Ok(db
                .query_row(
                    "SELECT 1 FROM session_events WHERE conversation=?1 AND identity=?2",
                    (&conversation, key),
                    |row| row.get::<_, i32>(0),
                )
                .optional()?
                .is_some())
        })
        .map_err(io::Error::other)
}

/// One immutable page and the high-water mark of the snapshot it came from.
pub(crate) struct Page {
    pub latest: u64,
    pub next: u64,
    pub events: Vec<(u64, SessionEvent)>,
}

/// Reads at most 512 events strictly after `after`, through the supplied fence.
/// An absent fence captures the latest accepted sequence in this read snapshot.
pub(crate) fn page(directory: &ConversationDir, after: u64, through: Option<u64>, limit: usize) -> io::Result<Page> {
    let (database, conversation) = database(directory)?;
    database.read_now(|db| {
        let transaction=db.unchecked_transaction()?;
        let (_,latest,_)=sql::head(&transaction,&conversation)?;
        let fence=through.unwrap_or(latest).min(latest);
        let mut query=transaction.prepare_cached("SELECT sequence,body FROM session_events WHERE conversation=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT ?4")?;
        let rows=query.query_map(params![conversation,after.to_be_bytes().as_slice(),fence.to_be_bytes().as_slice(),limit.clamp(1,512) as i64],|row|Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,Vec<u8>>(1)?)))?;
        let mut events=Vec::new();
        for row in rows {
            let (sequence,body)=row?;
            let event=serde_json::from_slice(&body).map_err(|error|fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Corrupt,error.to_string()))?;
            events.push((sql::number(sequence)?,event));
        }
        drop(query);
        transaction.commit()?;
        let next=events.last().map_or(after,|row|row.0);
        Ok(Page{latest:fence,next,events})
    }).map_err(io::Error::other)
}

/// What `page_each` hands each event to: its sequence and body.
pub(crate) type Each<'a> = &'a mut dyn FnMut(u64, &[u8]) -> Result<(), String>;

/// As `page`, each event handed to `each` as it is read — its sequence and
/// its body as kept — and none held (design §28.51 ⑥-1): the high-water
/// mark of the snapshot, the sequence of the last handed on (`after` with
/// none), and how many were.
pub(crate) fn page_each(
    directory: &ConversationDir,
    after: u64,
    through: Option<u64>,
    limit: usize,
    each: Each<'_>,
) -> io::Result<(u64, u64, usize)> {
    let (database, conversation) = database(directory)?;
    let mut refused = None;
    let read = database.read_now(|db| {
        let transaction = db.unchecked_transaction()?;
        let (_, latest, _) = sql::head(&transaction, &conversation)?;
        let fence = through.unwrap_or(latest).min(latest);
        let mut query = transaction.prepare_cached(
            "SELECT sequence,body FROM session_events WHERE conversation=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT ?4",
        )?;
        let mut rows = query.query(params![conversation, after.to_be_bytes().as_slice(), fence.to_be_bytes().as_slice(), limit.clamp(1, 512) as i64])?;
        let (mut next, mut count) = (after, 0);
        while let Some(row) = rows.next()? {
            let sequence = sql::number(row.get::<_, Vec<u8>>(0)?)?;
            let body: Vec<u8> = row.get(1)?;
            if let Err(error) = each(sequence, &body) {
                refused = Some(error);
                break;
            }
            (next, count) = (sequence, count + 1);
        }
        drop(rows);
        drop(query);
        transaction.commit()?;
        Ok((fence, next, count))
    });
    if let Some(error) = refused {
        return Err(io::Error::other(error));
    }
    read.map_err(io::Error::other)
}

/// The first event of a conversation's record that `found` takes, as of the
/// whole record read (`history`): every event read, a page at a time and
/// none held past `found` (design §28.51 ⑥) — one that cannot be read
/// fails the reading wherever it stands, as it fails `history`.
pub(crate) fn first<T>(directory: &ConversationDir, found: &mut dyn FnMut(SessionEvent) -> Option<T>) -> io::Result<Option<T>> {
    let (mut after, mut through, mut first) = (0, None, None);
    loop {
        let (latest, next, count) = page_each(directory, after, through, 512, &mut |_, body| {
            let event: SessionEvent = serde_json::from_slice(body).map_err(|error| error.to_string())?;
            if first.is_none() {
                first = found(event);
            }
            Ok(())
        })?;
        through = Some(latest);
        if count == 0 || next >= latest {
            return Ok(first);
        }
        after = next;
    }
}

pub(crate) fn latest(directory: &ConversationDir) -> io::Result<u64> {
    let (database, conversation) = database(directory)?;
    database.read_now(|db| Ok(sql::head(db, &conversation)?.1)).map_err(io::Error::other)
}

/// Compatibility for consumers of a complete history. New subscriptions read
/// `page` and keep its sequence fence, without materializing earlier pages.
pub fn history(directory: &ConversationDir) -> io::Result<Vec<SessionEvent>> {
    let mut result = Vec::new();
    let mut after = 0;
    let mut through = None;
    loop {
        let page = page(directory, after, through, 512)?;
        through = Some(page.latest);
        if page.events.is_empty() {
            break;
        }
        after = page.next;
        result.extend(page.events.into_iter().map(|(_, event)| event));
        if after >= page.latest {
            break;
        }
    }
    Ok(result)
}


pub fn stamp(directory: &Path, digest: &mut impl FnMut(&[u8])) -> io::Result<()> {
    digest(&latest(&ConversationDir::at(directory))?.to_be_bytes());
    Ok(())
}

pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    database(directory).map(|_| ())
}

pub(crate) fn append_in(
    connection: &fastctx_storage::rusqlite::Connection,
    conversation: &str,
    key: &str,
    event: &SessionEvent,
) -> fastctx_storage::Result<bool> {
    let body = serde_json::to_vec(event).map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
    sql::append(connection, conversation, key, &body, &search::Tags::of(event))
}
