//! Remote running jobs and unconfirmed whole answers share an 8 MiB / 16,384-row
//! budget per conversation, with 64 KiB per row. Pending routes cannot be folded
//! or expired; confirmation deletes them. Legacy excess may drain but cannot grow.
//! Command text here is a 4 KiB display summary; its full script stays at its executor.
use super::Elsewhere;
use crate::records::{self, Source};
use fastctx_storage::{Error, Priority, rusqlite::OptionalExtension};
use fastctx_types::layout::ConversationDir;
use serde::{Deserialize, Serialize};
use std::io::{self, Read};

pub(super) const RUNNING: &str = "remote-running";
pub(super) const DISPATCHED: &str = "remote-whole-answers";
const COUNTS: &str = "remote-route-budget";
const ROWS: u64 = 16_384;
const BYTES: u64 = 8 * 1024 * 1024;
const ROW_BYTES: usize = 64 * 1024;
const SOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Default, Serialize, Deserialize)]
struct Budget {
    rows: u64,
    bytes: u64,
}

pub(super) fn summarize(command: &str) -> String {
    if command.len() <= 4096 {
        return command.to_string();
    }
    fastctx_platform::readings::counter("node.remote_routes.summaries_clipped").add(1);
    let mut end = 3900;
    while !command.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} … (full command remains on the executor)", &command[..end])
}

pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    let folder = directory.elsewhere();
    records::ensure_each(directory, RUNNING, move |push| {
        let entries = match fastctx_platform::disk::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            crate::record_migration::file(&path, push, |push| {
                let mut bytes = Vec::new();
                fastctx_platform::disk::open(&path)?.take(SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > SOURCE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "legacy remote job exceeds 64 MiB; source preserved",
                    ));
                }
                let mut value: Elsewhere = serde_json::from_slice(&bytes)?;
                value.command = summarize(&value.command);
                let body = encode(&value)?;
                let key = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| crate::record_migration::errors::invalid("remote job identity is not UTF-8"))?;
                if key != super::elsewhere_key(&value.machine.name, &value.job) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "remote job file identity differs from its body",
                    ));
                }
                push(Source::new(key.into(), path.clone(), body)?)
            })?;
        }
        Ok(())
    })?;
    if records::read(directory, |rows| rows.exists(COUNTS, "current"))? {
        return Ok(());
    }
    let budget = records::read_background(directory, |rows| {
        let (count, bytes) = rows.connection().query_row(
            "SELECT count(*),coalesce(sum(length(body)),0) FROM conversation_records WHERE conversation=?1 AND family IN (?2,?3)",
            (rows.conversation(), RUNNING, DISPATCHED), |row| Ok((row.get::<_,i64>(0)?, row.get::<_,i64>(1)?))
        )?;
        Ok(Budget { rows: count as u64, bytes: bytes as u64 })
    })?;
    records::write_at(directory, Priority::Background, 64, move |rows| {
        if !rows.exists(COUNTS, "current")? {
            rows.put(COUNTS, "current", &budget)?;
        }
        Ok(())
    })
}

fn full() -> io::Error {
    fastctx_platform::readings::counter("node.remote_routes.at_capacity").add(1);
    io::Error::new(
        io::ErrorKind::StorageFull,
        "this conversation's remote completion records are full; pending confirmations must finish first",
    )
}

pub(super) fn available(directory: &ConversationDir) -> io::Result<()> {
    prepare(directory)?;
    let budget = records::read(directory, |rows| rows.get::<Budget>(COUNTS, "current"))?.unwrap_or_default();
    if budget.rows >= ROWS || budget.bytes >= BYTES { Err(full()) } else { Ok(()) }
}

fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > ROW_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "remote completion record exceeds 64 KiB"));
    }
    Ok(bytes)
}

pub(super) fn put(
    directory: &ConversationDir,
    family: &'static str,
    key: String,
    value: &impl Serialize,
) -> io::Result<()> {
    let body = encode(value)?;
    prepare(directory)?;
    records::write_at(directory, Priority::Interactive, body.len() + key.len() + 128, move |rows| {
        let old: Option<i64> = rows
            .connection()
            .query_row(
                "SELECT length(body) FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
                (rows.conversation(), family, &key),
                |row| row.get(0),
            )
            .optional()?;
        let prior = rows.get::<Budget>(COUNTS, "current")?.unwrap_or_default();
        let next = Budget {
            rows: prior
                .rows
                .checked_add(u64::from(old.is_none()))
                .ok_or_else(|| Error::new("remote route count exhausted"))?,
            bytes: prior
                .bytes
                .checked_sub(old.unwrap_or(0) as u64)
                .ok_or_else(|| Error::new("remote route budget is inconsistent"))?
                .checked_add(body.len() as u64)
                .ok_or_else(|| Error::new("remote route bytes exhausted"))?,
        };
        if (next.rows > ROWS && next.rows > prior.rows) || (next.bytes > BYTES && next.bytes > prior.bytes) {
            return Err(Error::new(full().to_string()));
        }
        rows.put_raw(family, &key, &body)?;
        rows.delete("remote-route-retries", &key)?;
        rows.put(COUNTS, "current", &next)
    })
}

pub(super) fn delete(
    directory: &ConversationDir,
    family: &'static str,
    key: &str,
    expected: Option<&[u8]>,
) -> io::Result<bool> {
    prepare(directory)?;
    let (key, expected) = (key.to_string(), expected.map(<[u8]>::to_vec));
    records::write_at(
        directory,
        Priority::Interactive,
        128 + key.len() + expected.as_ref().map_or(0, Vec::len),
        move |rows| {
            let bytes: Option<i64> = rows.connection().query_row("SELECT length(body) FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3 AND (?4 IS NULL OR body=?4)", (rows.conversation(), family, &key, &expected), |row| row.get(0)).optional()?;
            let Some(bytes) = bytes else { return Ok(false) };
            let mut budget = rows.get::<Budget>(COUNTS, "current")?.unwrap_or_default();
            budget.rows = budget.rows.checked_sub(1).ok_or_else(|| Error::new("remote route count is inconsistent"))?;
            budget.bytes = budget
                .bytes
                .checked_sub(bytes as u64)
                .ok_or_else(|| Error::new("remote route bytes are inconsistent"))?;
            rows.delete(family, &key)?;
            rows.delete("remote-route-retries", &key)?;
            rows.put(COUNTS, "current", &budget)?;
            Ok(true)
        },
    )
}

pub(super) fn running(directory: &ConversationDir) -> io::Result<Vec<Elsewhere>> {
    prepare(directory)?;
    records::read(directory, |rows| {
        let budget = rows.get::<Budget>(COUNTS, "current")?.unwrap_or_default();
        if budget.rows > ROWS || budget.bytes > BYTES {
            return Err(Error::new(full().to_string()));
        }
        rows.all(RUNNING)
    })
}

pub(super) fn has_pending(directory: &ConversationDir) -> io::Result<bool> {
    prepare(directory)?;
    records::read(directory, |rows| {
        Ok(rows.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_records WHERE conversation=?1 AND family IN (?2,?3))",
            (rows.conversation(), RUNNING, DISPATCHED),
            |row| row.get(0),
        )?)
    })
}

pub(super) fn page(directory: &ConversationDir, after: &str) -> io::Result<Vec<(String, Elsewhere)>> {
    prepare(directory)?;
    records::read_background(directory, |rows| rows.key_page(RUNNING, after, 128))
}

pub(super) fn dispatches(directory: &ConversationDir, prefix: &str) -> io::Result<Vec<(String, super::Dispatch)>> {
    prepare(directory)?;
    let end = format!("{prefix}~");
    records::read(directory, |rows| {
        let mut query = rows.connection().prepare_cached("SELECT identity,body FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity>=?3 AND identity<?4 ORDER BY identity LIMIT 2")?;
        query
            .query_map((rows.conversation(), DISPATCHED, prefix, &end), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?
            .map(|row| {
                let (key, body) = row?;
                Ok((key, records::decode(&body)?))
            })
            .collect()
    })
}

pub(super) fn dispatched_job(
    directory: &ConversationDir,
    job: &fastctx_types::JobId,
) -> io::Result<Vec<super::Dispatch>> {
    prepare(directory)?;
    records::read(directory, |rows| {
        let mut query = rows.connection().prepare_cached("SELECT body FROM conversation_records WHERE conversation=?1 AND family=?2 AND json_extract(body,'$.job')=?3 LIMIT 2")?;
        query
            .query_map((rows.conversation(), DISPATCHED, job.as_str()), |row| row.get::<_, Vec<u8>>(0))?
            .map(|row| records::decode(&row?))
            .collect()
    })
}

pub(super) fn retry(directory: &ConversationDir, key: &str, reason: &str) -> io::Result<bool> {
    prepare(directory)?;
    let mut end = reason.len().min(3900);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let reason =
        if end < reason.len() { format!("{} … (diagnostic shortened)", &reason[..end]) } else { reason.to_string() };
    let key = key.to_string();
    records::write_at(directory, Priority::Background, reason.len() + 64, move |rows| {
        if !rows.exists(RUNNING, &key)? {
            return Ok(false);
        }
        if rows.get::<String>("remote-route-retries", &key)?.as_ref() == Some(&reason) {
            return Ok(false);
        }
        rows.put("remote-route-retries", &key, &reason)?;
        Ok(true)
    })
}
