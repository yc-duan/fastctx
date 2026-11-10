//! What a native fork carried of the context it was forked from.
//!
//! Claude Code loads the source's transcript before it writes the fork's, and
//! loading leaves out assistant messages that hold nothing to send again: one
//! a stop cut off while the model was still thinking, or one of whitespace
//! alone (2.1.303). Everything else the fork carries is the source's own row,
//! under the same UUID and in the same order, closed by a synthetic reply
//! when the context ends on the user's side.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fastctx_hosts::claude::{exchange, transcript};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use serde_json::Value;

use crate::Node;

/// The context a fork is to carry: the source's, as its transcript holds it.
pub(super) struct Expected {
    uuids: Vec<String>,
    messages: Vec<Value>,
}

/// The fork's own transcript, once it holds the expected context.
pub(super) struct Carried {
    pub copied: transcript::Transcript,
    /// The expected messages Claude Code left out as it loaded them, by UUID.
    pub left_out: HashSet<String>,
}

pub(super) async fn expected(node: &Node, session: &str, prefix: Option<&Path>) -> Result<Expected, Failure> {
    let path = match prefix {
        Some(path) => path.to_path_buf(),
        None => exchange::transcript(&exchange::sessions(node.world.path()).join(session))
            .map_err(failure)?
            .ok_or_else(|| failure("the source conversation has no saved history"))?,
    };
    let read = read(path).await?;
    if !read.problems.is_empty() {
        return Err(failure(read.problems.join("; ")));
    }
    Ok(Expected { uuids: read.context_uuids, messages: read.context_messages })
}

/// Waits for the fork's transcript to hold the expected context. Creating the
/// transcript precedes asynchronously saving the copied rows.
pub(super) async fn confirm(node: &Node, child: &ResidentId, expected: &Expected) -> Result<Carried, Failure> {
    let session = node
        .residents
        .about(child)
        .and_then(|(about, _)| about.session)
        .ok_or_else(|| failure("the fork has no saved session"))?;
    let directory = exchange::sessions(node.world.path()).join(session);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let finding = directory.clone();
        let path = fastctx_platform::task::spawn_blocking(move || exchange::transcript(&finding))
            .await
            .map_err(failure)?
            .map_err(failure)?;
        if let Some(path) = path {
            let copied = read(path).await?;
            if copied.problems.is_empty()
                && let Some(left_out) = left_out(expected, &copied)
            {
                return Ok(Carried { copied, left_out });
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(failure("Claude's fork did not persist the expected context within 10 s"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The expected messages the fork left out, when all it carries is the
/// expected context in order and only messages that say nothing are missing.
fn left_out(expected: &Expected, copied: &transcript::Transcript) -> Option<HashSet<String>> {
    let mut carried = copied.context_uuids.iter().zip(&copied.context_messages).peekable();
    let mut left_out = HashSet::new();
    for (uuid, message) in expected.uuids.iter().zip(&expected.messages) {
        match carried.peek() {
            Some((kept, copy)) if *kept == uuid => {
                if !transcript::same_projection(copy, message) {
                    return None;
                }
                carried.next();
            }
            _ if transcript::says_nothing(message) => {
                left_out.insert(uuid.clone());
            }
            _ => return None,
        }
    }
    match carried.count() {
        0 => Some(left_out),
        1 if copied.synthetic_tail => Some(left_out),
        _ => None,
    }
}

/// The source's checkpoints as the fork holds them: without the messages it
/// left out, each turn's boundaries counting only what it kept. A turn that
/// kept nothing goes. A message the checkpoints do not name was never counted
/// in them.
pub(super) fn without(checkpoints: &mut Value, left_out: &HashSet<String>) {
    let (Some(uuids), Some(messages)) = (checkpoints["uuids"].as_array(), checkpoints["messages"].as_array()) else {
        return;
    };
    let gone: Vec<bool> = uuids.iter().map(|uuid| uuid.as_str().is_some_and(|uuid| left_out.contains(uuid))).collect();
    if uuids.len() != messages.len() || !gone.contains(&true) {
        return;
    }
    let kept_before = |count: u64| {
        let count = usize::try_from(count).unwrap_or(usize::MAX).min(gone.len());
        (count - gone[..count].iter().filter(|&&gone| gone).count()) as u64
    };
    let keep = |values: &Vec<Value>| -> Vec<Value> {
        values.iter().zip(&gone).filter(|(_, gone)| !**gone).map(|(value, _)| value.clone()).collect()
    };
    let (uuids, messages) = (keep(uuids), keep(messages));
    if let Some(turns) = checkpoints["turns"].as_object_mut() {
        for point in turns.values_mut() {
            for side in ["before", "after"] {
                if let Some(count) = point[side].as_u64() {
                    point[side] = kept_before(count).into();
                }
            }
        }
        turns.retain(|_, point| point["after"].as_u64().is_none_or(|after| after > 0));
    }
    checkpoints["uuids"] = uuids.into();
    checkpoints["messages"] = messages.into();
}

async fn read(path: PathBuf) -> Result<transcript::Transcript, Failure> {
    fastctx_platform::task::spawn_blocking(move || transcript::read_with_cuts(&path, &[]))
        .await
        .map_err(failure)?
        .map_err(failure)
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
