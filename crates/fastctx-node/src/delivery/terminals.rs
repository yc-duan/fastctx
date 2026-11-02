//! Terminal contents are immutable; reader additions publish in bounded batches.
use crate::{leases, record_payloads, records};
use fastctx_storage::{
    Database, Error, ErrorKind, Priority, Result,
    rusqlite::{OptionalExtension, params},
};
use fastctx_types::{Address, layout::ConversationDir, session::TurnEnd};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{Arc, Condvar, LazyLock, Mutex, Weak},
};

pub(crate) const FAMILY: &str = "terminal-heads";
const CONTENTS: &str = "terminal-end";
const INLINE: usize = 4096;
/// A few rows for each turn of a conversation, its end and who reads it,
/// kept as long as the conversation is.
pub(crate) const SCHEMA:&str="
CREATE TABLE IF NOT EXISTS terminal_reader_heads(conversation TEXT NOT NULL,turn TEXT NOT NULL,allocated INTEGER NOT NULL,PRIMARY KEY(conversation,turn)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS terminal_reader_revisions(conversation TEXT NOT NULL,turn TEXT NOT NULL,revision INTEGER NOT NULL,token TEXT NOT NULL,expected INTEGER NOT NULL,written INTEGER NOT NULL,ready INTEGER NOT NULL,PRIMARY KEY(conversation,turn,revision)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS terminal_reader_unready ON terminal_reader_revisions(conversation,turn,revision) WHERE ready=0;
CREATE TABLE IF NOT EXISTS terminal_readers(conversation TEXT NOT NULL,turn TEXT NOT NULL,machine TEXT NOT NULL,resident TEXT NOT NULL,revision INTEGER NOT NULL,owed INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(conversation,turn,machine,resident)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS terminal_reader_unowed ON terminal_readers(conversation,turn,machine,resident) WHERE owed=0;
CREATE TABLE IF NOT EXISTS terminal_usage(conversation TEXT NOT NULL,turn TEXT NOT NULL,ledger TEXT NOT NULL,identity TEXT NOT NULL,complete INTEGER NOT NULL,PRIMARY KEY(conversation,turn)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS terminal_usage_pending ON terminal_usage(ledger,identity) WHERE complete=0;
CREATE TABLE IF NOT EXISTS terminal_work(conversation TEXT NOT NULL,turn TEXT NOT NULL,PRIMARY KEY(conversation,turn)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS terminal_readers_revision ON terminal_readers(conversation,turn,revision,machine,resident);
";
type Key = (PathBuf, String, String);
#[derive(Default)]
struct Gate {
    busy: Mutex<bool>,
    available: Condvar,
}
static GATES: LazyLock<leases::Registry<Key, Gate>> = LazyLock::new(Default::default);
static LIVE: LazyLock<Mutex<HashMap<String, Weak<Pin>>>> = LazyLock::new(Default::default);
struct Pin(String);
impl Drop for Pin {
    fn drop(&mut self) {
        LIVE.lock().expect("terminal reader pins").remove(&self.0);
    }
}
struct Order {
    gate: leases::Lease<Key, Gate>,
}
impl Order {
    fn acquire(database: &Database, scope: &str, turn: &str) -> Arc<Self> {
        let gate = GATES
            .get_or_insert_with(&(database.path().to_path_buf(), scope.to_string(), turn.to_string()), Gate::default);
        let mut busy = gate.busy.lock().expect("terminal publication order");
        while *busy {
            busy = gate.available.wait(busy).expect("terminal publication order");
        }
        *busy = true;
        drop(busy);
        Arc::new(Self { gate })
    }
}
impl Drop for Order {
    fn drop(&mut self) {
        *self.gate.busy.lock().expect("terminal publication order") = false;
        self.gate.available.notify_one();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "storage", content = "value", rename_all = "snake_case")]
pub(crate) enum End {
    Inline(Box<Content>),
    Payload(record_payloads::Reference),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Content {
    pub end: TurnEnd,
    pub usage: Option<crate::usage::terminal_record::Record>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Header {
    pub turn: String,
    pub ended_at: Option<Timestamp>,
    pub at: Timestamp,
    pub end: End,
    pub settles_queue: bool,
    pub queue_order: u64,
    pub reader_revision: i64,
    pub reader_count: u64,
    #[serde(default)]
    pub effects_complete: bool,
}
pub(crate) struct Published {
    pub header: Header,
    pub first: bool,
}
fn invalid(message: &str) -> Error {
    Error::with_kind(ErrorKind::Invalid, message)
}
fn corrupt(message: &str) -> Error {
    Error::with_kind(ErrorKind::Corrupt, message)
}

fn write<T: Send + 'static>(
    directory: &ConversationDir,
    bytes: usize,
    order: Arc<Order>,
    pin: Arc<Pin>,
    action: impl FnOnce(&records::Rows<'_>) -> Result<T> + Send + 'static,
) -> io::Result<T> {
    records::write_then_at(directory, Priority::Normal, bytes, action, move |_| {
        drop(pin);
        drop(order);
    })
}

pub(crate) struct Initial {
    pub end: TurnEnd,
    pub ended_at: Option<Timestamp>,
    pub settles_queue: bool,
    pub imported: Option<(Timestamp, u64)>,
    pub usage: Option<crate::usage::terminal_record::Record>,
}

pub(crate) async fn publish(
    directory: ConversationDir,
    turn: String,
    initial: Option<Initial>,
    readers: Vec<Address>,
    committed: impl FnOnce() + Send + 'static,
) -> io::Result<Published> {
    fastctx_platform::task::spawn_blocking(move || {
        let (database, scope) = records::locate(&directory)?;
        let order = Order::acquire(&database, &scope, &turn);
        publish_now(directory, database, scope, turn, initial, readers, order, committed)
    })
    .await
    .map_err(io::Error::other)?
}

/// Blocking entry point for legacy import and worker-owned callers.
pub(crate) fn publish_blocking(
    directory: &ConversationDir,
    turn: &str,
    initial: Initial,
    readers: Vec<Address>,
) -> io::Result<Published> {
    let (database, scope) = records::locate(directory)?;
    let order = Order::acquire(&database, &scope, turn);
    publish_now(directory.clone(), database, scope, turn.to_string(), Some(initial), readers, order, || {})
}

#[allow(clippy::too_many_arguments)]
fn publish_now(
    directory: ConversationDir,
    database: Database,
    scope: String,
    turn: String,
    initial: Option<Initial>,
    mut readers: Vec<Address>,
    order: Arc<Order>,
    committed: impl FnOnce() + Send + 'static,
) -> io::Result<Published> {
    readers.sort();
    readers.dedup();
    let (prior, added) = records::read_background(&directory, |rows| {
        let prior = header(rows, &turn)?;
        if prior.is_none() {
            let existed: bool = rows.connection().query_row(
                "SELECT EXISTS(SELECT 1 FROM terminal_reader_revisions WHERE conversation=?1 AND turn=?2 AND ready=1)",
                (rows.conversation(), &turn),
                |row| row.get(0),
            )?;
            if existed || record_payloads::root(rows.connection(), rows.conversation(), CONTENTS, &turn)?.is_some() {
                return Err(corrupt("a published terminal has lost its header"));
            }
        }
        let mut query=rows.connection().prepare_cached("SELECT r.ready FROM terminal_readers m JOIN terminal_reader_revisions r ON r.conversation=m.conversation AND r.turn=m.turn AND r.revision=m.revision WHERE m.conversation=?1 AND m.turn=?2 AND m.machine=?3 AND m.resident=?4")?;
        let mut added = Vec::new();
        for reader in readers {
            if !query
                .query_row((rows.conversation(), &turn, reader.machine.as_str(), reader.resident.as_str()), |row| {
                    row.get::<_, bool>(0)
                })
                .optional()?
                .unwrap_or(false)
            {
                added.push(reader);
            }
        }
        Ok((prior, added))
    })?;
    let first = prior.is_none();
    if added.is_empty()
        && let Some(header) = prior
    {
        committed();
        return Ok(Published { header, first: false });
    }
    let (ended_at, settles_queue, imported) = initial
        .as_ref()
        .map(|initial| (initial.ended_at, initial.settles_queue, initial.imported))
        .unwrap_or((None, false, None));
    let accounting = initial
        .as_ref()
        .and_then(|initial| initial.usage.as_ref())
        .map(|usage| (usage.ledger.clone(), usage.key.clone()));
    let (content, staged) = if first {
        let initial = initial.ok_or_else(|| io::Error::other("cannot add readers to a missing terminal"))?;
        let end = Content { end: initial.end, usage: initial.usage };
        let bytes = serde_json::to_vec(&end)?;
        if bytes.len() <= INLINE {
            (Some(End::Inline(Box::new(end))), None)
        } else {
            let staged =
                record_payloads::stage(&database, &scope, bytes, Priority::Normal).map_err(io::Error::other)?;
            (Some(End::Payload(staged.reference.clone())), Some(staged))
        }
    } else {
        (None, None)
    };
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
    let token = format!("{:x}", u128::from_be_bytes(random));
    let pin = Arc::new(Pin(token.clone()));
    LIVE.lock().expect("terminal reader pins").insert(token.clone(), Arc::downgrade(&pin));
    let count = added.len() as u64;
    let revision = if added.len() > 16 {
        let (at, token) = (turn.clone(), token.clone());
        let revision =
            write(&directory, 512, order.clone(), pin.clone(), move |rows| allocate(rows, &at, &token, count))?;
        for batch in added.chunks(16) {
            let (at, batch) = (turn.clone(), batch.to_vec());
            let bytes = reader_bytes(&batch);
            write(&directory, bytes, order.clone(), pin.clone(), move |rows| {
                stage_readers(rows, &at, revision, &batch)
            })?;
        }
        Some(revision)
    } else {
        None
    };
    let kept = staged.clone();
    let at = turn.clone();
    let small = if revision.is_none() { added.clone() } else { Vec::new() };
    let header = records::write_then_at(
        &directory,
        Priority::Normal,
        INLINE + 1024 + reader_bytes(&small),
        move |rows| {
            let revision = match revision {
                Some(revision) => revision,
                None => {
                    let revision = allocate(rows, &at, &token, count)?;
                    stage_readers(rows, &at, revision, &small)?;
                    revision
                }
            };
            if rows.connection().execute("UPDATE terminal_reader_revisions SET ready=1 WHERE conversation=?1 AND turn=?2 AND revision=?3 AND ready=0 AND written=expected",(rows.conversation(),&at,revision))?!=1 {return Err(invalid("terminal readers were not completely staged"));}
            let mut header = match rows.get::<Header>(FAMILY, &at)? {
                Some(header) => header,
                None => {
                    let previous = rows.get::<u64>("terminal-counter", "order")?.unwrap_or(0);
                    let queue_order = match imported {
                        Some((_, order)) => order,
                        None => previous.checked_add(1).ok_or_else(|| invalid("terminal order exhausted"))?,
                    };
                    if queue_order > previous {
                        rows.put("terminal-counter", "order", &queue_order)?;
                    }
                    let end = content.ok_or_else(|| corrupt("terminal header disappeared during publication"))?;
                    if let End::Payload(reference) = &end {
                        record_payloads::link(rows.connection(), rows.conversation(), CONTENTS, &at, Some(reference))?;
                    }
                    Header {
                        turn: at.clone(),
                        ended_at,
                        at: imported.map(|(at, _)| at).unwrap_or_else(Timestamp::now),
                        end,
                        settles_queue,
                        queue_order,
                        reader_revision: 0,
                        reader_count: 0,
                        effects_complete: false,
                    }
                }
            };
            if header.reader_revision >= revision {
                return Err(invalid("terminal reader publication moved backwards"));
            }
            header.reader_revision = revision;
            header.reader_count =
                header.reader_count.checked_add(count).ok_or_else(|| invalid("terminal reader count exhausted"))?;
            rows.put(FAMILY, &at, &header)?;
            if first && let Some((ledger, identity)) = accounting {
                rows.connection().execute(
                    "INSERT INTO terminal_usage(conversation,turn,ledger,identity,complete) VALUES(?1,?2,?3,?4,0)",
                    (rows.conversation(), &at, ledger, identity),
                )?;
            }
            rows.connection().execute(
                "INSERT OR IGNORE INTO terminal_work(conversation,turn) VALUES(?1,?2)",
                (rows.conversation(), &at),
            )?;
            if let Some(staged) = staged {
                record_payloads::finish(rows.connection(), rows.conversation(), &staged)?;
            }
            Ok(header)
        },
        move |_| {
            drop(kept);
            drop(pin);
            drop(order);
            committed();
        },
    )?;
    Ok(Published { header, first })
}

fn reader_bytes(readers: &[Address]) -> usize {
    readers.iter().map(|reader| reader.machine.as_str().len() + reader.resident.as_str().len() + 128).sum()
}
fn allocate(rows: &records::Rows<'_>, turn: &str, token: &str, count: u64) -> Result<i64> {
    let expected = i64::try_from(count).map_err(|_| invalid("terminal reader count exhausted"))?;
    let db = rows.connection();
    db.execute(
        "INSERT OR IGNORE INTO terminal_reader_heads(conversation,turn,allocated) VALUES(?1,?2,0)",
        (rows.conversation(), turn),
    )?;
    let revision=db.query_row("UPDATE terminal_reader_heads SET allocated=allocated+1 WHERE conversation=?1 AND turn=?2 AND allocated<9223372036854775807 RETURNING allocated",(rows.conversation(),turn),|row|row.get::<_,i64>(0)).optional()?.ok_or_else(||invalid("terminal reader revision exhausted"))?;
    db.execute("INSERT INTO terminal_reader_revisions(conversation,turn,revision,token,expected,written,ready) VALUES(?1,?2,?3,?4,?5,0,0)",(rows.conversation(),turn,revision,token,expected))?;
    Ok(revision)
}
fn stage_readers(rows: &records::Rows<'_>, turn: &str, revision: i64, readers: &[Address]) -> Result<()> {
    let db = rows.connection();
    let mut insert=db.prepare_cached("INSERT INTO terminal_readers(conversation,turn,machine,resident,revision) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(conversation,turn,machine,resident) DO UPDATE SET revision=excluded.revision,owed=0 WHERE NOT EXISTS(SELECT 1 FROM terminal_reader_revisions r WHERE r.conversation=terminal_readers.conversation AND r.turn=terminal_readers.turn AND r.revision=terminal_readers.revision AND r.ready=1)")?;
    for reader in readers {
        if insert.execute((rows.conversation(), turn, reader.machine.as_str(), reader.resident.as_str(), revision))?
            != 1
        {
            return Err(invalid("terminal reader changed outside its publication order"));
        }
    }
    if db.execute("UPDATE terminal_reader_revisions SET written=written+?4 WHERE conversation=?1 AND turn=?2 AND revision=?3 AND ready=0",(rows.conversation(),turn,revision,readers.len() as i64))?!=1 {
        return Err(invalid("terminal reader revision is unavailable"));
    }
    Ok(())
}

pub(crate) fn header(rows: &records::Rows<'_>, turn: &str) -> Result<Option<Header>> {
    let Some(header) = rows.get::<Header>(FAMILY, turn)? else { return Ok(None) };
    if header.turn != turn || header.reader_revision < 1 {
        return Err(corrupt("terminal header identity is invalid"));
    }
    let ready: bool = rows.connection().query_row(
        "SELECT EXISTS(SELECT 1 FROM terminal_reader_revisions WHERE conversation=?1 AND turn=?2 AND revision=?3 AND ready=1)",
        (rows.conversation(), turn, header.reader_revision), |row| row.get(0),
    )?;
    if !ready {
        return Err(corrupt("terminal reader publication is missing"));
    }
    Ok(Some(header))
}
pub(crate) fn content(rows: &records::Rows<'_>, header: &Header) -> Result<Content> {
    match &header.end {
        End::Inline(end) => Ok(end.as_ref().clone()),
        End::Payload(reference) => {
            if record_payloads::root(rows.connection(), rows.conversation(), CONTENTS, &header.turn)?.as_ref()
                != Some(reference)
            {
                return Err(corrupt("terminal contents lost their owning root"));
            }
            records::decode(&record_payloads::read(rows.connection(), rows.conversation(), reference)?)
        }
    }
}
pub(crate) fn end(rows: &records::Rows<'_>, header: &Header) -> Result<TurnEnd> {
    Ok(content(rows, header)?.end)
}

pub(crate) fn read(directory: &ConversationDir, turn: &str) -> io::Result<Option<(Header, TurnEnd, Vec<Address>)>> {
    records::read_background(directory, |rows| {
        let Some(header) = header(rows, turn)? else { return Ok(None) };
        let end = end(rows, &header)?;
        let mut readers = Vec::new();
        let mut after = (String::new(), String::new());
        loop {
            let mut query=rows.connection().prepare_cached("SELECT m.machine,m.resident FROM terminal_readers m JOIN terminal_reader_revisions r ON r.conversation=m.conversation AND r.turn=m.turn AND r.revision=m.revision WHERE m.conversation=?1 AND m.turn=?2 AND m.revision<=?3 AND r.ready=1 AND (m.machine,m.resident)>(?4,?5) ORDER BY m.machine,m.resident LIMIT 128")?;
            let page = query
                .query_map(params![rows.conversation(), turn, header.reader_revision, after.0, after.1], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.clone();
            for (machine, resident) in page {
                readers.push(Address {
                    machine: machine.parse().map_err(|_| corrupt("invalid terminal reader machine"))?,
                    resident: resident.parse().map_err(|_| corrupt("invalid terminal reader resident"))?,
                });
            }
        }
        if readers.len() as u64 != header.reader_count {
            return Err(corrupt("terminal reader membership is incomplete"));
        }
        Ok(Some((header, end, readers)))
    })
}

pub(crate) fn collect(database: &Database, mut report: impl FnMut(&str, &Error)) -> Result<()> {
    let mut after = (String::new(), String::new(), 0i64);
    loop {
        let revisions=database.read_background_now(|db| {
            let mut query=db.prepare_cached("SELECT conversation,turn,revision,token FROM terminal_reader_revisions INDEXED BY terminal_reader_unready WHERE ready=0 AND (conversation,turn,revision)>(?1,?2,?3) ORDER BY conversation,turn,revision LIMIT 16")?;
            Ok(query.query_map((&after.0,&after.1,after.2),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,String>(3)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = revisions.last() else {
            return Ok(());
        };
        after = (last.0.clone(), last.1.clone(), last.2);
        for (scope, turn, revision, token) in revisions {
            let active =
                LIVE.lock().expect("terminal reader pins").get(&token).is_some_and(|pin| pin.strong_count() != 0);
            if !active && let Err(error) = collect_revision(database, &scope, &turn, revision) {
                report(&scope, &error);
            }
        }
    }
}
fn collect_revision(database: &Database, scope: &str, turn: &str, revision: i64) -> Result<()> {
    loop {
        let (scope, turn) = (scope.to_string(), turn.to_string());
        let count=database.write_now_at(Priority::Background,8192,move|db| {
            let mut query=db.prepare_cached("SELECT machine,resident FROM terminal_readers WHERE conversation=?1 AND turn=?2 AND revision=?3 AND EXISTS(SELECT 1 FROM terminal_reader_revisions WHERE conversation=?1 AND turn=?2 AND revision=?3 AND ready=0) ORDER BY machine,resident LIMIT 16")?;
            let readers=query.query_map((&scope,&turn,revision),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            let mut delete=db.prepare_cached("DELETE FROM terminal_readers WHERE conversation=?1 AND turn=?2 AND machine=?3 AND resident=?4 AND revision=?5 AND EXISTS(SELECT 1 FROM terminal_reader_revisions WHERE conversation=?1 AND turn=?2 AND revision=?5 AND ready=0)")?;
            for (machine,resident) in &readers {delete.execute((&scope,&turn,machine,resident,revision))?;}
            Ok(readers.len())
        })?;
        if count == 0 {
            break;
        }
    }
    let (scope, turn) = (scope.to_string(), turn.to_string());
    database.write_now_at(Priority::Background,256,move|db| {
        db.execute("DELETE FROM terminal_reader_revisions WHERE conversation=?1 AND turn=?2 AND revision=?3 AND ready=0 AND NOT EXISTS(SELECT 1 FROM terminal_readers WHERE conversation=?1 AND turn=?2 AND revision=?3)",(&scope,&turn,revision))?;Ok(())
    })
}

/// Small durable work keys survive a lost publication reply. Consumers settle
/// views and create answer debts before deleting this marker in their commit.
pub(crate) fn pending(database: &Database, after: &(String, String)) -> Result<Vec<(String, String)>> {
    database.read_now(|db| {
        let mut query=db.prepare_cached("SELECT conversation,turn FROM terminal_work WHERE (conversation,turn)>(?1,?2) ORDER BY conversation,turn LIMIT 32")?;
        Ok(query.query_map((&after.0,&after.1),|row|Ok((row.get(0)?,row.get(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
    })
}

/// The partial index excludes paid prefixes independently of SQLite's estimates.
pub(crate) fn unowed(rows: &records::Rows<'_>, turn: &str, limit: usize) -> Result<Vec<Address>> {
    let mut query=rows.connection().prepare_cached("SELECT m.machine,m.resident FROM terminal_readers m INDEXED BY terminal_reader_unowed JOIN terminal_reader_revisions r ON r.conversation=m.conversation AND r.turn=m.turn AND r.revision=m.revision WHERE m.conversation=?1 AND m.turn=?2 AND m.owed=0 AND r.ready=1 ORDER BY m.machine,m.resident LIMIT ?3")?;
    query
        .query_map((rows.conversation(), turn, limit.clamp(1, 16) as i64), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .map(|row| {
            let (machine, resident) = row?;
            Ok(Address {
                machine: machine.parse().map_err(|_| corrupt("invalid terminal reader machine"))?,
                resident: resident.parse().map_err(|_| corrupt("invalid terminal reader resident"))?,
            })
        })
        .collect()
}

/// Called in the same writer transaction as the idempotent answer debt.
pub(crate) fn owed(rows: &records::Rows<'_>, turn: &str, reader: &Address) -> Result<()> {
    rows.connection().execute("UPDATE terminal_readers SET owed=1 WHERE conversation=?1 AND turn=?2 AND machine=?3 AND resident=?4 AND EXISTS(SELECT 1 FROM terminal_reader_revisions r WHERE r.conversation=terminal_readers.conversation AND r.turn=terminal_readers.turn AND r.revision=terminal_readers.revision AND r.ready=1)",(rows.conversation(),turn,reader.machine.as_str(),reader.resident.as_str()))?;
    Ok(())
}

pub(crate) fn effects_complete(rows: &records::Rows<'_>, turn: &str) -> Result<()> {
    let mut header = rows.get::<Header>(FAMILY, turn)?.ok_or_else(|| corrupt("terminal header is missing"))?;
    if !header.effects_complete {
        header.effects_complete = true;
        rows.put(FAMILY, turn, &header)?;
    }
    Ok(())
}

pub(crate) fn settle_work(rows: &records::Rows<'_>, turn: &str) -> Result<bool> {
    let header = rows.get::<Header>(FAMILY, turn)?.ok_or_else(|| corrupt("terminal header is missing"))?;
    if !header.effects_complete || !unowed(rows, turn, 1)?.is_empty() {
        return Ok(false);
    }
    rows.connection()
        .execute("DELETE FROM terminal_work WHERE conversation=?1 AND turn=?2", (rows.conversation(), turn))?;
    Ok(true)
}

/// `None` starts before every turn, including the legacy empty turn identity.
pub(crate) fn pending_turns(directory: &ConversationDir, after: Option<&str>) -> io::Result<Vec<String>> {
    records::read(directory, |rows| {
        let mut query = rows.connection().prepare_cached(
            "SELECT turn FROM terminal_work WHERE conversation=?1 AND (?2 IS NULL OR turn>?2) ORDER BY turn LIMIT 32",
        )?;
        Ok(query
            .query_map((rows.conversation(), after), |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })
}
