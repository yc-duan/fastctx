//! A damaged database set aside, and what can be read of it salvaged into a
//! new one. A durable marker records the move before it starts, so that a
//! start cut short finishes it rather than opening an empty database where
//! the old one was; a database is never rebuilt empty in silence.

use crate::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const FORMAT: &str = "fastctx-sqlite-quarantine-v1";
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Rows salvaged between commits of the new database.
const BATCH: usize = 1024;

fn side(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}
fn marker(path: &Path) -> PathBuf {
    side(path, ".unavailable")
}
fn damage_mark(path: &Path) -> PathBuf {
    side(path, ".damaged")
}
fn directory(path: &Path, id: &str) -> PathBuf {
    side(path, ".quarantine").join(id)
}

struct Intent {
    id: String,
    reason: String,
}
fn intent(path: &Path) -> Result<Option<Intent>> {
    let text = match std::fs::read_to_string(marker(path)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut lines = text.splitn(3, '\n');
    if lines.next() != Some(FORMAT) {
        return Err(Error::with_kind(ErrorKind::Invalid, "the database quarantine marker has an unknown format"));
    }
    let id = lines.next().unwrap_or_default();
    if id.is_empty() || id.len() > 96 || !id.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-') {
        return Err(Error::with_kind(ErrorKind::Invalid, "the database quarantine marker has an invalid identity"));
    }
    Ok(Some(Intent { id: id.into(), reason: lines.next().unwrap_or_default().into() }))
}

/// Notes that the open database was found damaged; its next open sets it
/// aside and salvages it.
pub(super) fn mark_damaged(path: &Path, reason: &str) -> Result<()> {
    match fastctx_platform::publish_new(&damage_mark(path), reason.as_bytes(), fastctx_platform::Permissions::Private) {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => Err(error.into()),
        _ => Ok(()),
    }
}

/// Before a database opens: finishes setting aside and salvaging one that an
/// earlier start began on, and does so now for one marked damaged or found
/// so by `inspect`. What was salvaged, when it was.
pub(super) fn prepare(path: &Path) -> Result<Option<String>> {
    if let Some(intent) = intent(path)? {
        // Cut short after the salvage was put in place: only its marks are
        // left to take away. Before it: what is still beside the database
        // goes where the rest went, and the salvage starts again.
        if let Some(report) = salvaged_from(path, &intent.id).unwrap_or(None) {
            finish(path)?;
            return Ok(Some(report));
        }
        quarantine(path, &Error::damaged(intent.reason.clone()))?;
        return salvage(path, &intent).map(Some);
    }
    let damage = match std::fs::read_to_string(damage_mark(path)) {
        Ok(reason) => Some(reason),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => inspect(path)?,
        Err(error) => return Err(error.into()),
    };
    let Some(reason) = damage else { return Ok(None) };
    quarantine(path, &Error::damaged(reason))?;
    let intent = intent(path)?.ok_or_else(|| Error::new("the quarantine marker disappeared"))?;
    salvage(path, &intent).map(Some)
}

/// The checks of an existing database that cost no more than its header and
/// its schema: what is wrong, if anything. The whole file is checked in the
/// background once it is open (`crate::integrity`). The file is read without
/// a writable connection, so a broken header cannot make SQLite discard its
/// write-ahead log.
fn inspect(path: &Path) -> Result<Option<String>> {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let length = file.metadata()?.len();
    if length == 0 {
        // Nothing was ever committed to an empty file without a log: a start
        // cut short as it created it. One with a log may hold commits.
        let log = match std::fs::metadata(side(path, "-wal")) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        return Ok((log > 0).then(|| "the database file is empty but its write-ahead log is not".to_string()));
    }
    let mut header = [0u8; 100];
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(Some("the database header is truncated".into()));
        }
        Err(error) => return Err(error.into()),
    }
    if &header[..16] != b"SQLite format 3\0" {
        return Ok(Some("the database header is invalid".into()));
    }
    let word = |at: usize| u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]);
    let page = match u16::from_be_bytes([header[16], header[17]]) {
        1 => 65_536,
        size => u64::from(size),
    };
    // The page count in the header counts only while its "valid for" number
    // matches the change counter.
    let pages = u64::from(word(28));
    if word(92) == word(24) && pages > 0 && length < page.saturating_mul(pages) {
        return Ok(Some(format!(
            "the database file is shorter ({length} bytes) than its {pages} pages of {page} bytes"
        )));
    }
    drop(file);
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    match connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get::<_, i64>(0)) {
        Ok(_) => Ok(None),
        Err(error) => {
            let error = Error::from(error);
            if error.is_database_damage() { Ok(Some(error.to_string())) } else { Err(error) }
        }
    }
}

fn quarantine(path: &Path, cause: &Error) -> Result<PathBuf> {
    let intent = match intent(path)? {
        Some(intent) => intent,
        None => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let id = format!("{now:x}-{:x}-{:x}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
            let intent = Intent { id, reason: cause.to_string() };
            fastctx_platform::ensure_private_dir(&directory(path, &intent.id))?;
            let body = format!("{FORMAT}\n{}\n{}", intent.id, intent.reason);
            match fastctx_platform::publish_new(&marker(path), body.as_bytes(), fastctx_platform::Permissions::Private)
            {
                Ok(()) => intent,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    self::intent(path)?.ok_or_else(|| Error::new("the quarantine marker disappeared"))?
                }
                Err(error) => return Err(error.into()),
            }
        }
    };
    let target = directory(path, &intent.id);
    fastctx_platform::ensure_private_dir(&target)?;
    for suffix in ["", "-wal", "-shm"] {
        let source = side(path, suffix);
        let destination = target.join(source.file_name().ok_or_else(|| Error::new("database has no filename"))?);
        if !source.try_exists()? {
            continue;
        }
        if destination.try_exists()? {
            return Err(Error::new("both a database file and its quarantine copy exist; neither was replaced"));
        }
        std::fs::rename(&source, &destination)?;
    }
    #[cfg(unix)]
    {
        std::fs::File::open(&target)?.sync_all()?;
        std::fs::File::open(path.parent().ok_or_else(|| Error::new("database has no parent"))?)?.sync_all()?;
    }
    Ok(target)
}

/// Builds a new database at `path` from what can be read of the one set
/// aside under `intent`: its schema, and each table's rows up to the first
/// it cannot read. The new database records which quarantine it came from,
/// so a start cut short after it was put in place does not do it again.
/// What was salvaged and what was lost, as a report also kept beside the
/// damaged files.
fn salvage(path: &Path, intent: &Intent) -> Result<String> {
    let kept =
        directory(path, &intent.id).join(path.file_name().ok_or_else(|| Error::new("database has no filename"))?);
    if path.try_exists()? {
        return Err(Error::new("a database stands where a damaged one was set aside, and it was not salvaged"));
    }
    let building = side(path, ".salvage");
    for suffix in ["", "-journal", "-wal", "-shm"] {
        match std::fs::remove_file(side(&building, suffix)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    let mut lines = vec![format!("salvaged after: {}", intent.reason)];
    {
        let mut target = rusqlite::Connection::open(&building)?;
        target.pragma_update(None, "journal_mode", "OFF")?;
        target.pragma_update(None, "synchronous", "OFF")?;
        // A database that cannot be read at all leaves an empty one: its
        // files stay set aside, and the report says nothing came over.
        let readable = rusqlite::Connection::open_with_flags(
            &kept,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .and_then(|source| source.busy_timeout(std::time::Duration::from_secs(5)).map(|()| source))
        .map_err(Error::from)
        .and_then(|source| schema(&source).map(|schema| (source, schema)));
        match readable {
            Err(error) => lines.push(format!("nothing could be read: {error}")),
            Ok((source, schema)) => {
                for entry in schema.iter().filter(|entry| entry.kind == "table") {
                    if let Err(error) = target.execute_batch(&entry.sql) {
                        lines.push(format!("table {}: not made again: {error}", entry.name));
                        continue;
                    }
                    match copy(&source, &mut target, entry) {
                        Ok(said) => lines.push(format!("table {}: {said}", entry.name)),
                        Err(error) => lines.push(format!("table {}: nothing came over: {error}", entry.name)),
                    }
                }
                for entry in schema.iter().filter(|entry| entry.kind != "table") {
                    if let Err(error) = target.execute_batch(&entry.sql) {
                        lines.push(format!("{} {}: not made again: {error}", entry.kind, entry.name));
                    }
                }
                sequences(&source, &target, &mut lines);
                let version: i64 = source.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap_or(0);
                target.pragma_update(None, "user_version", version)?;
            }
        }
        let report = lines.join("\n");
        target.execute_batch(
            "CREATE TABLE IF NOT EXISTS storage_salvaged (quarantine TEXT PRIMARY KEY, report TEXT NOT NULL)",
        )?;
        target.execute("INSERT OR REPLACE INTO storage_salvaged VALUES (?1, ?2)", (&intent.id, &report))?;
    }
    // Flushing needs a handle that may write, on Windows.
    std::fs::OpenOptions::new().write(true).open(&building)?.sync_all()?;
    std::fs::rename(&building, path)?;
    #[cfg(unix)]
    std::fs::File::open(path.parent().ok_or_else(|| Error::new("database has no parent"))?)?.sync_all()?;
    let report = lines.join("\n");
    if let Err(error) = std::fs::write(directory(path, &intent.id).join("salvaged.txt"), &report) {
        eprintln!("{}: keeping the salvage report beside the damaged files: {error}", path.display());
    }
    finish(path)?;
    Ok(report)
}

/// The salvage report of a database salvaged from quarantine `id`.
fn salvaged_from(path: &Path, id: &str) -> Result<Option<String>> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let made: bool = connection.query_row(
        "SELECT count(*) > 0 FROM sqlite_schema WHERE type = 'table' AND name = 'storage_salvaged'",
        [],
        |row| row.get(0),
    )?;
    if !made {
        return Ok(None);
    }
    use rusqlite::OptionalExtension;
    Ok(connection
        .query_row("SELECT report FROM storage_salvaged WHERE quarantine = ?1", [id], |row| row.get(0))
        .optional()?)
}

/// The marks of the damaged database go once its salvage stands: the damage
/// mark first, the quarantine marker that guards the salvage last.
fn finish(path: &Path) -> Result<()> {
    for mark in [damage_mark(path), marker(path)] {
        match std::fs::remove_file(&mark) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    Ok(())
}

struct Entry {
    kind: String,
    name: String,
    sql: String,
    rowid: bool,
}

/// The damaged database's tables first, then its indexes, triggers and
/// views, without SQLite's own.
fn schema(source: &rusqlite::Connection) -> Result<Vec<Entry>> {
    let mut statement = source.prepare(
        "SELECT type, name, sql FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
         ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, rowid",
    )?;
    let rows = statement.query_map([], |row| {
        let sql: String = row.get(2)?;
        let rowid = !sql.to_ascii_uppercase().contains("WITHOUT ROWID");
        Ok(Entry { kind: row.get(0)?, name: row.get(1)?, sql, rowid })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Copies one table's rows in their order until one cannot be read: how many
/// came over, and why the rest did not.
fn copy(source: &rusqlite::Connection, target: &mut rusqlite::Connection, entry: &Entry) -> Result<String> {
    let table = quoted(&entry.name);
    let columns: Vec<String> = {
        let mut statement = source.prepare(&format!("PRAGMA table_info({table})"))?;
        let names = statement.query_map([], |row| row.get::<_, String>(1))?;
        names.collect::<rusqlite::Result<_>>()?
    };
    let mut wanted: Vec<String> = columns.iter().map(|column| quoted(column)).collect();
    if entry.rowid {
        wanted.insert(0, "rowid".into());
    }
    let list = wanted.join(", ");
    let marks = (1..=wanted.len()).map(|at| format!("?{at}")).collect::<Vec<_>>().join(", ");
    let mut read = source.prepare(&format!("SELECT {list} FROM {table}"))?;
    let mut rows = read.query([])?;
    let (mut copied, mut refused) = (0u64, 0u64);
    let mut stopped = None;
    let mut transaction = target.transaction()?;
    let mut in_batch = 0;
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => {
                stopped = Some(error.to_string());
                break;
            }
        };
        let values =
            (0..wanted.len()).map(|at| row.get::<_, rusqlite::types::Value>(at)).collect::<rusqlite::Result<Vec<_>>>();
        let values = match values {
            Ok(values) => values,
            Err(error) => {
                stopped = Some(error.to_string());
                break;
            }
        };
        let inserted = transaction
            .prepare_cached(&format!("INSERT INTO {table} ({list}) VALUES ({marks})"))?
            .execute(rusqlite::params_from_iter(values));
        if inserted.is_ok() {
            copied += 1;
        } else {
            refused += 1;
        }
        in_batch += 1;
        if in_batch == BATCH {
            transaction.commit()?;
            transaction = target.transaction()?;
            in_batch = 0;
        }
    }
    transaction.commit()?;
    let mut said = format!("{copied} rows");
    if refused > 0 {
        said.push_str(&format!(", {refused} refused by the new table"));
    }
    if let Some(why) = stopped {
        said.push_str(&format!("; the rest could not be read: {why}"));
    }
    Ok(said)
}

/// The next numbers of AUTOINCREMENT tables, so that numbers given out
/// before the damage are not given out again.
fn sequences(source: &rusqlite::Connection, target: &rusqlite::Connection, lines: &mut Vec<String>) {
    let read = || -> rusqlite::Result<Vec<(String, i64)>> {
        let mut statement = source.prepare("SELECT name, seq FROM sqlite_sequence")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    };
    let Ok(sequences) = read() else { return };
    for (name, seq) in sequences {
        let kept = target
            .execute("UPDATE sqlite_sequence SET seq = max(seq, ?2) WHERE name = ?1", (&name, seq))
            .and_then(|changed| {
                if changed == 0 {
                    target.execute("INSERT INTO sqlite_sequence (name, seq) VALUES (?1, ?2)", (&name, seq))
                } else {
                    Ok(changed)
                }
            });
        if let Err(error) = kept {
            lines.push(format!("the next number of {name} was not kept: {error}"));
        }
    }
}
