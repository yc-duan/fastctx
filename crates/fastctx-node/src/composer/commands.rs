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

/// Gives up the place of a command done or withdrawn. One that ran and did
/// not get done (`failed`, with why) ends as a turn that failed does: the
/// words still waiting wait for the user.
pub async fn forget_command(
    node: &Arc<Node>,
    id: &ResidentId,
    command: &str,
    failed: Option<&str>,
) -> Result<(), Failure> {
    let Some(cell) = storage::existing(node, id).await? else {
        return Ok(());
    };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    let Some(place) = next.queue.iter().position(|entry| entry.command && entry.id.as_str() == command) else {
        *cell = Some(next);
        return Ok(());
    };
    let after: Vec<NoticeId> = next.queue[place + 1..].iter().map(|entry| entry.id.clone()).collect();
    next.queue.remove(place);
    if let Some(why) = failed {
        unline_straight(&mut next, &after);
        hold_words(&mut next, why);
    }
    publish(node, id, &mut cell, next).await?;
    drop(cell);
    schedule(node, id);
    Ok(())
}

/// A command among the words that ended only after its place was given up
/// (a Codex compaction, which runs as a turn of its own) did not get done:
/// the words still waiting wait for the user.
pub async fn command_failed(node: &Arc<Node>, id: &ResidentId, why: &str) -> Result<(), Failure> {
    let Some(cell) = storage::existing(node, id).await? else {
        return Ok(());
    };
    let mut cell = cell.lock().await;
    load(node, id, &mut cell).await?;
    let mut next = cell.take().expect("loaded");
    // Its place is gone, and every word that went after it with it: words
    // sent from the input after a stop that still wait in line wait for the
    // user too, as behind a command that failed in its place.
    let all: Vec<NoticeId> = next.queue.iter().map(|entry| entry.id.clone()).collect();
    unline_straight(&mut next, &all);
    hold_words(&mut next, why);
    publish(node, id, &mut cell, next).await?;
    drop(cell);
    schedule(node, id);
    Ok(())
}

/// Words sent from the input after a stop that waited behind a command wait
/// for the user once it failed, as the words queued behind it do; words the
/// user asked to go at once from the queue still go.
fn unline_straight(state: &mut Stored, after: &[NoticeId]) {
    let waiting: Vec<NoticeId> = state
        .lined
        .iter()
        .filter(|lined| lined.straight && after.contains(&lined.entry))
        .map(|lined| lined.entry.clone())
        .collect();
    for entry in &waiting {
        state.unline(entry);
    }
}

fn hold_words(state: &mut Stored, why: &str) {
    if state.queue.iter().any(|entry| !entry.command && !state.on_their_way(&entry.id)) {
        state.opportunity = None;
        state.held = Some(format!("a command did not finish ({why}); queued messages wait for the user"));
    }
}

/// The waiting command to do now, of `waiting` in the control queue's
/// order: one with no place first (taken when no words waited), then the
/// first by place, unless words before it are still to go on their own,
/// on their way or waiting with nothing holding the queue: then none.
pub async fn next_command(node: &Arc<Node>, id: &ResidentId, waiting: &[String]) -> Result<Option<String>, Failure> {
    // Empty control queues must not materialize every idle draft during startup.
    // The composer recovery worker restores its own reservations independently.
    if waiting.is_empty() {
        return Ok(None);
    }
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
    for entry in &state.queue {
        if entry.command {
            if waiting.iter().any(|command| command == entry.id.as_str()) {
                return Ok(Some(entry.id.to_string()));
            }
        } else if state.on_their_way(&entry.id)
            || (state.held.is_none() && entry.error.is_none() && entry.submitted.is_none())
        {
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
    let waiting: Vec<String> = crate::control_queue::pending(node, id).await?.into_iter().map(|row| row.id).collect();
    let kept = |entry: &Entry| !entry.command || waiting.iter().any(|command| command == entry.id.as_str());
    if !state.queue.iter().all(kept) {
        let mut next = cell.take().expect("loaded");
        next.queue.retain(kept);
        publish(node, id, cell, next).await?;
    }
    Ok(cell.as_ref().expect("loaded").queue.first().is_some_and(|entry| entry.command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::composer::Draft;

    fn entry(words: Option<&str>) -> Entry {
        let id = crate::speech::new_id(if words.is_some() { "queued" } else { "control" });
        let machine: MachineName = "desktop".parse().expect("a machine name");
        match words {
            Some(words) => Entry {
                id,
                from: machine,
                document: document::Document::from_view(Draft { text: words.into(), ..Draft::default() }).unwrap(),
                editors: HashSet::new(),
                error: None,
                submitted: None,
                command: false,
            },
            None => Entry::command(id, machine),
        }
    }

    #[test]
    fn a_command_that_failed_holds_the_words_after_it() {
        let mut state = Stored::default();
        state.queue.push(entry(Some("after the compaction")));
        state.opportunity = Some("turn-1".into());
        hold_words(&mut state, "the compaction failed");
        assert!(state.held.as_deref().is_some_and(|held| held.contains("the compaction failed")));
        assert!(state.opportunity.is_none());
    }

    #[test]
    fn a_command_that_failed_holds_nothing_when_no_words_wait() {
        let mut state = Stored::default();
        state.queue.push(entry(None));
        hold_words(&mut state, "the compaction failed");
        assert!(state.held.is_none());
        // Words on their way do not wait for the user either.
        let mut state = Stored::default();
        state.queue.push(entry(Some("going")));
        let id = state.queue[0].id.clone();
        state.reindex();
        state.line_up(&id, None, false).unwrap();
        hold_words(&mut state, "the compaction failed");
        assert!(state.held.is_none());
    }
}
