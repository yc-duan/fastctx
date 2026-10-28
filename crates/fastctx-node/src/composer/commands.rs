//! The places of the commands the node took to do once a turn ends, among
//! the words that wait: one order for both, kept with the words and moved
//! like them. The commands themselves are the control queue's.

use super::*;
use fastctx_types::NoticeId;

/// Takes a place after the words waiting now for a command just taken.
/// A conversation with no shared input has no words waiting to go after.
pub async fn place_command(node: &Arc<Node>, id: &ResidentId, command: &NoticeId) -> Result<(), Failure> {
    let Some(cell) = storage::existing(node, id).await? else {
        return Ok(());
    };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    if next.queue.iter().any(|entry| &entry.id == command) {
        return Ok(());
    }
    next.queue.push(Entry::command(command.clone(), node.machine()));
    publish(node, id, &mut cell, next).await
}

/// Gives up the place of a command done or withdrawn.
pub async fn forget_command(node: &Arc<Node>, id: &ResidentId, command: &str) -> Result<(), Failure> {
    let Some(cell) = storage::existing(node, id).await? else {
        return Ok(());
    };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    if !next.queue.iter().any(|entry| entry.command && entry.id.as_str() == command) {
        return Ok(());
    }
    next.queue.retain(|entry| !(entry.command && entry.id.as_str() == command));
    publish(node, id, &mut cell, next).await?;
    drop(cell);
    schedule(node, id);
    Ok(())
}

/// The waiting command to do now, of `waiting` in the control queue's
/// order: one with no place first (taken when no words waited), then the
/// first by place, unless words before it are still to go on their own,
/// on their way or waiting with nothing holding the queue: then none.
pub async fn next_command(node: &Node, id: &ResidentId, waiting: &[String]) -> Result<Option<String>, Failure> {
    let Some(cell) = storage::existing(node, id).await? else {
        return Ok(waiting.first().cloned());
    };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let state = cell.as_ref().expect("loaded");
    let placed = |command: &str| state.queue.iter().any(|entry| entry.command && entry.id.as_str() == command);
    if let Some(unplaced) = waiting.iter().find(|command| !placed(command)) {
        return Ok(Some(unplaced.clone()));
    }
    let going =
        |entry: &NoticeId| state.sending.iter().chain(state.pending.iter()).any(|flight| &flight.entry == entry);
    for entry in &state.queue {
        if entry.command {
            if waiting.iter().any(|command| command == entry.id.as_str()) {
                return Ok(Some(entry.id.to_string()));
            }
        } else if going(&entry.id) || (state.held.is_none() && entry.error.is_none() && entry.submitted.is_none()) {
            return Ok(None);
        }
    }
    Ok(None)
}

/// Drops the places of commands no longer waiting; says whether a waiting
/// one stands at the head, before every word.
pub(super) async fn settle(node: &Arc<Node>, id: &ResidentId, cell: &mut Option<Stored>) -> Result<bool, Failure> {
    let state = cell.as_ref().expect("loaded");
    if !state.queue.iter().any(|entry| entry.command) {
        return Ok(false);
    }
    let waiting: Vec<String> = crate::control_queue::pending(node, id)?.into_iter().map(|row| row.id).collect();
    let kept = |entry: &Entry| !entry.command || waiting.iter().any(|command| command == entry.id.as_str());
    if !state.queue.iter().all(kept) {
        let mut next = cell.take().expect("loaded");
        next.queue.retain(kept);
        publish(node, id, cell, next).await?;
    }
    Ok(cell.as_ref().expect("loaded").queue.first().is_some_and(|entry| entry.command))
}
