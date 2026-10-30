//! Between native turns, send changed named context with the next input. Only
//! its receipt is journaled; account bodies remain in memory until the host
//! accepts them into its own conversation record.
use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure};
use std::sync::Arc;

/// The caller holds the management read gate and the speech lock, and has
/// confirmed that neither the node nor the native host has a running turn.
pub(crate) async fn prepare(node: &Arc<Node>, id: &ResidentId) -> Result<(Vec<String>, super::Receipt), Failure> {
    let (about, town) = node.residents.about(id).ok_or_else(|| Failure::new("the conversation is unavailable"))?;
    super::refresh_local(node, id).map_err(|error| Failure::new(error.to_string()))?;
    super::sources::plan_guide(node, id, &about.origin, &about.acting)
        .await
        .map_err(|error| Failure::new(error.to_string()))?;
    node.plan_reads.invalidate(id);
    crate::plan::context(node, id, &about.origin, &about.acting).await?;
    super::sources::skills(node, id, &about.acting).await?;
    super::sources::map(node, id, town.as_ref(), &about.origin, &about.acting).await?;
    super::collect(node, id).map_err(|error| Failure::new(error.to_string()))
}

pub(crate) fn prepend(input: &mut fastctx_hosts::codex::Input, notes: &[String]) {
    if notes.is_empty() {
        return;
    }
    let prefix = notes.join("\n\n");
    use fastctx_hosts::codex::{Input, InputMessage};
    let context = InputMessage { text: prefix.clone(), images: Vec::new() };
    match input {
        Input::Text(text) => {
            *input = Input::Messages(vec![context, InputMessage { text: std::mem::take(text), images: Vec::new() }]);
        }
        Input::Messages(messages) => messages.insert(0, context),
        Input::Skill { text, .. } => text.insert_str(0, &format!("{prefix}\n\n")),
        Input::ToolOutput { output, .. } => output.insert_str(0, &format!("{prefix}\n\n")),
    }
}

pub(crate) fn accepted(node: &Node, id: &ResidentId, receipt: &super::Receipt) -> std::io::Result<()> {
    if let Some(accepted) = super::storage::acknowledge(&node.world, id, receipt)? {
        super::sources::observed(node, id, &accepted)?;
    }
    Ok(())
}
