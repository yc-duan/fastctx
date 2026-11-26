//! Indexed actions, messages and observed file versions. Historical actions and
//! messages last until conversation deletion; seen replaces one canonical key.

use crate::records::{self, Source};
use fastctx_storage::{
    Priority,
    rusqlite::{OptionalExtension, params},
};
use fastctx_types::{
    layout::ConversationDir,
    residents::{Action, MessageKind, MessageLine, Seen},
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    io::{self, BufRead},
    path::Path,
};

fn digest(value: &impl Serialize) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

pub(crate) fn prepare(directory: &ConversationDir, family: &'static str) -> io::Result<()> {
    let path = match family {
        "actions" => directory.actions(),
        "messages" => directory.messages(),
        "seen" => directory.seen(),
        _ => return Err(io::Error::other("unknown resident activity family")),
    };
    // The import and its check read the file twice; both see the same
    // canonical paths even when the files they name change in between.
    let mut canonical = std::collections::HashMap::<String, Option<String>>::new();
    records::ensure_file(directory, family, path, move |path, push| {
        let file = match fastctx_platform::disk::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut reader = io::BufReader::new(file);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            let count = reader.read_until(b'\n', &mut bytes)?;
            if count == 0 {
                break;
            }
            // A last line cut short by a stop mid-append was never written whole;
            // one that is whole but lacks its line end still counts.
            if bytes.last() != Some(&b'\n') && serde_json::from_slice::<serde::de::IgnoredAny>(&bytes).is_err() {
                break;
            }
            if bytes.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let row = match family {
                "actions" => {
                    let row: Action = serde_json::from_slice(&bytes)?;
                    Source::new(
                        format!("legacy-{}", digest(&row)?),
                        path.to_path_buf(),
                        records::legacy::raw::merge(&bytes, &row)?,
                    )?
                    .counted()
                    .ordered(row.at)
                }
                "messages" => {
                    let row: MessageLine = serde_json::from_slice(&bytes)?;
                    Source::new(digest(&row)?, path.to_path_buf(), records::legacy::raw::merge(&bytes, &row)?)?
                        .ordered(row.at)
                }
                "seen" => {
                    let mut row: Seen = serde_json::from_slice(&bytes)?;
                    let named = canonical
                        .entry(row.path.clone())
                        .or_insert_with(|| {
                            fastctx_hands::files::canonical_path(Path::new(&row.path))
                                .ok()
                                .map(|path| path.to_string_lossy().into_owned())
                        })
                        .clone();
                    let Some(named) = named else {
                        push(Source::skip(path.to_path_buf())?)?;
                        continue;
                    };
                    row.path = named;
                    Source::new(row.path.clone(), path.to_path_buf(), records::legacy::raw::merge(&bytes, &row)?)?
                        .ordered(row.at)
                        .replacing()
                }
                _ => unreachable!(),
            };
            push(row)?;
        }
        Ok(())
    })
}

pub(crate) fn action(directory: &ConversationDir, action: &Action) -> io::Result<()> {
    prepare(directory, "actions")?;
    let (action, key) = (action.clone(), crate::speech::new_id("action").to_string());
    let bytes = serde_json::to_vec(&action)?.len();
    records::write_at(directory, Priority::Normal, bytes, move |rows| {
        rows.put_ordered("actions", &key, &action, action.at)
    })
}

pub(crate) fn message(directory: &ConversationDir, message: &MessageLine) -> io::Result<()> {
    prepare(directory, "messages")?;
    let (message, bytes) = (message.clone(), serde_json::to_vec(message)?.len());
    records::write_at(directory, Priority::Normal, bytes, move |rows| message_in(rows, &message))
}
pub(crate) fn message_in(rows: &records::Rows<'_>, message: &MessageLine) -> fastctx_storage::Result<()> {
    if let MessageKind::Ended { turn, .. } = &message.what {
        let known=rows.connection().query_row("SELECT 1 FROM conversation_records WHERE conversation=?1 AND family='messages' AND json_extract(body,'$.kind')='ended' AND json_extract(body,'$.turn')=?2 LIMIT 1",(rows.conversation(),turn),|row|row.get::<_,i32>(0)).optional()?.is_some();
        if known {
            return Ok(());
        }
    }
    let key = digest(message).map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
    rows.put_ordered("messages", &key, message, message.at)
}

pub(crate) fn seen(directory: &ConversationDir, seen: &[Seen]) -> io::Result<()> {
    prepare(directory, "seen")?;
    for group in seen.chunks(16) {
        let mut prepared = Vec::new();
        for row in group {
            let key = match fastctx_hands::files::canonical_path(Path::new(&row.path)) {
                Ok(path) => path.to_string_lossy().into_owned(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let mut row = row.clone();
            row.path = key.clone();
            prepared.push((key, row));
        }
        let bytes = serde_json::to_vec(&prepared)?.len();
        records::write_at(directory, Priority::Normal, bytes, move |rows| {
            for (key, row) in prepared {
                rows.put_ordered("seen", &key, &row, row.at)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}
pub(crate) fn get_seen(directory: &ConversationDir, path: &Path) -> io::Result<Option<Seen>> {
    prepare(directory, "seen")?;
    let canonical = match fastctx_hands::files::canonical_path(path) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    records::read(directory, |rows| rows.get("seen", &canonical.to_string_lossy()))
}

pub(crate) fn lines<T: DeserializeOwned>(path: &Path) -> io::Result<Vec<T>> {
    let directory = directory(path)?;
    let family = family(path)?;
    prepare(&directory, family)?;
    records::read(&directory, |rows| rows.all(family))
}
fn directory(path: &Path) -> io::Result<ConversationDir> {
    let parent = path.parent().ok_or_else(|| io::Error::other("activity has no conversation"))?;
    let parent = if path.file_name().is_some_and(|name| name == "seen") {
        parent.parent().ok_or_else(|| io::Error::other("seen has no conversation"))?
    } else {
        parent
    };
    Ok(ConversationDir::at(parent))
}
fn family(path: &Path) -> io::Result<&'static str> {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("actions") => Ok("actions"),
        Some("messages") => Ok("messages"),
        Some("seen") => Ok("seen"),
        _ => Err(io::Error::other("invalid activity path")),
    }
}
pub(crate) fn last_action(
    path: &Path,
    from: Option<jiff::Timestamp>,
    until: Option<jiff::Timestamp>,
) -> io::Result<Option<Action>> {
    let directory = directory(path)?;
    prepare(&directory, "actions")?;
    records::read(&directory, |rows| {
        let body: Option<Vec<u8>> = if from.is_none() && until.is_none() {
            rows.connection().query_row(LAST_ACTION, [rows.conversation()], |row| row.get(0)).optional()?
        } else {
            let from = from.map_or([0; 16], |at| (at.as_nanosecond() ^ i128::MIN).to_be_bytes());
            let until = until.map_or([255; 16], |at| (at.as_nanosecond() ^ i128::MIN).to_be_bytes());
            last_within(rows.connection(), &rows.conversation(), &from, &until)?
        };
        body.as_deref().map(records::decode).transpose()
    })
}

/// The last action written: the end of the family's order, one row read.
/// Asked with a span of arrivals too, SQLite walked every action of the
/// conversation by arrival and sorted them with their bodies to find it.
const LAST_ACTION: &str = "SELECT body FROM conversation_records INDEXED BY conversation_records_order \
    WHERE conversation=?1 AND family='actions' ORDER BY position DESC LIMIT 1";
/// The last action written of those that arrived in a span of time. An
/// action that happened earlier may be written later, and two may arrive at
/// one moment, so the span's end by arrival is not it: it is the greatest
/// place among those of the span. The end of the order is looked at first,
/// `NEAR` actions of it: a conversation still in the town it is asked of, or
/// one that left it lately, has it there, however much arrived in the span.
/// Not there, it is told by the index of arrivals, an entry for each that
/// arrived in the span and no row: a conversation that did nothing in the
/// span costs a seek, however much it wrote after. Either way alone cost a
/// machine dearly: gone back along the order to its start, every action
/// written since the span, at each asking of who had been in a town; by the
/// arrivals alone, every action of a long stay.
fn last_within<C: fastctx_storage::rusqlite::ToSql>(
    db: &fastctx_storage::rusqlite::Connection,
    conversation: &C,
    from: &[u8],
    until: &[u8],
) -> fastctx_storage::rusqlite::Result<Option<Vec<u8>>> {
    let mut near = db.prepare_cached(LAST_ACTIONS)?;
    let mut rows = near.query(params![conversation, NEAR])?;
    let mut place: Option<i64> = None;
    while let Some(row) = rows.next()? {
        let arrived: Option<Vec<u8>> = row.get(1)?;
        if arrived.as_deref().is_some_and(|at| at >= from && at < until) {
            place = Some(row.get(0)?);
            break;
        }
    }
    drop(rows);
    let place = match place {
        Some(place) => Some(place),
        None => {
            db.prepare_cached(LAST_ARRIVED_WITHIN)?.query_row(params![conversation, from, until], |row| row.get(0))?
        }
    };
    match place {
        Some(place) => {
            db.prepare_cached(ACTION_AT)?.query_row(params![conversation, place], |row| row.get(0)).optional()
        }
        None => Ok(None),
    }
}

/// How many actions at the end of the order are looked at for the last of
/// a span before the span's arrivals are gone through.
const NEAR: i64 = 256;
const LAST_ACTIONS: &str = "SELECT position, arrival FROM conversation_records INDEXED BY conversation_records_order \
    WHERE conversation=?1 AND family='actions' ORDER BY position DESC LIMIT ?2";
const LAST_ARRIVED_WITHIN: &str = "SELECT max(position) FROM conversation_records INDEXED BY conversation_records_arrival \
    WHERE conversation=?1 AND family='actions' AND arrival>=?2 AND arrival<?3";
const ACTION_AT: &str = "SELECT body FROM conversation_records WHERE conversation=?1 AND position=?2";
pub(crate) fn recent_actions(directory: &ConversationDir, limit: usize) -> io::Result<(Vec<Action>, usize)> {
    prepare(directory, "actions")?;
    records::read(directory, |rows| {
        let count: i64 = rows.connection().query_row(
            "SELECT count(*) FROM conversation_records WHERE conversation=?1 AND family='actions'",
            [rows.conversation()],
            |row| row.get(0),
        )?;
        let mut query=rows.connection().prepare_cached("SELECT body FROM conversation_records INDEXED BY conversation_records_order WHERE conversation=?1 AND family='actions' ORDER BY position DESC LIMIT ?2")?;
        let mut actions = query
            .query_map((rows.conversation(), limit.min(512) as i64), |row| row.get::<_, Vec<u8>>(0))?
            .map(|row| records::decode(&row?))
            .collect::<fastctx_storage::Result<Vec<_>>>()?;
        actions.reverse();
        Ok((actions, count as usize))
    })
}
/// How many times each conversation of `world` had its actions written, by
/// its key, in one read for them all (`actions_version`, of one); none where
/// its records are not open. A conversation never written is not in it.
pub(crate) fn actions_versions(
    world: &fastctx_types::layout::WorldDir,
) -> io::Result<Option<std::collections::HashMap<String, u64>>> {
    let Some(database) = crate::record_database::opened(world) else { return Ok(None) };
    database
        .read_now(|connection| {
            let mut query = connection.prepare_cached(
                "SELECT conversation, version FROM conversation_record_versions WHERE family='actions'",
            )?;
            let versions = query
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)))?
                .collect::<Result<std::collections::HashMap<_, _>, _>>()?;
            Ok(versions)
        })
        .map(Some)
        .map_err(io::Error::other)
}

/// How many times the actions at `path` were written: the same number, the
/// same actions.
pub(crate) fn actions_version(path: &Path) -> io::Result<u64> {
    let directory = directory(path)?;
    prepare(&directory, "actions")?;
    records::read(&directory, |rows| {
        Ok(rows
            .connection()
            .query_row(
                "SELECT version FROM conversation_record_versions WHERE conversation=?1 AND family='actions'",
                [rows.conversation()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0) as u64)
    })
}
/// The conversation's asks of colleagues, the newest first; with `target`, of
/// that colleague alone.
const ASKED: &str = "SELECT body FROM conversation_records INDEXED BY conversation_records_action WHERE conversation=?1 AND family='actions' AND json_extract(body,'$.hand')='ask_colleague' AND (?2 IS NULL OR json_extract(body,'$.result')=?2) ORDER BY position DESC";

pub(crate) fn asked(path: &Path, target: Option<&str>) -> io::Result<Vec<Action>> {
    let directory = directory(path)?;
    prepare(&directory, "actions")?;
    records::read(&directory, |rows| {
        let mut query = rows.connection().prepare_cached(ASKED)?;
        query
            .query_map((rows.conversation(), target), |row| row.get::<_, Vec<u8>>(0))?
            .map(|row| records::decode(&row?))
            .collect()
    })
}

/// Where a colleague was asked and who it is, as an ask's action records it.
#[derive(serde::Deserialize)]
struct Asked {
    machine: fastctx_types::MachineName,
    result: String,
}

/// Every colleague the conversation asked, its machine and its id as `asked`
/// reads them, of a record missing what else an action holds too; one
/// without those two is passed over and counted.
pub(crate) fn asked_readable(path: &Path) -> io::Result<(Vec<(fastctx_types::MachineName, String)>, usize)> {
    let directory = directory(path)?;
    prepare(&directory, "actions")?;
    records::read(&directory, |rows| {
        let mut query = rows.connection().prepare_cached(ASKED)?;
        let bodies = query
            .query_map((rows.conversation(), None::<&str>), |row| row.get::<_, Vec<u8>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut asked = Vec::with_capacity(bodies.len());
        let mut unread = 0;
        for body in bodies {
            match records::decode::<Asked>(&body) {
                Ok(Asked { machine, result }) => asked.push((machine, result)),
                Err(_) => unread += 1,
            }
        }
        Ok((asked, unread))
    })
}

#[cfg(test)]
mod tests;
