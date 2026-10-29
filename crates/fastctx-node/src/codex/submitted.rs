//! Words the node before this one handed to Codex without learning which
//! turn took them (`world-concurrency.md`, 交接与回执). The thread's record
//! keeps them with the id they were sent under: found there, they are bound
//! to their turn, which is ended as the record says, or as interrupted by
//! the restart where the record says it ran on, rather than left unknown.

use std::sync::Arc;

use fastctx_types::layout::ConversationDir;
use fastctx_types::session::{TurnEnd, TurnHow};

use crate::Node;
use crate::delivery::Handover;
use crate::residents::OnRecord;

/// How long one thread's record is read for: beyond it, what the restart
/// left unknown stays unknown, and the node's start goes on.
const READING: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether the words of `handover` were found in the Codex thread of
/// `resident`, and so bound and settled. A record that cannot be read finds
/// nothing. The caller holds the speech lock.
pub(crate) async fn found(
    node: &Arc<Node>,
    resident: &OnRecord,
    directory: &ConversationDir,
    handover: &mut Handover,
) -> std::io::Result<bool> {
    let about = &resident.about;
    let Some(thread) = about.session.as_deref().filter(|_| about.host.agent.as_str() == "codex") else {
        return Ok(false);
    };
    let client = format!("{}{}", fastctx_hosts::codex::WORLD_CLIENT, handover.id);
    let read = async {
        crate::homes::available(node, about).map_err(|failure| failure.message)?;
        node.codex.of(node, about).map_err(|failure| failure.message)?.submitted(thread, &client).await
    };
    let submitted = match tokio::time::timeout(READING, read).await {
        Ok(Ok(Some(submitted))) => submitted,
        Ok(Ok(None)) => return Ok(false),
        Ok(Err(error)) => {
            node.log.line(format_args!(
                "reading the Codex thread of {} for words its restart left unknown: {error}",
                resident.id
            ));
            return Ok(false);
        }
        Err(_) => {
            node.log.line(format_args!(
                "reading the Codex thread of {} for words its restart left unknown took over {} s",
                resident.id,
                READING.as_secs()
            ));
            return Ok(false);
        }
    };
    let turn = submitted.turn;
    crate::speech::bind(node, &resident.id, directory, handover, &turn)
        .await
        .map_err(|failure| std::io::Error::other(failure.message))?;
    if crate::delivery::terminal(directory, &turn)?.is_none() {
        // Nothing runs it now: its app-server went with the node before.
        let restarted = format!("the node on {} restarted while it ran", node.machine());
        let end = match submitted.end {
            Some(mut end) => {
                if end.how == TurnHow::Interrupted && end.reason.is_none() {
                    end.reason = Some(restarted);
                }
                end
            }
            None => TurnEnd {
                facts: None,
                error: None,
                how: TurnHow::Interrupted,
                reason: Some(restarted),
                duration_ms: None,
                last_message: None,
            },
        };
        crate::turns::finish_locked(node, &resident.id, &turn, end, &[]).await?;
    }
    node.log.line(format_args!("words {} was given before the restart went into its turn {turn}", resident.id));
    Ok(true)
}
