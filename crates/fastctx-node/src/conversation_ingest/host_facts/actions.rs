//! Full actions use public content references; summaries never become arguments.
use super::{
    content::{self, TextWriter},
    forward_list::ForwardList,
    lists::List,
};
use crate::conversation_ingest::step_content::StepContent;
use fastctx_hosts::claude::ingest::{
    document::{Address, read::Reader},
    semantic::{
        children::Children,
        message::string,
        tool::{self, Action, Name},
    },
};
use fastctx_sync::store::Error;
use fastctx_types::sync::{conversation_timeline::*, cursor::Count, manifest::ContentRef};

pub async fn build(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    name: &Name,
    input: Address,
) -> Result<ToolActionV1, Error> {
    let text = |span| content::text(io, reader, span);
    let optional = |span| content::optional_text(io, reader, span);
    Ok(match tool::action(reader, name, input).await.map_err(super::error)? {
        Action::Command { command, cwd } => {
            ToolActionV1::Command { command: text(command).await?, cwd: optional(cwd).await?, job: None }
        }
        Action::Edit { path, before, after } => {
            let mut diff = TextWriter::new(io);
            diff.marked(reader, before, b'-').await?;
            diff.marked(reader, after, b'+').await?;
            ToolActionV1::Edit {
                changes: single_change(
                    io,
                    text(path).await?,
                    FactChangeV1::Updated,
                    FactTextV1::Content { content: diff.finish().await? },
                )
                .await?,
            }
        }
        Action::Write { path, text: added } => {
            let mut diff = TextWriter::new(io);
            diff.marked(reader, added, b'+').await?;
            ToolActionV1::Edit {
                changes: single_change(
                    io,
                    text(path).await?,
                    FactChangeV1::Replaced,
                    FactTextV1::Content { content: diff.finish().await? },
                )
                .await?,
            }
        }
        Action::MultiEdit { path, edits } => {
            let mut parts = List::new(io);
            if let Some(edits) = edits
                && let Some(mut children) = Children::of(reader, edits).await.map_err(super::error)?
            {
                while let Some((edit, node)) = children.step(reader).await.map_err(super::error)? {
                    let mut diff = TextWriter::new(io);
                    diff.push(b"@@\n").await?;
                    diff.marked(reader, string(reader, edit, "old_string").await.map_err(super::error)?, b'-').await?;
                    diff.marked(reader, string(reader, edit, "new_string").await.map_err(super::error)?, b'+').await?;
                    parts.prepend(node.ordinal, FactPartV1::Text { text: diff.finish().await? }).await?;
                }
            }
            let diff = match parts.finish().await? {
                Some(parts) => FactTextV1::Parts { parts },
                None => FactTextV1::Content { content: content::literal(io, "").await? },
            };
            ToolActionV1::Edit { changes: single_change(io, text(path).await?, FactChangeV1::Updated, diff).await? }
        }
        Action::Patch { text } => ToolActionV1::Edit { changes: super::patch::changes(io, reader, text).await? },
        Action::Search { query } => ToolActionV1::Search { query: text(query).await? },
        Action::Colleague { to, task, .. } => {
            ToolActionV1::Colleague { to: optional(to).await?, task: optional(task).await? }
        }
        Action::Agent { act, to, agent_kind, task } => {
            let agents = match to {
                Some(to) => {
                    let mut list = ForwardList::new(io, FactPartKindV1::Text);
                    list.push(FactPartV1::Text { text: text(Some(to)).await? }).await?;
                    Some(list.finish().await?)
                }
                None => None,
            };
            ToolActionV1::Agent { act, agents, agent_kind: optional(agent_kind).await?, task: optional(task).await? }
        }
        Action::Call => ToolActionV1::Call,
    })
}
async fn single_change(
    io: &StepContent<'_>,
    path: ContentRef,
    how: FactChangeV1,
    diff: FactTextV1,
) -> Result<ContentRef, Error> {
    content::publish(
        io,
        ItemFactV1::Parts {
            part_kind: FactPartKindV1::Change,
            start: Count(0),
            parts: vec![FactPartV1::Change { path, how, diff }],
        },
    )
    .await
}
