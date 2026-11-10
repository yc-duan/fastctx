//! Exact tool actions over validated scalar spans. Long commands, paths and
//! diffs remain body plans; bounded labels are used only to select semantics.
use super::{fields, message::string};
use crate::claude::ingest::document::{
    Address, Span,
    read::{Reader, exact},
};
use fastctx_types::{session::AgentAct, sync::cursor::ByteCount};
use std::io;

#[derive(Clone, Debug)]
pub struct Name {
    pub name: Span,
    pub server: Option<Span>,
    pub world: bool,
    pub known: Option<String>,
}
#[derive(Clone, Debug)]
pub enum Action {
    Command { command: Option<Span>, cwd: Option<Span> },
    Edit { path: Option<Span>, before: Option<Span>, after: Option<Span> },
    MultiEdit { path: Option<Span>, edits: Option<Address> },
    Write { path: Option<Span>, text: Option<Span> },
    Patch { text: Option<Span> },
    Search { query: Option<Span> },
    Colleague { to: Option<Span>, task: Option<Span>, from_result: bool },
    Agent { act: AgentAct, to: Option<Span>, agent_kind: Option<Span>, task: Option<Span> },
    Call,
}

pub async fn name(reader: &Reader<'_>, block: Address) -> io::Result<Option<Name>> {
    let Some(full) = string(reader, block, "name").await? else { return Ok(None) };
    let mut name = full;
    let mut server = None;
    if fields::prefix(reader, full, 5).await? == "mcp__"
        && let Some(separator) = separator(reader, full, 5).await?
    {
        full.end()?;
        server = Some(Span { offset: ByteCount(full.offset.0 + 5), bytes: ByteCount(separator - 5) });
        name =
            Span { offset: ByteCount(full.offset.0 + separator + 2), bytes: ByteCount(full.bytes.0 - separator - 2) };
    }
    let world = match server {
        None => false,
        Some(server) => {
            reader.text(server, 8).await?.as_deref() == Some("fastctx")
                || server.bytes.0 >= 8
                    && exact(reader.input, &reader.document.bodies.strings, server.offset.0 + server.bytes.0 - 8, 8)
                        .await?
                        == b"_fastctx"
        }
    };
    let known = reader.text(name, 128).await?;
    Ok(Some(Name { name, server, world, known }))
}

pub async fn action(reader: &Reader<'_>, name: &Name, input: Address) -> io::Result<Action> {
    let field = |key| string(reader, input, key);
    let own = name.server.is_none();
    let tool = name.known.as_deref().unwrap_or("");
    Ok(match tool {
        "Bash" if own => Action::Command { command: field("command").await?, cwd: None },
        "run" if name.world => Action::Command { command: field("command").await?, cwd: field("cwd").await? },
        "Edit" if own => Action::Edit {
            path: field("file_path").await?,
            before: field("old_string").await?,
            after: field("new_string").await?,
        },
        "edit" if name.world => Action::Edit {
            path: field("file_path").await?,
            before: field("old_string").await?,
            after: field("new_string").await?,
        },
        "MultiEdit" if own => Action::MultiEdit {
            path: field("file_path").await?,
            edits: reader.field(input, "edits").await?.map(|(address, _)| address),
        },
        "Write" if own => Action::Write { path: field("file_path").await?, text: field("content").await? },
        "write" if name.world => Action::Write { path: field("file_path").await?, text: field("content").await? },
        "apply_patch" if name.world => Action::Patch { text: field("input").await? },
        "WebSearch" if own => Action::Search { query: field("query").await? },
        "ask_colleague" if name.world => {
            Action::Colleague { to: None, task: field("description").await?, from_result: true }
        }
        "message_colleague" if name.world => {
            Action::Colleague { to: field("to").await?, task: None, from_result: false }
        }
        "Agent" | "Task" if own => Action::Agent {
            act: AgentAct::Start,
            to: None,
            agent_kind: field("subagent_type").await?,
            task: field("description").await?,
        },
        "SendMessage" if own => Action::Agent {
            act: AgentAct::Message,
            to: field("to").await?,
            agent_kind: None,
            task: field("message").await?.or(field("text").await?),
        },
        _ => Action::Call,
    })
}
async fn separator(reader: &Reader<'_>, full: Span, start: u64) -> io::Result<Option<u64>> {
    let mut at = start;
    let mut underscore = false;
    while at < full.bytes.0 {
        let take = (full.bytes.0 - at).min(16 * 1024) as usize;
        let bytes = exact(reader.input, &reader.document.bodies.strings, full.offset.0 + at, take).await?;
        for (index, &byte) in bytes.iter().enumerate() {
            if underscore && byte == b'_' {
                return Ok(Some(at + index as u64 - 1));
            }
            underscore = byte == b'_';
        }
        at += bytes.len() as u64;
    }
    Ok(None)
}
