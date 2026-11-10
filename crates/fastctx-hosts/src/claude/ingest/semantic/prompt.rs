use super::{children::Children, fields, row::Header};
use crate::claude::ingest::document::{Span, Value, read::Reader};
use std::io;

pub fn excluded_prompt(first: &str) -> bool {
    [
        "<local-command-caveat>",
        "<local-command-stdout>",
        "<local-command-stderr>",
        "<command-name>",
        "<command-message>",
        "[Request interrupted by user",
    ]
    .iter()
    .any(|prefix| first.starts_with(prefix))
}
#[derive(Clone, Debug)]
pub struct Prompt {
    pub visible: bool,
    pub first: Option<Span>,
    pub has_tool_result: bool,
}
pub async fn prompt(reader: &Reader<'_>, header: &Header) -> io::Result<Prompt> {
    let mut first = None;
    let mut visible = false;
    let mut has_tool_result = false;
    if let Some(content) = header.content {
        match reader.node(content).await?.value {
            Value::String { text, .. } => {
                first = Some(text);
                visible = true;
            }
            Value::Array { .. } => {
                let mut children =
                    Children::of(reader, content).await?.ok_or_else(|| io::Error::other("missing content array"))?;
                while let Some((address, _)) = children.step(reader).await? {
                    let kind = fields::text(reader, address, "type").await?;
                    has_tool_result |= kind.as_deref() == Some("tool_result");
                    if kind.as_deref() == Some("image") {
                        visible = true;
                    }
                    if let Some((_, node)) = reader.field(address, "text").await?
                        && let Value::String { text, .. } = node.value
                    {
                        if kind.as_deref() == Some("text") {
                            visible = true;
                        }
                        if !fields::leading(reader, text, 64).await?.is_empty() {
                            first = Some(text);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let excluded =
        if let Some(first) = first { excluded_prompt(&fields::leading(reader, first, 64).await?) } else { false };
    Ok(Prompt { visible: visible && header.prompt_candidate && !has_tool_result && !excluded, first, has_tool_result })
}
