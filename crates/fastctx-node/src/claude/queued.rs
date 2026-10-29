//! A new Claude conversation's first turn takes a turn of its login, as
//! every turn does (`turn_slots`, `world-usage.md`): with none free, the
//! conversation is there with its first words, and its session starts once
//! one is. A mark in its directory says it waits, so a node that restarts
//! meanwhile goes on waiting rather than calling the fate of its first
//! words unknown: no session was asked for yet.

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::ResidentId;
use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::Activity;

use crate::Node;

/// The mark of a conversation whose session waits for its login's turn.
const MARK: &str = "waiting-for-its-login";

/// How often a waiting first turn asks for its login's turn again: well
/// within the time its place in line is kept unasked.
const ASK_AGAIN: Duration = Duration::from_secs(2);

pub(super) fn mark(directory: &ConversationDir) -> std::io::Result<()> {
    fastctx_platform::publish(&directory.path().join(MARK), b"", fastctx_platform::Permissions::Private)
}

pub(super) fn marked(directory: &ConversationDir) -> bool {
    directory.path().join(MARK).is_file()
}

fn unmark(directory: &ConversationDir) -> std::io::Result<()> {
    match std::fs::remove_file(directory.path().join(MARK)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Waits for a turn of `id`'s login, then starts its session with its first
/// words. A conversation deleted meanwhile is let go.
pub(super) fn wait(node: &Arc<Node>, id: ResidentId) {
    let node = node.clone();
    tokio::spawn(async move {
        node.ready().await;
        loop {
            if node.residents.active(&id).is_err() {
                return;
            }
            let Some((about, _)) = node.residents.about(&id) else { return };
            if crate::turn_slots::admit(&node, &id, &about).is_ok() {
                break;
            }
            tokio::time::sleep(ASK_AGAIN).await;
        }
        let directory = node.world.conversation(&id);
        let started = node
            .residents
            .change_now(&node.world, &id, |now| {
                now.activity = Activity::Busy;
                now.since = jiff::Timestamp::now();
            })
            .and_then(|()| unmark(&directory));
        if let Err(error) = started {
            node.log.line(format_args!("starting {id}'s first turn, which waited for its login: {error}"));
            return;
        }
        if let Err(failure) = super::launch(&node, &id).await {
            node.log.line(format_args!("starting {id}'s session, which waited for its login: {}", failure.message));
        }
    });
}
