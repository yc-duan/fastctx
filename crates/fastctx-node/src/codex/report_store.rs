//! Each durable host report has its own identity and indexed queue position.
//! Legacy snapshots are streamed once; normal arrivals are never hash-deduped.
use super::{HostEvent, Node};
use crate::records::{self, Source};
use fastctx_storage::{Priority, rusqlite::OptionalExtension};
use fastctx_types::layout::ConversationDir;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{self, BufRead},
    path::Path,
};

fn family(thread: &str) -> String {
    format!("host-reports/{}", crate::speech::fixed_id("thread", &[thread]))
}
fn scope(node: &Node, thread: &str) -> io::Result<ConversationDir> {
    let id = node
        .residents
        .resident_of_thread(thread)
        .ok_or_else(|| io::Error::other("the host thread has no conversation"))?;
    Ok(node.world.conversation(&id))
}
fn prepare(node: &Node, thread: &str) -> io::Result<(ConversationDir, String)> {
    let directory = scope(node, thread)?;
    let family = family(thread);
    records::ensure_each(&directory, &family, |push| {
        let current = directory.books().join("host-reports.json");
        let legacy = node
            .world
            .path()
            .join("codex-reports")
            .join(format!("{}.json", crate::speech::fixed_id("thread", &[thread])));
        let mut seen = HashSet::<[u8; 32]>::new();
        let mut ordinal = 0u64;
        let mut first = true;
        for path in [legacy.clone(), current.clone()] {
            if !path.try_exists()? {
                continue;
            }
            if records::legacy::field::<String>(&path, "thread")?.as_deref() != Some(thread) {
                return Err(io::Error::other(format!("{} belongs to another thread", path.display())));
            }
            records::legacy::array::<HostEvent>(&path, Some("events"), &mut |event| {
                let body = serde_json::to_vec(&event)?;
                let hash = Sha256::digest(&body).into();
                let new = seen.insert(hash);
                if first || new {
                    push(Source::new(format!("legacy-{ordinal:020}"), path.clone(), body)?)?;
                    ordinal += 1;
                }
                Ok(())
            })?;
            first = false;
        }
        for path in [current.with_extension("jsonl"), legacy.with_extension("jsonl")] {
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let mut reader = io::BufReader::new(file);
            let mut line = Vec::new();
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                let event: HostEvent = match serde_json::from_slice(&line) {
                    Ok(event) => event,
                    Err(_) if line.last() != Some(&b'\n') => {
                        node.log
                            .line(format_args!("an interrupted host report remains preserved at {}", path.display()));
                        break;
                    }
                    Err(error) => return Err(io::Error::other(format!("{}: {error}", path.display()))),
                };
                let body = serde_json::to_vec(&event)?;
                if seen.insert(Sha256::digest(&body).into()) {
                    push(Source::new(format!("legacy-{ordinal:020}"), path.clone(), body)?)?;
                    ordinal += 1;
                }
            }
        }
        Ok(())
    })?;
    Ok((directory, family))
}
pub(super) fn append(node: &Node, thread: &str, key: String, event: &HostEvent) -> io::Result<u64> {
    let (directory, family) = prepare(node, thread)?;
    let body = serde_json::to_vec(event)?;
    records::write_at(&directory, Priority::Interactive, body.len(), move |rows| {
        if let Some(prior) = rows.get_raw(&family, &key)? {
            if prior != body {
                return Err(fastctx_storage::Error::new("host report identity changed"));
            }
        } else {
            rows.put_raw(&family, &key, &body)?;
        }
        Ok(rows.connection().query_row(
            "SELECT position FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
            (rows.conversation(), family, key),
            |row| row.get::<_, i64>(0),
        )? as u64)
    })
}
pub(super) fn latest(node: &Node, thread: &str) -> io::Result<u64> {
    let (directory, family) = prepare(node, thread)?;
    records::read(&directory, |rows| {
        Ok(rows
            .connection()
            .query_row(
                "SELECT max(position) FROM conversation_records WHERE conversation=?1 AND family=?2",
                (rows.conversation(), family),
                |row| row.get::<_, Option<i64>>(0),
            )?
            .unwrap_or(0) as u64)
    })
}
pub(super) fn first(node: &Node, thread: &str) -> io::Result<Option<(u64, HostEvent)>> {
    let (directory, family) = prepare(node, thread)?;
    records::read(&directory, |rows| {
        let row=rows.connection().query_row("SELECT position,body FROM conversation_records WHERE conversation=?1 AND family=?2 ORDER BY position LIMIT 1",(rows.conversation(),family),|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Vec<u8>>(1)?))).optional()?;
        row.map(|(position, body)| records::decode(&body).map(|event| (position as u64, event))).transpose()
    })
}
pub(super) fn acknowledge(node: &Node, thread: &str, position: u64) -> io::Result<()> {
    let directory = scope(node, thread)?;
    let family = family(thread);
    records::write_at(&directory, Priority::Interactive, 128, move |rows| {
        rows.connection().execute(
            "DELETE FROM conversation_records WHERE conversation=?1 AND family=?2 AND position=?3",
            (rows.conversation(), family, position as i64),
        )?;
        Ok(())
    })
}
pub(crate) fn pending(directory: &ConversationDir) -> io::Result<bool> {
    records::read(directory, |rows| {
        Ok(rows.connection().query_row("SELECT 1 FROM conversation_records WHERE conversation=?1 AND family>='host-reports/' AND family<'host-reports0' LIMIT 1",[rows.conversation()],|row|row.get::<_,i64>(0)).optional()?.is_some())
    })
}
pub(super) fn discard_legacy(node: &Node) -> io::Result<()> {
    let root = node.world.path().join("codex-reports");
    for path in crate::retention::entries(&root)? {
        if path.extension().is_some_and(|ext| ext == "json") {
            let thread = records::legacy::field::<String>(&path, "thread")?
                .ok_or_else(|| io::Error::other("legacy reports have no thread"))?;
            if node.residents.resident_of_thread(&thread).is_some() {
                continue;
            }
        } else if path.extension().is_some_and(|ext| ext == "jsonl") && claimed_legacy(node, &path) {
            continue;
        }
        std::fs::remove_file(path)?;
    }
    Ok(())
}
fn claimed_legacy(node: &Node, path: &Path) -> bool {
    node.residents.everyone().filter_map(|resident| resident.about.session).any(|thread| {
        path.file_stem().and_then(|s| s.to_str()) == Some(crate::speech::fixed_id("thread", &[&thread]).as_str())
    })
}

pub(super) fn pending_thread(node: &Node, thread: &str) -> io::Result<bool> {
    first(node, thread).map(|row| row.is_some())
}
pub(super) fn delete(directory: &ConversationDir, thread: &str) -> io::Result<()> {
    let family = family(thread);
    loop {
        let family = family.clone();
        let count = records::write_at(directory, Priority::Background, 4096, move |rows| {
            Ok(rows.connection().execute("DELETE FROM conversation_records WHERE position IN (SELECT position FROM conversation_records WHERE conversation=?1 AND family=?2 LIMIT 64)",(rows.conversation(),family))?)
        })?;
        if count == 0 {
            return Ok(());
        }
    }
}
