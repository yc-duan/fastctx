//! What a session tells the user (`world-voice.md`). The words themselves
//! stay in the conversation's record as the `message_user` call; the node
//! keeps the latest as the conversation's `told`, which notifications and
//! the project's page read. A colleague's user is the agent that asked it
//! (`world-colleagues.md`, 两个工具): what it tells goes there instead.

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Delivery, Notice, Origin, Owing, TOLD_CHARS, TellStatus, Told};
use fastctx_types::session::Speaker;
use fastctx_types::voice::{Attachment, ColleagueTold};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// Whether `id` speaks through `message_user`: a conversation the World
/// started, for the user or as a colleague. A session the user opened in a
/// host's own window is read there.
pub(crate) fn speaks(node: &Node, id: &ResidentId) -> bool {
    node.residents.removed(id).is_none()
        && node.residents.about(id).map_or_else(
            || crate::here::born_for_user(node, id),
            |(about, _)| about.world_started && matches!(about.origin, Origin::User | Origin::Colleague { .. }),
        )
}

/// Keeps `message` as what `id` last told the user, or tells a colleague's
/// asker.
pub(crate) async fn say(
    node: &Arc<Node>,
    id: &ResidentId,
    message: &str,
    status: TellStatus,
    attachments: &[Attachment],
) -> Result<(), Failure> {
    if !speaks(node, id) {
        return Err(Failure::not_run("only a conversation the World started speaks with message_user"));
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
    if let Some((about, _)) = node.residents.about(id)
        && let Origin::Colleague { asked_by, label } = about.origin
    {
        return tell_asker(node, id, asked_by, label, message, attachments).await;
    }
    let at = Timestamp::now();
    let told = Told {
        id: format!("told-{}", at.as_nanosecond()),
        at,
        status,
        text: message.chars().take(TOLD_CHARS).collect(),
        turn: node.residents.now(id).and_then(|now| now.turn),
    };
    let (keeping, kept) = (node.clone(), id.clone());
    fastctx_platform::task::blocking(move || {
        keeping.residents.change_now(&keeping.world, &kept, |now| now.told = Some(told))
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?;
    node.log.line(format_args!("voice {id}: told the user ({status:?}, {} attached)", attachments.len()));
    Ok(())
}

/// What a colleague tells the agent that asked it: owed to it as the
/// colleague's report is, so that it reaches it on whichever machine it
/// lives, wakes it, and is not lost to a restart on either side.
async fn tell_asker(
    node: &Arc<Node>,
    id: &ResidentId,
    asker: Address,
    label: String,
    message: &str,
    attachments: &[Attachment],
) -> Result<(), Failure> {
    let colleague = Address { machine: node.machine(), resident: id.clone() };
    let told = ColleagueTold { colleague: colleague.clone(), label, attachments: attachments.to_vec() };
    let owing = Owing {
        to: asker,
        delivery: Delivery {
            id: crate::speech::new_id("told"),
            at: Timestamp::now(),
            notice: Notice::Words {
                from: Speaker::Resident { address: colleague },
                text: message.to_string(),
                goal_report: None,
                goal_question: None,
                told: Some(Box::new(told)),
            },
        },
    };
    let directory = node.world.conversation(id);
    let keeping = directory.clone();
    fastctx_platform::task::blocking(move || crate::inbox::owe(&keeping, &owing))
        .await
        .map_err(|error| Failure::new(format!("keeping what it tells: {error}")))?;
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn(async move { crate::turns::pay(&node, &id, &directory).await });
    Ok(())
}
