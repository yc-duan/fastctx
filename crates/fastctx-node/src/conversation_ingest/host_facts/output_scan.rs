//! One result element per publication unit, retaining a signed prefix forest.
use super::{
    content, outputs,
    part_tree::{Builder, State},
};
use crate::conversation_ingest::{indexes::Indexes, step_content::StepContent};
use fastctx_hosts::claude::ingest::{
    document::{Address, Span, Value, read::Reader},
    jsonl::Row,
    semantic::{children::Children, lines::Lines},
};
use fastctx_sync::store::{Error, ingest_effects::SourceFamily};
use fastctx_types::sync::{
    conversation_timeline::*,
    cursor::{ByteCount, Count},
    manifest::ContentRef,
};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
struct Scan {
    start: ByteCount,
    block: Count,
    children: Children,
    forest: Option<State>,
    head: Option<Span>,
}
pub async fn advance(
    io: &StepContent<'_>,
    rows: &mut Indexes<'_>,
    reader: &Reader<'_>,
    row: &Row,
    block: Count,
    at: Option<Address>,
) -> Result<outputs::Output, Error> {
    let key = format!(
        "claude/public-output/{}/{:016x}/{:016x}",
        io.target().step().primary().source.as_str(),
        row.start.0,
        block.0
    );
    let old = rows.get::<Scan>(SourceFamily::Consumers, &key).await?;
    let existed = old.is_some();
    let mut scan = match old {
        Some(scan) => scan,
        None => {
            let Some(at) = at else { return outputs::header(io, reader, None, None).await };
            match reader.node(at).await.map_err(super::error)?.value {
                Value::Array { .. } => Scan {
                    start: row.start,
                    block,
                    children: Children::of(reader, at).await.map_err(super::error)?.ok_or(Error::Busy)?,
                    forest: None,
                    head: None,
                },
                Value::String { text } => {
                    let part = FactPartV1::Output {
                        output: ToolOutputV1::Text { text: content::text(io, reader, Some(text)).await? },
                    };
                    let reference = content::publish(
                        io,
                        ItemFactV1::Parts { part_kind: FactPartKindV1::Output, start: Count(0), parts: vec![part] },
                    )
                    .await?;
                    return outputs::header(
                        io,
                        reader,
                        Some(reference),
                        Lines::new(text).next(reader).await.map_err(super::error)?,
                    )
                    .await;
                }
                Value::Null => return outputs::header(io, reader, None, None).await,
                _ => return Err(super::invalid("native tool output is not a supported string or array")),
            }
        }
    };
    if scan.start != row.start || scan.block != block {
        return Err(super::invalid("tool output cursor belongs to another block"));
    }
    let mut builder = match scan.forest.take() {
        Some(state) => Builder::resume(io, state),
        None => Builder::new(io, Some(FactPartKindV1::Output), true),
    };
    if let Some((at, node)) = scan.children.step(reader).await.map_err(super::error)? {
        let (output, text) = outputs::block(io, reader, at).await?;
        if let Some(text) = text
            && let Some(line) = Lines::new(text).next(reader).await.map_err(super::error)?
        {
            scan.head = Some(line);
        }
        builder.push(node.ordinal, FactPartV1::Output { output }).await?;
    }
    let parts: Option<ContentRef> = if scan.children.remaining.0 > 0 {
        let state = builder.checkpoint().await?;
        let prefix = Builder::resume(io, state.clone()).finish().await?;
        scan.forest = Some(state);
        rows.put(SourceFamily::Consumers, &key, &scan).await?;
        prefix
    } else {
        let result = builder.finish().await?;
        if existed {
            rows.delete(SourceFamily::Consumers, &key).await?;
        }
        result
    };
    outputs::header(io, reader, parts, scan.head).await
}
