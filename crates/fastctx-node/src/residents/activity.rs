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
    records::ensure_file(directory, family, path, move |path, push| {
        let file = match fastctx_platform::disk::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut reader = io::BufReader::new(file);
        let mut bytes = Vec::new();
        let mut offset = 0u64;
        loop {
            bytes.clear();
            let count = reader.read_until(b'\n', &mut bytes)?;
            if count == 0 || bytes.last() != Some(&b'\n') {
                break;
            }
            offset += count as u64;
            if bytes.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let row = match family {
                "actions" => {
                    let row: Action = serde_json::from_slice(&bytes)?;
                    Source::new(format!("legacy-{offset:016x}"), path.to_path_buf(), serde_json::to_vec(&row)?)?
                        .ordered(row.at)
                }
                "messages" => {
                    let row: MessageLine = serde_json::from_slice(&bytes)?;
                    Source::new(digest(&row)?, path.to_path_buf(), serde_json::to_vec(&row)?)?.ordered(row.at)
                }
                "seen" => {
                    let mut row: Seen = serde_json::from_slice(&bytes)?;
                    let canonical = match fastctx_hands::files::canonical_path(Path::new(&row.path)) {
                        Ok(path) => path,
                        Err(_) => {
                            push(Source::skip(path.to_path_buf())?)?;
                            continue;
                        }
                    };
                    row.path = canonical.to_string_lossy().into_owned();
                    Source::new(row.path.clone(), path.to_path_buf(), serde_json::to_vec(&row)?)?
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
        let from = from.map_or([0; 16], |at| (at.as_nanosecond() ^ i128::MIN).to_be_bytes());
        let until = until.map_or([255; 16], |at| (at.as_nanosecond() ^ i128::MIN).to_be_bytes());
        let body:Option<Vec<u8>>=rows.connection().query_row("SELECT body FROM conversation_records WHERE conversation=?1 AND family='actions' AND arrival>=?2 AND arrival<?3 ORDER BY position DESC LIMIT 1",params![rows.conversation(),from.as_slice(),until.as_slice()],|row|row.get(0)).optional()?;
        body.as_deref().map(records::decode).transpose()
    })
}
pub(crate) fn recent_actions(directory: &ConversationDir, limit: usize) -> io::Result<(Vec<Action>, usize)> {
    prepare(directory, "actions")?;
    records::read(directory, |rows| {
        let count: i64 = rows.connection().query_row(
            "SELECT count(*) FROM conversation_records WHERE conversation=?1 AND family='actions'",
            [rows.conversation()],
            |row| row.get(0),
        )?;
        let mut query=rows.connection().prepare_cached("SELECT body FROM conversation_records WHERE conversation=?1 AND family='actions' ORDER BY position DESC LIMIT ?2")?;
        let mut actions = query
            .query_map((rows.conversation(), limit.min(512) as i64), |row| row.get::<_, Vec<u8>>(0))?
            .map(|row| records::decode(&row?))
            .collect::<fastctx_storage::Result<Vec<_>>>()?;
        actions.reverse();
        Ok((actions, count as usize))
    })
}
pub(crate) fn asked(path: &Path, target: Option<&str>) -> io::Result<Vec<Action>> {
    let directory = directory(path)?;
    prepare(&directory, "actions")?;
    records::read(&directory, |rows| {
        let mut query=rows.connection().prepare_cached("SELECT body FROM conversation_records WHERE conversation=?1 AND family='actions' AND json_extract(body,'$.hand')='ask_colleague' AND (?2 IS NULL OR json_extract(body,'$.result')=?2) ORDER BY position DESC")?;
        query
            .query_map((rows.conversation(), target), |row| row.get::<_, Vec<u8>>(0))?
            .map(|row| records::decode(&row?))
            .collect()
    })
}
