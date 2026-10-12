//! Each block of thinking a live session shows, finished again as Claude
//! Code recorded it: with its own measure of how long the model thought and
//! the request's usage (`fastctx_hosts::claude::thoughts`). The mod says which
//! live item a stored row ended; the follower of the session's file reads the
//! row once Claude Code has written it, which is when its step is recorded.
//!
//! A claim usually comes before its row: the mod sends it as the thinking
//! ends, and Claude Code writes the step's rows only after the step. A row
//! read first waits for its claim. The mod claims rows in the order Claude
//! Code stores them and the node applies its reports in that order, so once
//! a later row is claimed, an unclaimed earlier one never will be.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_hosts::claude::thoughts::Thought;
use fastctx_types::ResidentId;

use crate::Node;

#[derive(Default)]
pub(crate) struct Thoughts(Mutex<HashMap<PathBuf, File>>);

/// What waits on one session file.
#[derive(Default)]
struct File {
    /// Rows the mod says ended a live item, not read yet.
    claimed: HashMap<String, Claim>,
    /// Thinking rows written while the file was followed, oldest first,
    /// whose claim has not come yet.
    unclaimed: Vec<Thought>,
}

pub(super) struct Claim {
    pub resident: ResidentId,
    pub turn: String,
    pub item: String,
}

/// A claimed row that has been read: what to show.
pub(super) struct Ready {
    claim: Claim,
    thought: Thought,
}

impl Thoughts {
    /// The mod says the row `uuid` of the session file at `path` ended the
    /// live item the claim names.
    pub(super) fn claim(&self, path: &Path, uuid: String, claim: Claim) -> Option<Ready> {
        let mut files = self.0.lock().expect("never poisoned");
        let file = files.entry(path.to_path_buf()).or_default();
        match file.unclaimed.iter().position(|thought| thought.uuid == uuid) {
            Some(at) => {
                Some(Ready { claim, thought: file.unclaimed.drain(..=at).next_back().expect("the claimed row") })
            }
            None => {
                file.claimed.insert(uuid, claim);
                None
            }
        }
    }

    /// The follower read a row of thinking from the file at `path`; `new`
    /// says it was written while the file was followed, not before.
    pub(super) fn read(&self, path: &Path, thought: Thought, new: bool) -> Option<Ready> {
        let mut files = self.0.lock().expect("never poisoned");
        let file = files.entry(path.to_path_buf()).or_default();
        match file.claimed.remove(&thought.uuid) {
            Some(claim) => {
                file.unclaimed.clear();
                Some(Ready { claim, thought })
            }
            None => {
                if new {
                    file.unclaimed.push(thought);
                }
                None
            }
        }
    }

    /// The file was rewritten: what it held is read again from its start.
    pub(super) fn restarted(&self, path: &Path) {
        if let Some(file) = self.0.lock().expect("never poisoned").get_mut(path) {
            file.unclaimed.clear();
        }
    }
}

/// The thinking item again, finished as Claude Code recorded it, and the
/// request's usage for those watching. The usage is not kept: Claude Code's
/// file is the record, and history reads it from there.
pub(super) async fn show(node: &Arc<Node>, ready: Ready) -> std::io::Result<()> {
    let Ready { claim, thought } = ready;
    let mut events = thought.events(&claim.turn, &claim.item).into_iter();
    if let Some(finished) = events.next() {
        crate::turns::apply(node, &claim.resident, finished).await?;
    }
    for usage in events {
        node.watchers.publish(&claim.resident, &usage);
    }
    Ok(())
}
