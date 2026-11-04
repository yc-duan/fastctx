//! What a session tells the user (`world-voice.md`). The words themselves
//! stay in the conversation's record as the `message_user` call; the node
//! keeps the latest as the conversation's `told`, which notifications and
//! the project's page read. `message_user` reaches the person in every
//! conversation, a colleague's too; a colleague tells the agent that asked
//! it with `message_colleague` (`world-colleagues.md`, 两个工具).

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Delivery, Notice, Origin, Owing, TOLD_CHARS, TellStatus, Told};
use fastctx_types::session::Speaker;
use fastctx_types::voice::{Attachment, ColleagueTold};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// Whether `id` speaks through `message_user`: a conversation the World
/// started, for the user or as a colleague, and one it is starting, whose
/// host lists its tools before its record is written. A session the user
/// opened in a host's own window is read there.
pub(crate) fn speaks(node: &Node, id: &ResidentId) -> bool {
    node.residents.removed(id).is_none()
        && node.residents.about(id).map_or_else(
            || crate::here::being_born(node, id),
            |(about, _)| about.world_started && matches!(about.origin, Origin::User | Origin::Colleague { .. }),
        )
}

/// Keeps `message` as what `id` last told the user.
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

/// What `id` asked without stopping: to the agent it works for, when it is
/// a colleague, otherwise to the user, as something that can wait until
/// they look.
pub(crate) async fn asked_without_stopping(node: &Arc<Node>, id: &ResidentId, said: &str) -> Result<(), Failure> {
    if asker_of(node, id).is_some() {
        return tell_asker(node, id, said, &[]).await.map(|_| ());
    }
    say(node, id, said, TellStatus::Info, &[]).await
}

/// The agent the colleague `id` was asked by, and the task it was asked
/// for; none for a conversation that is no colleague.
pub(crate) fn asker_of(node: &Node, id: &ResidentId) -> Option<(Address, String)> {
    match node.residents.about(id)?.0.origin {
        Origin::Colleague { asked_by, label } => Some((asked_by, label)),
        Origin::User => None,
    }
}

/// What a colleague tells the agent that asked it, with `message_colleague`
/// or by asking without stopping: owed to it as the colleague's answer is,
/// so that it reaches it on whichever machine it lives, wakes it, and is not
/// lost to a restart on either side. It owes nothing back: the turn it
/// wakes there answers no one, so the two never talk on and on by
/// themselves. Where it goes.
pub(crate) async fn tell_asker(
    node: &Arc<Node>,
    id: &ResidentId,
    message: &str,
    attachments: &[Attachment],
) -> Result<Address, Failure> {
    let (asker, label) =
        asker_of(node, id).ok_or_else(|| Failure::not_run("only a colleague tells the agent that asked it"))?;
    if message.trim().is_empty() {
        return Err(Failure::not_run("the message is empty"));
    }
    let colleague = Address { machine: node.machine(), resident: id.clone() };
    let told = ColleagueTold { colleague: colleague.clone(), label, attachments: attachments.to_vec() };
    let owing = Owing {
        to: asker.clone(),
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
    Ok(asker)
}
