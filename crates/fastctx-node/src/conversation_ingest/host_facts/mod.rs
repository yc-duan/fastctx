//! Public host facts are decoded once at ingestion; readers never reinterpret
//! original JSONL. Content writers retain exact typed references and bounded IO.
mod actions;
mod content;
mod context;
mod display_budget;
mod forward_list;
mod id_list;
mod lists;
pub(super) mod other;
mod output_scan;
mod outputs;
mod part_tree;
mod patch;
mod row_stats;
pub(super) mod session;
pub(super) mod tools;
mod usage;

use super::{bodies::Bodies, indexes::Indexes, step_content::StepContent};
use fastctx_hosts::claude::ingest::{
    document::{Completion, read::Reader},
    jsonl::Row,
    semantic::{
        message,
        row::{self, RowKind},
    },
};
use fastctx_sync::store::{Error, ingest_effects::SourceFamily};
use fastctx_types::sync::{
    conversation_identity::{NativeId, NativeItemKey},
    conversation_timeline::*,
    cursor::Count,
};

pub(super) async fn attach(
    io: &StepContent<'_>,
    row: &Row,
    rows: &mut Indexes<'_>,
    placement: Option<&Placement>,
    identity: &ItemIdentity,
    display: &mut ItemDisplayV1,
) -> Result<(), Error> {
    let Completion::Valid(document) = &row.content else { return Ok(()) };
    let input = Bodies { content: io };
    let reader = Reader { document, input: &input };
    let RowKind::Row(header) = row::header(&reader, None).await.map_err(error)? else { return Ok(()) };
    let mut facts = display.facts.take().map(|facts| *facts).unwrap_or_default();
    let tool = display.kind == DisplayKind::Tool || facts.tool_status.is_some();
    if !tool {
        facts.at = header.at;
        facts.started_at = header.at;
    } else {
        facts.at = facts.at.or(header.at);
    }
    let observed = message::decode(&reader, &header).await.map_err(error)?;
    if header.kind == "assistant"
        && !tool
        && identity.role() == fastctx_types::sync::conversation_identity::ItemRole::AssistantBlock
    {
        facts.tokens = observed.tokens.clone();
    }
    if facts.native.is_none() {
        let block = match identity {
            ItemIdentity::Native(key) => match &key.native {
                NativeItemKey::ClaudeBlock { block, .. } => Some(*block),
                _ => None,
            },
            _ => None,
        };
        facts.native = Some(
            content::publish(
                io,
                ItemFactV1::Native {
                    identity: identity.clone(),
                    draft: None,
                    source: io.target().step().primary().source.clone(),
                    generation: io.target().step().generation().clone(),
                    line: Some(Count(row.line.0.checked_add(1).ok_or(Error::Busy)?)),
                    block,
                    response: native(header.response.as_deref())?,
                    context: context_ref(rows, header.uuid.as_deref()).await?,
                },
            )
            .await?,
        );
    }
    facts.turn = if let Some(turn) = placement.and_then(|placement| placement.turn.as_ref()) {
        match rows.get::<TurnDeclaration>(SourceFamily::HostRows, &turn_key(turn)).await? {
            Some(declaration) => Some(
                content::publish(
                    io,
                    ItemFactV1::TurnMapping {
                        turn: turn.clone(),
                        key: declaration.key,
                        proof: PlacementProof::Bootstrap {
                            method: NativeId::new("claude-prompt-chain-v1").map_err(invalid)?,
                        },
                    },
                )
                .await?,
            ),
            None => None,
        }
    } else {
        None
    };
    if display.kind == DisplayKind::Thinking {
        let stats = row_stats::get(io, &reader, &header, row, rows).await?;
        facts.duration_ms = (stats.thinking == 1).then_some(observed.thinking_duration_ms).flatten();
    }
    if matches!(display.kind, DisplayKind::User | DisplayKind::Message | DisplayKind::Thinking)
        && facts.semantic.is_none()
    {
        let detail = if let ItemIdentity::Native(key) = identity
            && let NativeItemKey::ClaudeBlock { block, .. } = &key.native
        {
            let key = format!(
                "claude/block/{}/{:016x}/{:016x}",
                io.target().step().primary().source.as_str(),
                row.start.0,
                block.0
            );
            match rows.get(SourceFamily::HostRows, &key).await? {
                Some(address) => reader.node(address).await.map_err(error)?.raw,
                None => reader.node(document.root).await.map_err(error)?.raw,
            }
        } else {
            reader.node(document.root).await.map_err(error)?.raw
        };
        facts.semantic = Some(
            content::publish(
                io,
                ItemFactV1::Message {
                    response: native(header.response.as_deref())?,
                    model: content::optional_text(io, &reader, observed.model).await?,
                    stop_reason: content::optional_text(io, &reader, observed.stop_reason).await?,
                    api_error: observed.api_error,
                    usage: match observed.usage {
                        Some(span) => Some(content::raw(io, &reader, span).await?),
                        None => None,
                    },
                    detail: content::raw(io, &reader, detail).await?,
                },
            )
            .await?,
        );
    }
    display.facts = Some(Box::new(facts));
    display_budget::fit(display)
}

/// Bootstrap declarations already choose the canonical identity. A stronger
/// shared TurnBinding may later supply another native key for this same target.
pub(super) async fn remember_turn(rows: &mut Indexes<'_>, turn: &TurnDeclaration) -> Result<(), Error> {
    rows.put(SourceFamily::HostRows, &turn_key(&turn.key.id()), turn).await
}

fn turn_key(turn: &fastctx_types::sync::identity::ObjectId) -> String {
    format!("claude/public-turn-declaration/{}", turn.as_str())
}
fn native(value: Option<&str>) -> Result<Option<NativeId>, Error> {
    value.map(NativeId::new).transpose().map_err(invalid)
}
fn error(error: std::io::Error) -> Error {
    super::reading::error(error)
}
fn tree_error(error: fastctx_sync::conversation::body_tree::BodyTreeError) -> Error {
    self::error(super::bodies::tree_error(error))
}
fn invalid(error: impl std::fmt::Display) -> Error {
    Error::Invalid(error.to_string())
}

pub(super) async fn context_ref(
    rows: &mut Indexes<'_>,
    uuid: Option<&str>,
) -> Result<Option<fastctx_types::sync::manifest::ContentRef>, Error> {
    match uuid {
        Some(uuid) => rows.get(SourceFamily::HostRows, &super::indexes::key("public-context", uuid)).await,
        None => Ok(None),
    }
}
