//! A conversation's own attachments, named by their ids: the receipt its home
//! keeps for each, which a command the conversation runs on any machine reads
//! the file by, as every attachment is read. Only the conversation's own are
//! found, and only for an account it works for (`crate::admission`); where the
//! file is kept does not matter, so an agent assisting the user, which may
//! not work on the machine it lives on, looks at what the user sent all the
//! same (`world-run.md`, 受协助的人附的文件).

use super::references::optional;
use super::*;

/// The receipt of the attachment `id` of `resident`, which lives here: one
/// kept here, one kept on another machine, or one of the conversation it was
/// forked from.
pub(crate) fn receipt(node: &Node, resident: &ResidentId, id: &fastctx_types::NoticeId) -> Result<Attachment, Failure> {
    let room = node.world.conversation(resident).attachments();
    for name in ["receipt.json", "reference.json"] {
        if let Some(kept) = optional::<Attachment>(&room.join(id.as_str()).join(name))? {
            return Ok(kept);
        }
    }
    optional::<Vec<Attachment>>(&room.join("inherited.json"))?
        .unwrap_or_default()
        .into_iter()
        .find(|kept| kept.id == *id)
        .ok_or_else(|| Failure::not_run(format!("this conversation has no attachment {id}")))
}

/// Whether the command at `peer` reads a file the user attached in the
/// conversation it runs for, as `fastctx view --attachment` does, which
/// only the user's interface reads otherwise.
pub(crate) fn of_its_own(
    node: &Node,
    peer: &fastctx_types::ipc::Peer,
    attachment: &fastctx_types::attachments::AttachmentSource,
) -> bool {
    let Ok(crate::town::caller::Caller::Resident(conversation)) = crate::town::caller::of(node, peer) else {
        return false;
    };
    let fastctx_types::attachments::AttachmentSource::Upload(receipt) = attachment else { return false };
    let of = receipt.conversation();
    of.resident == conversation.resident
        && (of.machine == conversation.machine || node.is_me(&of.machine) && node.is_me(&conversation.machine))
}

/// The receipt of the attachment `id` of the conversation the command at
/// `peer` runs for, asked of its home when that is another machine.
pub(crate) async fn asked(
    node: &Arc<Node>,
    peer: &fastctx_types::ipc::Peer,
    id: fastctx_types::NoticeId,
) -> Result<Attachment, Failure> {
    let caller = crate::town::caller::of(node, peer)?;
    let crate::town::caller::Caller::Resident(conversation) = &caller else {
        return Err(Failure::not_run("only a command a conversation runs looks at its attachments by id"));
    };
    if node.is_me(&conversation.machine) {
        return receipt(node, &conversation.resident, &id);
    }
    let acting = crate::around::acting_of(node, &caller);
    let call = RemoteCall::OwnAttachment { resident: conversation.resident.clone(), id: id.clone() };
    match node.call(&acting, &conversation.machine.clone().into(), Purpose::Agents, call).await {
        // A forked conversation's include those of the one it came from.
        Ok(Ok(RemoteReply::Uploaded(kept))) if kept.id == id => Ok(kept),
        Ok(Ok(_)) => {
            Err(Failure::new(format!("{} answered something other than the attachment", conversation.machine)))
        }
        Ok(Err(failure)) => Err(failure),
        Err(failure) => Err(crate::authorization::call_failure(failure)),
    }
}
