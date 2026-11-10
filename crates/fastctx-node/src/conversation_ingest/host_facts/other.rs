//! Unknown native blocks remain inspectable without interpreting original JSONL.
use super::content;
use crate::conversation_ingest::{render, step_content::StepContent};
use fastctx_hosts::claude::ingest::{
    document::read::Reader,
    semantic::{block::Block, message::string},
};
use fastctx_sync::store::Error;
use fastctx_types::sync::conversation_timeline::*;
pub async fn block(io: &StepContent<'_>, reader: &Reader<'_>, block: &Block) -> Result<ItemDisplayV1, Error> {
    let host_kind = match string(reader, block.address, "type").await.map_err(super::error)? {
        Some(span) => content::text(io, reader, Some(span)).await?,
        None => content::literal(io, "block").await?,
    };
    let mut display = render::empty(DisplayKind::Unknown, None, DisplayStatus::Complete);
    display.facts = Some(Box::new(DisplayFactsV1 {
        event: Some(
            content::publish(
                io,
                ItemFactV1::Session {
                    event: SessionFactV1::Other { host_kind, detail: content::raw(io, reader, block.raw).await? },
                },
            )
            .await?,
        ),
        ..Default::default()
    }));
    Ok(display)
}

pub async fn diagnostic(
    io: &StepContent<'_>,
    row: &fastctx_hosts::claude::ingest::jsonl::Row,
    identity: &ItemIdentity,
) -> Result<ItemDisplayV1, Error> {
    use fastctx_hosts::claude::ingest::{
        document::Completion,
        semantic::row::{self, RowKind},
    };
    let (message, at, host_kind) = match &row.content {
        Completion::Invalid { reason, .. } => (reason.clone(), None, None),
        Completion::Valid(document) => {
            let input = crate::conversation_ingest::bodies::Bodies { content: io };
            let reader = Reader { document, input: &input };
            match row::header(&reader, None).await.map_err(super::error)? {
                RowKind::Row(header)
                    if header.problem.is_none() && !matches!(header.kind.as_str(), "user" | "assistant") =>
                {
                    (
                        String::new(),
                        header.at,
                        Some(
                            content::text(
                                io,
                                &reader,
                                string(&reader, document.root, "type").await.map_err(super::error)?,
                            )
                            .await?,
                        ),
                    )
                }
                RowKind::Row(header) => (
                    header.problem.clone().unwrap_or_else(|| "native record has no supported identity".into()),
                    header.at,
                    None,
                ),
                RowKind::Invalid { reason, .. } => (reason, None, None),
                RowKind::Unindexed => ("native row exceeded its semantic indexing budget".into(), None, None),
                _ => ("unrecognized native record".into(), None, None),
            }
        }
    };
    let detail = crate::conversation_ingest::publisher::original(row).clone();
    let (status, event) = match host_kind {
        Some(host_kind) => (DisplayStatus::Complete, SessionFactV1::Other { host_kind, detail }),
        None => (
            DisplayStatus::Failed,
            SessionFactV1::Diagnostic { message: content::literal(io, &message).await?, detail: Some(detail) },
        ),
    };
    let mut display = crate::conversation_ingest::render::empty(DisplayKind::Unknown, None, status);
    display.facts = Some(Box::new(DisplayFactsV1 {
        at,
        started_at: at,
        native: Some(
            content::publish(
                io,
                ItemFactV1::Native {
                    identity: identity.clone(),
                    draft: None,
                    source: io.target().step().primary().source.clone(),
                    generation: io.target().step().generation().clone(),
                    line: Some(fastctx_types::sync::cursor::Count(row.line.0.checked_add(1).ok_or(Error::Busy)?)),
                    block: None,
                    response: None,
                    context: None,
                },
            )
            .await?,
        ),
        event: Some(content::publish(io, ItemFactV1::Session { event }).await?),
        ..Default::default()
    }));
    Ok(display)
}
