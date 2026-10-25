//! What a session tells the user (`world-voice.md`). The words themselves
//! stay in the conversation's record as the `message_user` call; the node
//! keeps the latest as the conversation's `told`, which notifications and
//! the project's page read.

use std::sync::Arc;

use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Origin, TOLD_CHARS, TellStatus, Told};
use fastctx_types::voice::Attachment;
use jiff::Timestamp;

use crate::Node;

/// Whether `id` speaks to the user through `message_user`: a conversation
/// the World started for the user. A colleague answers whoever asked for it,
/// and a session the user opened in a host's own window is read there.
pub(crate) fn speaks(node: &Node, id: &ResidentId) -> bool {
    node.residents.removed(id).is_none()
        && node
            .residents
            .about(id)
            .map_or_else(
                || crate::here::born_for_user(node, id),
                |(about, _)| about.world_started && matches!(about.origin, Origin::User),
            )
}

/// Keeps `message` as what `id` last told the user.
pub(crate) fn say(
    node: &Arc<Node>,
    id: &ResidentId,
    message: &str,
    status: TellStatus,
    attachments: &[Attachment],
) -> Result<(), Failure> {
    if !speaks(node, id) {
        return Err(Failure::not_run("only a conversation the World started for the user speaks to them"));
    }
    if message.trim().is_empty() {
        return Err(Failure::not_run("the message is empty"));
    }
    if attachments.len() > fastctx_types::voice::ATTACHMENTS_MOST {
        return Err(Failure::not_run(format!("at most {} attachments", fastctx_types::voice::ATTACHMENTS_MOST)));
    }
    if attachments.iter().any(|attachment| attachment.machine.trim().is_empty() || attachment.path.trim().is_empty()) {
        return Err(Failure::not_run("each attachment names its machine and its path"));
    }
    let at = Timestamp::now();
    let told = Told {
        id: format!("told-{}", at.as_nanosecond()),
        at,
        status,
        text: message.chars().take(TOLD_CHARS).collect(),
        turn: node.residents.now(id).and_then(|now| now.turn),
    };
    node.residents
        .change_now(&node.world, id, |now| now.told = Some(told))
        .map_err(|error| Failure::new(error.to_string()))?;
    node.log.line(format_args!("voice {id}: told the user ({status:?}, {} attached)", attachments.len()));
    Ok(())
}
