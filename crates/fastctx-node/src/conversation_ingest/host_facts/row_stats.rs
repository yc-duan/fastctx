//! Row-wide observations are scanned once, never once per thinking block.
use crate::conversation_ingest::{indexes::Indexes, step_content::StepContent};
use fastctx_hosts::claude::ingest::{
    document::{Value, read::Reader},
    jsonl::Row,
    semantic::{block::only_fields, children::Children, fields, message, prompt, row::Header},
};
use fastctx_sync::store::{Error, ingest_effects::SourceFamily};
use fastctx_types::sync::cursor::ByteCount;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub struct Stats {
    start: ByteCount,
    pub thinking: u64,
    pub hidden: bool,
    pub synthetic: bool,
}
fn key(io: &StepContent<'_>) -> String {
    format!("claude/public-row-stats/{}", io.target().step().primary().source.as_str())
}
pub async fn get(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    header: &Header,
    row: &Row,
    rows: &mut Indexes<'_>,
) -> Result<Stats, Error> {
    let key = key(io);
    if let Some(stats) = rows.get::<Stats>(SourceFamily::Consumers, &key).await? {
        if stats.start != row.start {
            return Err(super::invalid("public row statistics belong to another source row"));
        }
        return Ok(stats);
    }
    let mut stats = Stats { start: row.start, thinking: 0, hidden: false, synthetic: false };
    let mut first = None;
    let mut result = false;
    if let Some(content) = header.content {
        if let Some(mut children) = Children::of(reader, content).await.map_err(super::error)? {
            let count = children.remaining.0;
            while let Some((at, _)) = children.step(reader).await.map_err(super::error)? {
                let kind = fields::summary(reader, at, "type", 64).await.map_err(super::error)?;
                result |= kind.as_deref() == Some("tool_result");
                if matches!(kind.as_deref(), Some("thinking" | "redacted_thinking")) {
                    stats.thinking = stats.thinking.saturating_add(1);
                }
                if kind.as_deref() == Some("text")
                    && let Some(text) = message::string(reader, at, "text").await.map_err(super::error)?
                {
                    first = Some(text);
                    if header.kind == "assistant"
                        && count == 1
                        && only_fields(reader, at, &["type", "text"]).await.map_err(super::error)?
                    {
                        stats.synthetic = reader.text(text, 64).await.map_err(super::error)?.as_deref()
                            == Some("No response requested.");
                    }
                }
            }
        } else if let Value::String { text } = reader.node(content).await.map_err(super::error)?.value {
            first = Some(text);
        }
    }
    if stats.synthetic {
        stats.synthetic = match header.message {
            Some(message) => {
                fields::summary(reader, message, "model", 64).await.map_err(super::error)?.as_deref()
                    == Some("<synthetic>")
            }
            None => false,
        };
    }
    let excluded = match first {
        Some(text) => prompt::excluded_prompt(&fields::leading(reader, text, 64).await.map_err(super::error)?),
        None => false,
    };
    stats.hidden = stats.synthetic
        || matches!(header.kind.as_str(), "attachment" | "system" | "queue-operation")
        || header.kind == "user" && (!header.prompt_candidate || excluded) && !result;
    rows.put(SourceFamily::Consumers, &key, &stats).await?;
    Ok(stats)
}
pub async fn clear(io: &StepContent<'_>, rows: &mut Indexes<'_>) -> Result<(), Error> {
    if rows.get::<Stats>(SourceFamily::Consumers, &key(io)).await?.is_some() {
        rows.delete(SourceFamily::Consumers, &key(io)).await?;
    }
    Ok(())
}
