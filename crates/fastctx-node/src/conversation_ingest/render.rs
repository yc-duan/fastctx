//! Host-independent descriptors from selected, validated host fields.
use super::{parts, step_content::StepContent};
use fastctx_hosts::claude::ingest::{
    document::{Address, Value, read::Reader},
    image::ImageSource,
    semantic::{
        block::{Block, Kind},
        fields,
    },
};
use fastctx_sync::store::Error;
use fastctx_types::sync::{conversation_identity::ImageSelector, conversation_timeline::*, cursor::Count};

pub fn empty(kind: DisplayKind, tool: Option<ToolSummary>, status: DisplayStatus) -> ItemDisplayV1 {
    ItemDisplayV1 {
        version: 1,
        kind,
        tool,
        status: Some(status),
        facts: None,
        append: None,
        parts: Vec::new(),
        more: None,
    }
}
pub async fn block(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    block: &Block,
    selector: ImageSelector,
    channel: DisplayChannel,
) -> Result<Option<DisplayPart>, Error> {
    let part = match &block.kind {
        Kind::Text { body } | Kind::Thinking { body } => Some(
            parts::text(io, &reader.document.bodies.strings, *body, Count(0), channel, 4096)
                .await
                .map_err(super::reading::error)?,
        ),
        Kind::Image { block, .. } => {
            let source = ImageSource::from_block(reader, *block)
                .await
                .map_err(super::reading::error)?
                .ok_or_else(|| Error::Invalid("indexed image lost its source".into()))?;
            match parts::image(io, reader, &source, Count(0), selector).await {
                Ok(part) => Some(part),
                // The exact external value remains in typed tool/native detail.
                Err(error) if fastctx_hosts::claude::ingest::image::is_invalid_data(&error) => None,
                Err(error) => return Err(super::reading::error(error)),
            }
        }
        _ => None,
    };
    Ok(part)
}
pub async fn scalar(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    address: Address,
    channel: DisplayChannel,
) -> Result<Option<DisplayPart>, Error> {
    match reader.node(address).await.map_err(super::reading::error)?.value {
        Value::String { text } => Ok(Some(
            parts::text(io, &reader.document.bodies.strings, text, Count(0), channel, 4096)
                .await
                .map_err(super::reading::error)?,
        )),
        _ => Ok(None),
    }
}
pub async fn tool(reader: &Reader<'_>, name: &str, fields: Address) -> Result<ToolSummary, Error> {
    let command = fields::summary(reader, fields, "command", 1024).await.map_err(super::reading::error)?;
    let path = fields::summary(reader, fields, "file_path", 1024).await.map_err(super::reading::error)?;
    let text = fields::summary(reader, fields, "description", 1024).await.map_err(super::reading::error)?;
    let machine = fields::summary(reader, fields, "on", 256).await.map_err(super::reading::error)?;
    Ok(ToolSummary {
        name: display_preview(name, 256),
        server: None,
        machine,
        does: ActionSummary {
            kind: if command.is_some() {
                ActionKind::Command
            } else if path.is_some() {
                ActionKind::Read
            } else {
                ActionKind::Call
            },
            text,
            command,
            path,
        },
    })
}
