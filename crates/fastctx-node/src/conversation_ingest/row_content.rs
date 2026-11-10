//! Stable message blocks and cross-row tool pairing. Results revise the invocation
//! item; missing invocations remain unplaced until their native evidence arrives.
use super::{
    claim::Claim,
    display_append, indexes, item_keys, parts,
    publisher::{block_key, identity as identity_error, item_key, observed},
    render,
    row_state::*,
    step_content::StepContent,
};
use fastctx_hosts::claude::ingest::{
    document::{Address, Value, read::Reader},
    jsonl::Row,
    semantic::{
        block::{self, Kind},
        row::Header,
    },
};
use fastctx_sync::{
    cursor::Count,
    store::{Error, ingest_effects::SourceFamily},
};
use fastctx_types::sync::{
    conversation_identity::{ImageSelector, NativeId},
    conversation_timeline::*,
};
use serde::{Deserialize, Serialize};

pub(super) async fn content(
    claim: &Claim,
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    header: &Header,
    row: &Row,
    local: Local<'_, '_>,
    block: &block::Block,
) -> Result<Option<TimelineFact>, Error> {
    let Local { rows, clock, work } = local;
    let uuid = header.uuid.as_deref().ok_or(Error::Busy)?;
    let selector =
        ImageSelector::ClaudeMessage { uuid: NativeId::new(uuid).map_err(identity_error)?, image: work.images };
    if matches!(block.kind, Kind::Image { .. }) {
        work.images.0 += 1;
    }
    if work.visible {
        let identity = item_keys::user(&claim.conversation, uuid).map_err(identity_error)?;
        let old = rows.get::<Item>(SourceFamily::HostRows, &item_key(&identity.id())).await?;
        let mut display = if work.prompt_emitted {
            display_append::load(io, &old.ok_or_else(|| Error::Corrupt("partial prompt lost its item".into()))?.display)
                .await?
        } else {
            render::empty(DisplayKind::User, None, DisplayStatus::Pending)
        };
        if let Some(part) = render::block(io, reader, block, selector, DisplayChannel::Text).await? {
            display_append::append(io, &mut display, part).await?;
        }
        let complete = work.next.0 + 1 == work.total.0;
        display.status = Some(if complete { DisplayStatus::Complete } else { DisplayStatus::Pending });
        work.prompt_emitted = true;
        return Ok(Some(observed(io, row, Local::borrow(rows, clock, work), identity, display, complete).await?));
    }
    match &block.kind {
        Kind::Text { .. } | Kind::Thinking { .. } | Kind::RedactedThinking if header.kind == "assistant" => {
            let thinking = matches!(block.kind, Kind::Thinking { .. } | Kind::RedactedThinking);
            let mut display = render::empty(
                if thinking { DisplayKind::Thinking } else { DisplayKind::Message },
                None,
                DisplayStatus::Complete,
            );
            if let Some(part) = render::block(
                io,
                reader,
                block,
                selector,
                if thinking { DisplayChannel::Thinking } else { DisplayChannel::Text },
            )
            .await?
            {
                display_append::append(io, &mut display, part).await?;
            }
            Ok(Some(
                observed(
                    io,
                    row,
                    Local::borrow(rows, clock, work),
                    item_keys::assistant_block(&claim.conversation, uuid, block.ordinal).map_err(identity_error)?,
                    display,
                    true,
                )
                .await?,
            ))
        }
        Kind::ToolUse { .. } if header.kind == "assistant" => {
            tool(claim, io, reader, header, row, Local::borrow(rows, clock, work), block).await.map(Some)
        }
        Kind::ToolResult { .. } if header.kind == "user" => {
            tool(claim, io, reader, header, row, Local::borrow(rows, clock, work), block).await.map(Some)
        }
        _ => Ok(None),
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Pair {
    call: Option<Occurrence>,
    result: Option<Occurrence>,
    duplicate: bool,
    source_assistant: Option<String>,
    invalid_source: bool,
    failed: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Occurrence {
    uuid: String,
    block: Count,
}
impl Pair {
    fn ambiguous(&self) -> bool {
        self.duplicate
            || self.invalid_source
            || self.source_assistant.as_ref().is_some_and(|s| self.call.as_ref().is_some_and(|c| &c.uuid != s))
    }
}
async fn tool(
    claim: &Claim,
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    header: &Header,
    row: &Row,
    local: Local<'_, '_>,
    block: &block::Block,
) -> Result<TimelineFact, Error> {
    let Local { rows, clock, work } = local;
    let is_call = matches!(block.kind, Kind::ToolUse { .. });
    let pairing = block.pairing.as_ref().ok_or(Error::Busy)?;
    let identity = item_keys::tool(&claim.conversation, &pairing.id).map_err(identity_error)?;
    let old = rows.get::<Item>(SourceFamily::HostRows, &item_key(&identity.id())).await?;
    let pair_key = indexes::key("tool", &pairing.id);
    let mut pair = rows.get::<Pair>(SourceFamily::HostRows, &pair_key).await?.unwrap_or_default();
    let occurrence = Occurrence { uuid: header.uuid.clone().ok_or(Error::Busy)?, block: block.ordinal };
    let entry = if is_call { &mut pair.call } else { &mut pair.result };
    pair.duplicate |= entry.as_ref().is_some_and(|old| old != &occurrence);
    *entry = Some(occurrence);
    if !is_call {
        pair.source_assistant = header.source_assistant.clone();
        pair.invalid_source = header.source_assistant_invalid;
        if let Kind::ToolResult { failed, .. } = block.kind {
            pair.failed = failed;
        }
    }
    let previous = match old {
        Some(old) => Some(display_append::load(io, &old.display).await?),
        None => None,
    };
    let mut display = if is_call {
        let Kind::ToolUse { name, input, fields } = &block.kind else { return Err(Error::Busy) };
        let mut display =
            render::empty(DisplayKind::Tool, Some(render::tool(reader, name, *fields).await?), DisplayStatus::Running);
        let part = parts::text(io, &reader.document.bodies.raw, *input, Count(0), DisplayChannel::ToolInput, 4096)
            .await
            .map_err(super::reading::error)?;
        display_append::append(io, &mut display, part).await?;
        if let Some(previous) = &previous {
            for index in 0..display_append::count(previous) {
                let part = display_append::part(io, previous, Count(index)).await?;
                if part.channel != DisplayChannel::ToolInput {
                    display_append::append(io, &mut display, part).await?;
                }
            }
        }
        display
    } else if work.nested.as_ref().is_some_and(|n| n.next.0 > 0) {
        previous.clone().ok_or_else(|| Error::Corrupt("partial tool result lost its display".into()))?
    } else {
        let summary = previous.as_ref().and_then(|p| p.tool.clone()).unwrap_or(ToolSummary {
            name: "tool".into(),
            server: None,
            machine: None,
            does: ActionSummary { kind: ActionKind::Call, text: None, command: None, path: None },
        });
        let mut display = render::empty(DisplayKind::Tool, Some(summary), DisplayStatus::Pending);
        if let Some(previous) = &previous {
            for index in 0..display_append::count(previous) {
                let part = display_append::part(io, previous, Count(index)).await?;
                if part.channel == DisplayChannel::ToolInput {
                    display_append::append(io, &mut display, part).await?;
                }
            }
        }
        display
    };
    if let Kind::ToolResult { content: Some(content), .. } = &block.kind {
        let value = reader.node(*content).await.map_err(super::reading::error)?;
        if let Value::Array { members, .. } = value.value {
            if work.nested.is_none() {
                work.nested = Some(Nested {
                    parent: block.ordinal,
                    address: *content,
                    next: Count(0),
                    total: members,
                    images: Count(0),
                });
            }
            let nested = work.nested.as_mut().ok_or(Error::Busy)?;
            if nested.next < nested.total {
                let key = nested_key(claim, row, block.ordinal.0, nested.next.0);
                let address = rows
                    .get::<Address>(SourceFamily::HostRows, &key)
                    .await?
                    .ok_or_else(|| Error::Corrupt("nested result cursor is absent".into()))?;
                let child = block::decode(reader, address).await.map_err(super::reading::error)?;
                let selector = ImageSelector::ClaudeTool {
                    tool_use_id: NativeId::new(&pairing.id).map_err(identity_error)?,
                    image: nested.images,
                };
                if matches!(child.kind, Kind::Image { .. }) {
                    nested.images.0 += 1;
                }
                if let Some(part) = render::block(io, reader, &child, selector, DisplayChannel::ToolOutput).await? {
                    display_append::append(io, &mut display, part).await?;
                }
                rows.delete(SourceFamily::HostRows, &key).await?;
                nested.next.0 += 1;
            }
            if nested.next >= nested.total {
                work.nested = None;
            }
        } else if let Some(part) = render::scalar(io, reader, *content, DisplayChannel::ToolOutput).await? {
            display_append::append(io, &mut display, part).await?;
        }
    }
    display.status = Some(if pair.ambiguous() {
        DisplayStatus::Failed
    } else if work.nested.is_some() || pair.call.is_none() {
        DisplayStatus::Pending
    } else if pair.result.is_none() {
        DisplayStatus::Running
    } else if pair.failed {
        DisplayStatus::Failed
    } else {
        DisplayStatus::Complete
    });
    if pair.ambiguous() {
        display.kind = DisplayKind::Unknown;
        display.tool = None;
    }
    rows.put(SourceFamily::HostRows, &pair_key, &pair).await?;
    let placement = work.placement.clone();
    if pair.call.is_none() {
        work.placement = None;
    }
    let complete = work.nested.is_none();
    let result = observed(io, row, Local::borrow(rows, clock, work), identity, display, complete).await;
    work.placement = placement;
    result
}
pub(super) fn nested_key(claim: &Claim, row: &Row, parent: u64, index: u64) -> String {
    format!("{}/result/{index:016x}", block_key(claim, row, parent))
}
