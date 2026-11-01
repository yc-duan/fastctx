//! Outgoing request lookup is global; its durable row belongs to its conversation.
use super::Request;
use crate::{Node, records};
use fastctx_storage::{Priority, rusqlite::OptionalExtension};
use fastctx_types::{NoticeId, layout::ConversationDir};
use std::io;

fn prepare(node: &Node) -> io::Result<ConversationDir> {
    let root = node.world.path().join("control-requests");
    let (directory, _, _, _) = crate::record_database::item(&root.join(".requests"))?;
    records::ensure_each(&directory, "control-requests", |push| {
        let entries = match fastctx_platform::disk::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let request: Request = serde_json::from_slice(&fastctx_platform::disk::read(&path)?)?;
            let (owner, _, _, _) = crate::record_database::item(&request.path)?;
            let (_, conversation) = crate::record_database::get(&owner)?;
            let key = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| io::Error::other("invalid control request filename"))?
                .to_string();
            push(records::Source::new(key, path, serde_json::to_vec(&request)?)?.scoped(conversation))?;
        }
        Ok(())
    })?;
    Ok(directory)
}

pub(super) fn keep(node: &Node, id: &NoticeId, request: &Request) -> io::Result<()> {
    prepare(node)?;
    let (directory, _, _, _) = crate::record_database::item(&request.path)?;
    let (id, body) = (id.to_string(), serde_json::to_vec(request)?);
    records::write_at(&directory, Priority::Control, body.len(), move |rows| {
        rows.put_raw("control-requests", &id, &body)
    })
}

pub(super) fn get(node: &Node, id: &NoticeId) -> io::Result<Option<Request>> {
    records::read(&prepare(node)?, |rows| {
        let body: Option<Vec<u8>> = rows
            .connection()
            .query_row(
                "SELECT body FROM conversation_records WHERE family='control-requests' AND identity=?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        body.map(|body| records::decode(&body)).transpose()
    })
}

pub(super) fn page(node: &Node, after: &str) -> io::Result<Vec<(String, jiff::Timestamp, Request)>> {
    records::read(&prepare(node)?, |rows| {
        rows.connection().prepare_cached("SELECT identity,modified,body FROM conversation_records WHERE family='control-requests' AND identity>?1 ORDER BY identity LIMIT 128")?
            .query_map([after], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?)))?
            .map(|row| { let (key, modified, body) = row?; Ok((key, modified.parse().map_err(|error: jiff::Error| fastctx_storage::Error::new(error.to_string()))?, records::decode(&body)?)) }).collect()
    })
}

pub(super) fn delete(node: &Node, id: String) -> io::Result<()> {
    records::write_at(&prepare(node)?, Priority::Background, id.len(), move |rows| {
        rows.connection()
            .execute("DELETE FROM conversation_records WHERE family='control-requests' AND identity=?1", [id])?;
        Ok(())
    })
}
