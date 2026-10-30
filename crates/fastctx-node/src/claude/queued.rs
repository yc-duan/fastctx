//! A new Claude conversation's first turn takes a turn of its login, as
//! every turn does (`turn_slots`, `world-usage.md`): with none free, the
//! conversation is there with its first words, and its session starts once
//! one is. A mark in its directory says it waits, so a node that restarts
//! meanwhile goes on waiting rather than calling the fate of its first
//! words unknown: no session was asked for yet. The mark goes only once
//! Claude Code is asked: until then, what any wake is checked against
//! (`lifecycle::wake::check`, the account's layer among it) holds the first
//! turn back, and a start that fails before reaching Claude Code is tried
//! again later as it stands.

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

/// The longest a first turn whose start failed before reaching Claude Code
/// waits before it is tried again.
const RETRY_AT_MOST: Duration = Duration::from_secs(60);

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
        node.ready_for(&id).await;
        let directory = node.world.conversation(&id);
        let mut again = ASK_AGAIN;
        loop {
            if node.residents.active(&id).is_err() {
                return;
            }
            let Some((about, _)) = node.residents.about(&id) else { return };
            // Held back as any wake is: while its account is in Connect, its
            // home is away, its assistance or its project's access does not
            // hold.
            if crate::lifecycle::wake::check(&node, &id, &about).is_err()
                || crate::turn_slots::admit(&node, &id, &about).is_err()
            {
                tokio::time::sleep(ASK_AGAIN).await;
                continue;
            }
            let prepared = match super::prepare_launch(&node, &id) {
                Ok(prepared) => prepared,
                Err(failure) => {
                    node.log.line(format_args!(
                        "preparing {id}'s first turn, which waited for its login; tried again in {} s: {}",
                        again.as_secs(),
                        failure.message
                    ));
                    tokio::time::sleep(again).await;
                    again = (again * 2).min(RETRY_AT_MOST);
                    continue;
                }
            };
            let started = node
                .residents
                .change_now(&node.world, &id, |now| {
                    now.activity = Activity::Busy;
                    now.since = jiff::Timestamp::now();
                })
                .and_then(|()| unmark(&directory));
            if let Err(error) = started {
                node.log
                    .line(format_args!("starting {id}'s first turn, which waited for its login; tried again: {error}"));
                tokio::time::sleep(again).await;
                again = (again * 2).min(RETRY_AT_MOST);
                continue;
            }
            // Claude Code is asked from here: what comes of it is settled
            // with the first words, as for any new conversation.
            if let Err(failure) = super::begin_launch(&node, &id, prepared).await {
                node.log.line(format_args!("starting {id}'s session, which waited for its login: {}", failure.message));
            }
            return;
        }
    });
}
