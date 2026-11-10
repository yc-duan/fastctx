//! What a colleague tells the user (`world-voice.md`, 同事的 message_user).
//! A session working for the user answers in its own last words; a
//! colleague's last words go to the agent that asked it, so the user it
//! answers hears it through `message_user`, and that agent through
//! `message_colleague` (`world-colleagues.md`, 两个工具). The words stay in
//! the conversation's record as the call; the node keeps the latest as the
//! conversation's `told`, which notifications read.

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Delivery, Notice, Origin, Owing, TOLD_CHARS, TellStatus, Told};
use fastctx_types::session::Speaker;
use fastctx_types::voice::{Attachment, ColleagueTold};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// Whether `id` speaks through `message_user`: a colleague the World
/// started, or one it is starting, whose host lists its tools before its
/// record is written.
pub(crate) fn speaks(node: &Node, id: &ResidentId) -> bool {
    node.residents.removed(id).is_none()
        && node.residents.about(id).map_or_else(
            || crate::here::birth_origin(node, id).is_some_and(|origin| matches!(origin, Origin::Colleague { .. })),
            |(about, _)| about.world_started && matches!(about.origin, Origin::Colleague { .. }),
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
        return Err(Failure::not_run(
            "the user reads this conversation itself: write what they should read in your answer, as the last thing you say in your turn",
        ));
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

/// The agent the colleague `id` was asked by, and the task it was asked
/// for; none for a conversation that is no colleague.
pub(crate) fn asker_of(node: &Node, id: &ResidentId) -> Option<(Address, String)> {
    let origin = match node.residents.about(id) {
        Some((about, _)) => about.origin,
        None => crate::here::birth_origin(node, id)?,
    };
    match origin {
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
