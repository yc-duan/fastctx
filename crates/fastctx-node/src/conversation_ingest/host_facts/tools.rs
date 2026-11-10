//! Call and result parts retain both native details across out-of-order arrivals.
use super::{actions, content, forward_list::ForwardList, output_scan};
use crate::conversation_ingest::step_content::StepContent;
use fastctx_hosts::claude::ingest::{
    document::read::Reader,
    jsonl::Row,
    semantic::{
        block::{Block, Kind},
        fields,
        message::string,
        row::Header,
        tool,
    },
};
use fastctx_sync::store::Error;
use fastctx_types::{
    session::{AgentAct, ToolStatus},
    sync::{conversation_timeline::*, cursor::Count},
};

pub struct PairInfo {
    pub has_result: bool,
    pub failed: bool,
    pub ambiguous: bool,
    pub continuing: bool,
}
pub struct ToolBlock<'a, 'document> {
    pub reader: &'a Reader<'document>,
    pub header: &'a Header,
    pub row: &'a Row,
    pub block: &'a Block,
    pub identity: &'a ItemIdentity,
}
pub async fn attach(
    io: &StepContent<'_>,
    rows: &mut crate::conversation_ingest::indexes::Indexes<'_>,
    observed: ToolBlock<'_, '_>,
    info: PairInfo,
    previous: Option<&ItemDisplayV1>,
    display: &mut ItemDisplayV1,
) -> Result<(), Error> {
    let ToolBlock { reader, header, row, block, identity } = observed;
    let mut facts = previous.and_then(|display| display.facts.as_deref()).cloned().unwrap_or_default();
    facts.tool_status = Some(if info.ambiguous || !info.has_result {
        ToolStatus::Unknown
    } else if info.failed {
        ToolStatus::Failed
    } else {
        ToolStatus::Done
    });
    facts.duration_ms = None;
    if info.continuing && info.ambiguous {
        display.facts = Some(Box::new(facts));
        return Ok(());
    }
    if info.ambiguous {
        facts.at = header.at;
        facts.started_at = None;
        facts.tokens = None;
        facts.semantic = None;
        facts.event = Some(
            content::publish(
                io,
                ItemFactV1::Session {
                    event: SessionFactV1::Diagnostic {
                        message: content::literal(io, "ambiguous native tool call/result pair").await?,
                        detail: Some(content::raw(io, reader, block.raw).await?),
                    },
                },
            )
            .await?,
        );
        display.facts = Some(Box::new(facts));
        return Ok(());
    }
    let mut value = match &facts.semantic {
        Some(reference) => content::load(io, reference).await?.fact,
        None => ItemFactV1::Tool {
            name: None,
            server: None,
            machine: None,
            source: None,
            does: None,
            input: None,
            output: None,
            structured: None,
            detail: content::publish(
                io,
                ItemFactV1::ToolDetail { uuid: None, requested: None, requested_machine: None, stored: None },
            )
            .await?,
        },
    };
    let ItemFactV1::Tool { name, server, machine, source, does, input, output, structured, detail } = &mut value else {
        return Err(super::invalid("native tool has a non-tool public descriptor"));
    };
    let mut native_detail = content::load(io, detail).await?.fact;
    let ItemFactV1::ToolDetail { uuid, requested, requested_machine, stored } = &mut native_detail else {
        return Err(super::invalid("native tool detail has another public variant"));
    };
    match &block.kind {
        Kind::ToolUse { input: arguments, fields: arguments_at, .. } => {
            let named = tool::name(reader, block.address).await.map_err(super::error)?.ok_or(Error::Busy)?;
            *name = Some(content::text(io, reader, Some(named.name)).await?);
            *server = content::optional_text(io, reader, named.server).await?;
            *input = Some(content::raw(io, reader, *arguments).await?);
            *source = Some(if named.world {
                ToolSourceV1::World
            } else if named.server.is_some() {
                ToolSourceV1::Mcp
            } else {
                ToolSourceV1::Native
            });
            *does = Some(Box::new(actions::build(io, reader, &named, *arguments_at).await?));
            *requested_machine = None;
            if named.world && named.known.as_deref() != Some("run") {
                *requested_machine = match string(reader, *arguments_at, "node").await.map_err(super::error)? {
                    Some(span) => {
                        reader.text(span, 256).await.map_err(super::error)?.and_then(|name| name.parse().ok())
                    }
                    None => None,
                };
            }
            *uuid = super::native(header.uuid.as_deref())?;
            *requested = Some(content::raw(io, reader, block.raw).await?);
            facts.started_at = header.at;
            facts.tokens = fastctx_hosts::claude::ingest::semantic::message::decode(reader, header)
                .await
                .map_err(super::error)?
                .tokens;
            if !info.has_result {
                facts.at = header.at;
            }
            facts.native = Some(
                content::publish(
                    io,
                    ItemFactV1::Native {
                        identity: identity.clone(),
                        draft: None,
                        source: io.target().step().primary().source.clone(),
                        generation: io.target().step().generation().clone(),
                        line: Some(Count(row.line.0.checked_add(1).ok_or(Error::Busy)?)),
                        block: Some(block.ordinal),
                        response: super::native(header.response.as_deref())?,
                        context: super::context_ref(rows, header.uuid.as_deref()).await?,
                    },
                )
                .await?,
            );
            if let Some(summary) = &mut display.tool {
                summary.name = fields::prefix(reader, named.name, DISPLAY_LABEL_BYTES).await.map_err(super::error)?;
                summary.server = match named.server {
                    Some(span) => Some(fields::prefix(reader, span, DISPLAY_LABEL_BYTES).await.map_err(super::error)?),
                    None => None,
                };
                summary.machine = machine.as_ref().map(ToString::to_string);
                summary.does.kind = match does.as_deref() {
                    Some(ToolActionV1::Command { .. }) => ActionKind::Command,
                    Some(ToolActionV1::Edit { .. }) => ActionKind::Edit,
                    Some(ToolActionV1::Search { .. }) => ActionKind::Search,
                    Some(ToolActionV1::Image { .. }) => ActionKind::Image,
                    Some(ToolActionV1::Colleague { .. }) => ActionKind::Colleague,
                    Some(ToolActionV1::Agent { .. }) => ActionKind::Agent,
                    _ => ActionKind::Call,
                };
            }
        }
        Kind::ToolResult { content: result, .. } => {
            let normalized = output_scan::advance(io, rows, reader, row, block.ordinal, *result).await?;
            *output = normalized.parts;
            if !info.continuing {
                *structured = match reader.field(reader.document.root, "toolUseResult").await.map_err(super::error)? {
                    Some((_, node)) => Some(content::raw(io, reader, node.raw).await?),
                    None => None,
                };
                let result_at =
                    fields::object(reader, reader.document.root, "toolUseResult").await.map_err(super::error)?;
                let agent = match result_at {
                    Some(at) => match string(reader, at, "agentId").await.map_err(super::error)? {
                        Some(span)
                            if reader
                                .text(span, 128)
                                .await
                                .map_err(super::error)?
                                .is_some_and(|id| fastctx_hosts::claude::subagents::valid_id(&id)) =>
                        {
                            Some(content::text(io, reader, Some(span)).await?)
                        }
                        _ => None,
                    },
                    None => None,
                };
                *stored = Some(ToolResultDetailV1 {
                    uuid: super::native(header.uuid.as_deref())?,
                    block: content::raw(io, reader, block.raw).await?,
                    structured: structured.clone(),
                    source_assistant: super::native(header.source_assistant.as_deref())?,
                    machine: normalized.machine,
                    colleague: normalized.colleague,
                    agent,
                });
            } else if let Some(stored) = stored {
                stored.machine = normalized.machine;
                stored.colleague = normalized.colleague;
            }
            facts.at = header.at;
        }
        _ => return Err(super::invalid("non-tool block reached native tool facts")),
    }
    // Header-derived names do not apply to arbitrary tools with similar output.
    let world = matches!(source, Some(ToolSourceV1::World));
    *machine = if world && display.tool.as_ref().is_some_and(|tool| tool.name != "run") {
        requested_machine.clone().or_else(|| stored.as_ref().and_then(|stored| stored.machine.clone()))
    } else {
        None
    };
    if let Some(stored) = stored {
        if world
            && let Some(ToolActionV1::Colleague { to, .. }) = does.as_deref_mut()
            && display.tool.as_ref().is_some_and(|tool| tool.name == "ask_colleague")
        {
            *to = stored.colleague.clone();
        }
        if let Some(ToolActionV1::Agent { act: AgentAct::Start, agents, .. }) = does.as_deref_mut() {
            *agents = if let Some(agent) = &stored.agent {
                let mut list = ForwardList::new(io, FactPartKindV1::Text);
                list.push(FactPartV1::Text { text: agent.clone() }).await?;
                Some(list.finish().await?)
            } else {
                None
            };
        }
    }
    if let Some(summary) = &mut display.tool {
        summary.machine = machine.as_ref().map(ToString::to_string);
    }
    *detail = content::publish(io, native_detail).await?;
    facts.semantic = Some(content::publish(io, value).await?);
    display.facts = Some(Box::new(facts));
    Ok(())
}
