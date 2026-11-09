//! Consume durable executor journals in bounded batches. Contributions and their
//! cursors commit together; segment retention waits for this independent reader.
//! One thread reads a journal at a time, and no other waits for it: one asking
//! meanwhile marks the journal to be read again, which the reader does before
//! it stops, with the job's record as it stands then.

use super::*;
use fastctx_storage::{
    Priority,
    rusqlite::{Connection, OptionalExtension},
};
use fastctx_types::run_facts::RunStep;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::{io, path::Path};

/// The journals being read now, by path, each marked when its job was asked
/// to be observed again while it was read.
static READING: LazyLock<Mutex<HashMap<PathBuf, bool>>> = LazyLock::new(Default::default);

fn reading() -> MutexGuard<'static, HashMap<PathBuf, bool>> {
    READING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A journal's reader: a reader that unwinds lets go of the journal, so the
/// next observation reads it.
struct Reader<'a> {
    path: &'a Path,
    done: bool,
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        if !self.done {
            reading().remove(self.path);
        }
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub(crate) struct Cursor {
    pub steps_segment: u64,
    pub steps_offset: u64,
    pub writes_offset: u64,
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    sql_writes: bool,
    looked: i64,
    copied: i64,
}

fn cursor(db: &Connection, deed: &str) -> fastctx_storage::Result<Cursor> {
    let body: Option<Vec<u8>> =
        db.query_row("SELECT body FROM book_cursors WHERE deed=?1", [deed], |row| row.get(0)).optional()?;
    body.map(|body| crate::records::decode(&body)).transpose().map(|row| row.unwrap_or_default())
}

/// How far into a job's step journal the book has read, below which the
/// journal may let segments go. A damaged book reads nothing until it is
/// mended, so it holds none back: a long job's steps would otherwise fill
/// their budget and stop being kept, and the book shows the gap once it
/// reads again. Collecting the whole job still waits for it (`consumed`).
pub(crate) fn segment(node: &Node, record: &JobRecord) -> io::Result<u64> {
    if node.book_database.fault().is_some() {
        return Ok(u64::MAX);
    }
    node.book_database.read_now(|db| Ok(cursor(db, &deed(record))?.steps_segment)).map_err(io::Error::other)
}

/// Whole-job collection obeys the same independent consumer as segment retention.
/// A failed database is never evidence that its source has been consumed.
pub(crate) fn consumed(node: &Node, record: &JobRecord) -> io::Result<bool> {
    if node.book_database.fault().is_some() {
        return Ok(false);
    }
    node.book_database
        .read_now(|db| {
            let key = deed(record);
            Ok(super::frozen(db, &key)?
                || db.query_row("SELECT EXISTS(SELECT 1 FROM book_complete WHERE deed=?1)", [&key], |row| {
                    row.get::<_, bool>(0)
                })?)
        })
        .map_err(io::Error::other)
}

pub(crate) fn observe(node: &Node, path: &Path) -> io::Result<()> {
    if let Some(error) = node.book_database.fault() {
        return Err(io::Error::other(error.clone()));
    }
    {
        let mut reading = reading();
        if let Some(again) = reading.get_mut(path) {
            *again = true;
            return Ok(());
        }
        reading.insert(path.to_path_buf(), false);
    }
    let mut reader = Reader { path, done: false };
    loop {
        let read = current(node, path);
        // Whether to read again and letting go are decided together, so an
        // observation asked meanwhile is never left unread, nor dropped with
        // a read that failed before it.
        let mut reading = reading();
        if !reading.get_mut(path).is_some_and(std::mem::take) {
            reading.remove(path);
            reader.done = true;
            return read;
        }
    }
}

/// Reads the journal with the job's record as it stands now. A record an
/// observer took earlier can be older than one already read, and reading
/// with it would take back the end the book had marked complete.
fn current(node: &Node, path: &Path) -> io::Result<()> {
    match fastctx_hands::run::record::read(path) {
        Ok(record) => read(node, path, &record),
        // A job's files go only once the book holds all of it.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Reads what `record`'s journal holds past the book's cursor into the book.
fn read(node: &Node, path: &Path, record: &JobRecord) -> io::Result<()> {
    let key = deed(record);
    super::migrate::deed(node, &key)?;
    if node.book_database.read_now(|db| super::frozen(db, &key)).map_err(io::Error::other)? {
        return Ok(());
    }
    let row = super::row(node, record, &[], &[]);
    node.book_database
        .write_now_at(Priority::Background, row.script.len() + 512, move |db| {
            db.execute("DELETE FROM book_complete WHERE deed=?1", [&row.deed])?;
            super::keep(db, &row)
        })
        .map_err(io::Error::other)?;
    let mut position = node.book_database.read_now(|db| cursor(db, &key)).map_err(io::Error::other)?;
    if !position.initialized || !position.sql_writes {
        position.writes_offset = 0;
        position.sql_writes = true;
        crate::run_facts::repair_journal(path, true)?;
        position.initialized = true;
        let (key, saved) = (key.clone(), position.clone());
        node.book_database.write_now_at(Priority::Background,128,move |db| {
            db.execute("INSERT INTO book_cursors(deed,body) VALUES(?1,?2) ON CONFLICT(deed) DO UPDATE SET body=excluded.body",params![key,crate::records::encode(&saved)?])?;
            Ok(())
        }).map_err(io::Error::other)?;
    }
    loop {
        let (writes, steps) = crate::run_facts::book_since(path, &mut position)?;
        if writes.is_empty() && steps.is_empty() {
            if record.end.is_some() {
                let key = key.clone();
                node.book_database
                    .write_now_at(Priority::Background, key.len(), move |db| {
                        if !super::frozen(db, &key)? && super::kept(db, &key)? {
                            db.execute("INSERT OR IGNORE INTO book_complete(deed) VALUES(?1)", [key])?;
                        }
                        Ok(())
                    })
                    .map_err(io::Error::other)?;
            }
            return Ok(());
        }
        // The source cursor advances per page, while the SQL work is split into
        // small batches. A replay after an interrupted page is idempotent.
        let count = writes.len().max(steps.len()).div_ceil(16);
        for index in 0..count {
            let writes: Vec<_> = writes.iter().skip(index * 16).take(16).cloned().collect();
            let steps: Vec<_> = steps.iter().skip(index * 16).take(16).cloned().collect();
            let bytes = serde_json::to_vec(&(&writes, &steps))?.len();
            let (key, machine) = (key.clone(), node.machine());
            let final_cursor = (index + 1 == count).then(|| position.clone());
            node.book_database.write_now_at(Priority::Background,bytes,move |db| {
                if super::frozen(db,&key)? || !super::kept(db,&key)? {return Ok(());}
                let mut current=cursor(db,&key)?;
                for step in &steps {contribute(db,&key,&machine,step,&mut current)?;}
                for write in writes.iter().filter(|write|write.machine==machine) {
                    if db.execute("INSERT OR IGNORE INTO book_writes(deed,identity) VALUES(?1,?2)",(&key,&write.id))?==0 {continue;}
                    let how=match write.kind {ChangeKind::Added=>"added",ChangeKind::Modified=>"changed",ChangeKind::Deleted=>"removed"};
                    db.execute("INSERT OR REPLACE INTO book_touches(deed,path,how,added,removed,seen) VALUES(?1,?2,?3,?4,?5,0)",params![key,write.path,how,write.added,write.removed])?;
                }
                if let Some(final_cursor)=final_cursor {
                    current.steps_segment=final_cursor.steps_segment;
                    current.steps_offset=final_cursor.steps_offset;
                    current.writes_offset=final_cursor.writes_offset;
                    current.sql_writes=final_cursor.sql_writes;
                }
                db.execute("INSERT INTO book_cursors(deed,body) VALUES(?1,?2) ON CONFLICT(deed) DO UPDATE SET body=excluded.body",params![key,crate::records::encode(&current)?])?;
                db.execute("UPDATE book_deeds SET looked=max(looked,?2),copied=max(copied,?3) WHERE deed=?1",params![key,current.looked,current.copied])?;
                Ok(())
            }).map_err(io::Error::other)?;
        }
    }
}

fn contribute(
    db: &Connection,
    key: &str,
    machine: &fastctx_types::MachineName,
    step: &RunStep,
    counts: &mut Cursor,
) -> fastctx_storage::Result<()> {
    let prior: Option<(i64, i64, bool)> = db
        .query_row(
            "SELECT looked,copied,finished FROM book_steps WHERE deed=?1 AND identity=?2",
            (key, &step.id),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if prior.is_some_and(|(_, _, finished)| finished && step.ended.is_none()) {
        return Ok(());
    }
    let (looked, copied) = match &step.did {
        StepDid::View { paths, .. } => (paths.len() as i64, 0),
        StepDid::Copy { files, .. } => (0, *files as i64),
        _ => (0, 0),
    };
    let (old_looked, old_copied, _) = prior.unwrap_or_default();
    counts.looked += looked - old_looked;
    counts.copied += copied - old_copied;
    match &step.did {
        StepDid::On { to, .. } => {
            db.execute("INSERT OR IGNORE INTO book_visits(deed,machine) VALUES(?1,?2)", (key, to.as_str()))?;
        }
        StepDid::Copy { to, .. } if &to.machine == machine && prior.is_none() => {
            db.execute(
                "INSERT OR IGNORE INTO book_touches(deed,path,how,seen) VALUES(?1,?2,'copied',0)",
                (key, &to.path),
            )?;
        }
        _ => {}
    }
    db.execute("INSERT INTO book_steps(deed,identity,looked,copied,finished) VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(deed,identity) DO UPDATE SET looked=excluded.looked,copied=excluded.copied,finished=excluded.finished",params![key,step.id,looked,copied,step.ended.is_some()])?;
    Ok(())
}
