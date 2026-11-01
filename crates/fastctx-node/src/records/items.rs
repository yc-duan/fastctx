//! Bounded queue items and singleton metadata with legacy logical path identities.
//! These are not filesystem operations: callers explicitly opt a node-owned
//! family into SQL. Host protocols, outputs and attachment bodies never use it.

use super::*;
use std::path::{Path, PathBuf};

pub(crate) fn prepare(path: &Path) -> io::Result<(ConversationDir, String, String)> {
    let (directory, family, key, folder) = crate::record_database::item(path)?;
    let group = family.clone();
    ensure_each(&directory, &family, move |push| {
        for primary in [true, false] {
            let entries = match fastctx_platform::disk::read_dir(&folder) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| io::Error::other("record item identity is not UTF-8"))?;
                if name.starts_with('.') || name.ends_with(".lock") {
                    continue;
                }
                let path = entry.path();
                if path.extension().is_some_and(|extension| extension == "json") != primary {
                    continue;
                }
                crate::record_migration::file(&path, push, |push| {
                    if crate::record_database::rejected_origin(&path) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "the original queue envelope conflicts with the existing conversation record",
                        ));
                    }
                    let bytes = fastctx_platform::disk::read(&path)?;
                    super::item_legacy::validate(&group, &name, &bytes)?;
                    let category = path.extension().and_then(|extension| extension.to_str()).unwrap_or("").to_string();
                    let order = if category == "json"
                        && (group.ends_with("/received") || group.ends_with("/received-controls"))
                    {
                        let value = serde_json::from_slice::<serde_json::Value>(&bytes)?;
                        let sequence = value.get("order").and_then(serde_json::Value::as_u64).unwrap_or(0);
                        let at = value
                            .pointer("/delivery/at")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|at| at.parse().ok());
                        Some(order_key(sequence, at))
                    } else {
                        None
                    };
                    let terminal = matches!(category.as_str(), "done" | "taken" | "stopped");
                    let mut source = Source::new(name.clone(), path.clone(), bytes)?.categorized(category);
                    if terminal {
                        let expires = source.modified() + jiff::SignedDuration::from_hours(30 * 24);
                        source = source.expiring(expires);
                    }
                    if let Some(order) = order {
                        source = source.ordered_bytes(order);
                    }
                    push(source)
                })?;
            }
        }
        Ok(())
    })?;
    Ok((directory, family, key))
}

pub(crate) fn read(path: &Path) -> io::Result<Vec<u8>> {
    let (directory, family, key) = prepare(path)?;
    super::read(&directory, |rows| rows.get_raw(&family, &key))?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no record item {}", path.display())))
}
pub(crate) fn get<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
pub(crate) fn exists(path: &Path) -> io::Result<bool> {
    let (directory, family, key) = prepare(path)?;
    super::read(&directory, |rows| Ok(rows.get_raw(&family, &key)?.is_some()))
}
pub(crate) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let bytes = bytes.to_vec();
    change(path, bytes.len(), move |rows, family, key| {
        rows.put_raw(family, key, &bytes)?;
        if Path::new(key)
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| matches!(value, "done" | "taken" | "stopped"))
        {
            let now = jiff::Timestamp::now();
            rows.expire(family, key, now + jiff::SignedDuration::from_hours(30 * 24))?;
            rows.connection().execute(
                "UPDATE conversation_records SET modified=?4 WHERE conversation=?1 AND family=?2 AND identity=?3",
                (rows.conversation(), family, key, now.to_string()),
            )?;
        }
        Ok(())
    })
}
pub(crate) fn create(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let bytes = bytes.to_vec();
    let created = change(path, bytes.len(), move |rows, family, key| {
        if rows.get_raw(family, key)?.is_some() {
            return Ok(false);
        }
        rows.put_raw(family, key, &bytes)?;
        Ok(true)
    })?;
    if created { Ok(()) } else { Err(io::Error::new(io::ErrorKind::AlreadyExists, "record item already exists")) }
}
pub(crate) fn keep(path: &Path, value: &impl Serialize) -> io::Result<()> {
    write(path, &serde_json::to_vec(value)?)
}
pub(crate) fn delete(path: &Path) -> io::Result<()> {
    if change(path, 64, |rows, family, key| rows.delete(family, key))? {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::NotFound, "record item was already deleted"))
    }
}
pub(crate) fn delete_if_exists(path: &Path) -> io::Result<()> {
    change(path, 64, |rows, family, key| rows.delete(family, key).map(|_| ()))
}
pub(crate) fn modified(path: &Path) -> io::Result<std::time::SystemTime> {
    let (directory, family, key) = prepare(path)?;
    super::read(&directory, |rows| rows.modified(&family, &key))?
        .map(Into::into)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "record item is absent"))
}

/// SQL-only read/modify/write of one item and its sibling receipts.
pub(crate) fn change<T: Send + 'static>(
    path: &Path,
    bytes: usize,
    action: impl FnOnce(&Rows<'_>, &str, &str) -> Result<T> + Send + 'static,
) -> io::Result<T> {
    let (directory, family, key) = prepare(path)?;
    let priority = if family.contains("stops") || family.ends_with("/received-controls") {
        Priority::Control
    } else if family.ends_with("/received") {
        Priority::Interactive
    } else {
        Priority::Normal
    };
    super::write_at(&directory, priority, bytes, move |rows| action(rows, &family, &key))
}

pub(crate) fn page(folder: &Path, category: Option<&str>, after: &str, limit: usize) -> io::Result<Vec<PathBuf>> {
    let (directory, family, _) = prepare(&folder.join(".items"))?;
    let keys = super::read(&directory, |rows| {
        let sql = if category.is_some() {
            "SELECT identity FROM conversation_records WHERE conversation=?1 AND family=?2 AND category=?3 AND identity>?4 ORDER BY identity LIMIT ?5"
        } else {
            "SELECT identity FROM conversation_records WHERE conversation=?1 AND family=?2 AND ?3 IS NULL AND identity>?4 ORDER BY identity LIMIT ?5"
        };
        let mut query = rows.connection().prepare_cached(sql)?;
        Ok(query
            .query_map((rows.conversation(), &family, category, after, limit.clamp(1, 128) as i64), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })?;
    Ok(keys.into_iter().map(|key| crate::record_database::original(&folder.join(key))).collect())
}
pub(crate) fn entries(folder: &Path, category: Option<&str>) -> io::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    let mut after = String::new();
    loop {
        let page = page(folder, category, &after, 128)?;
        let Some(last) = page.last() else { break };
        after = last.file_name().and_then(|name| name.to_str()).expect("stored item identity").to_string();
        result.extend(page);
    }
    Ok(result)
}

fn order_key(sequence: u64, at: Option<jiff::Timestamp>) -> Vec<u8> {
    let mut key = sequence.to_be_bytes().to_vec();
    key.extend_from_slice(&(at.map_or(0, |at| at.as_nanosecond()) ^ i128::MIN).to_be_bytes());
    key
}
pub(crate) enum Enqueued {
    Inserted,
    Existing(Vec<u8>),
    Finished,
}
/// Reserve the queue ordinal and insert the immutable envelope in one commit.
pub(crate) fn enqueue(path: &Path, bytes: Vec<u8>, at: Option<jiff::Timestamp>) -> io::Result<Enqueued> {
    change(path, bytes.len() + 64, move |rows, family, key| {
        let done = Path::new(key).with_extension("done").to_string_lossy().into_owned();
        if rows.get_raw(family, &done)?.is_some() {
            return Ok(Enqueued::Finished);
        }
        if let Some(prior) = rows.get_raw(family, key)? {
            return Ok(Enqueued::Existing(prior));
        }
        let order = match rows.get_raw(family, "sequence")? {
            Some(bytes) => std::str::from_utf8(&bytes)
                .map_err(|error| Error::new(error.to_string()))?
                .parse::<u64>()
                .map_err(|error| Error::new(error.to_string()))?,
            None => {
                let last:Option<Vec<u8>>=rows.connection().query_row("SELECT arrival FROM conversation_records WHERE conversation=?1 AND family=?2 AND category='json' ORDER BY arrival DESC LIMIT 1",(rows.conversation(),family),|row|row.get(0)).optional()?;
                match last {
                    Some(last) if last.len() >= 8 => u64::from_be_bytes(last[..8].try_into().expect("ordinal bytes"))
                        .checked_add(1)
                        .ok_or_else(|| Error::new("queue sequence exhausted"))?,
                    _ => 0,
                }
            }
        };
        let next = order.checked_add(1).ok_or_else(|| Error::new("queue sequence exhausted"))?;
        rows.put_raw(family, "sequence", next.to_string().as_bytes())?;
        rows.put_raw_ordered(family, key, &bytes, &order_key(order, at))?;
        Ok(Enqueued::Inserted)
    })
}

pub(crate) struct Queued {
    pub order: Vec<u8>,
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}
pub(crate) fn queue_end(folder: &Path) -> io::Result<Vec<u8>> {
    let (directory, family, _) = prepare(&folder.join(".items"))?;
    super::read(&directory, |rows| {
        Ok(rows.connection().query_row("SELECT arrival FROM conversation_records WHERE conversation=?1 AND family=?2 AND category='json' ORDER BY arrival DESC,identity DESC LIMIT 1",(rows.conversation(),family),|row|row.get::<_,Vec<u8>>(0)).optional()?.unwrap_or_default())
    })
}
pub(crate) fn queued(folder: &Path, after: &[u8], key: &str, through: &[u8]) -> io::Result<Vec<Queued>> {
    let (directory, family, _) = prepare(&folder.join(".items"))?;
    super::read(&directory, |rows| {
        let mut query=rows.connection().prepare_cached("SELECT arrival,identity,body FROM conversation_records WHERE conversation=?1 AND family=?2 AND category='json' AND (arrival,identity)>(?3,?4) AND arrival<=?5 ORDER BY arrival,identity LIMIT 32")?;
        let mut result = Vec::new();
        let mut bytes = 0;
        let records = query.query_map((rows.conversation(), &family, after, key, through), |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?))
        })?;
        for row in records {
            let (order, key, body) = row?;
            if !result.is_empty() && bytes + body.len() > 1024 * 1024 {
                break;
            }
            bytes += body.len();
            result.push(Queued { order, path: crate::record_database::original(&folder.join(key)), bytes: body });
        }
        Ok(result)
    })
}
