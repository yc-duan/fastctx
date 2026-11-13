//! When this machine was shared whole, and as which body (design §6.2,
//! 7965 N02): kept as each network's directory tells it, every change with
//! when it was seen, so a conversation born here belongs to the body this
//! machine was as it was born, however late its first scope is made.

use crate::Node;
use fastctx_types::access::HubId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The most changes kept: the oldest go first.
const KEPT: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Change {
    /// When the change was seen, in milliseconds since 1970.
    at: i64,
    hub: String,
    /// The body this machine is from then on, none while it is not shared
    /// whole.
    body: Option<String>,
}

fn path(node: &Node) -> PathBuf {
    node.world.path().join("shared-history.json")
}

fn read(node: &Node) -> Vec<Change> {
    fastctx_platform::disk::read(path(node)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Notes what the directory of `hub` says of this machine now: shared whole
/// as a body, or not. Call on a thread that may block.
pub(crate) fn noted(node: &Node, hub: &HubId, directory: &fastctx_net::directory::Directory) {
    let me = node.machine();
    let machine = directory.canonical(&me);
    let shared = directory.shares.iter().any(|share| &share.machine == machine && share.active());
    let body = if shared { directory.body_of(machine) } else { None };
    let mut changes = read(node);
    let last = changes.iter().rev().find(|change| change.hub == hub.as_str()).map(|change| change.body.clone());
    if last.as_ref() == Some(&body) {
        return;
    }
    // The first seen of this network stands for all before it, as nothing
    // earlier is known; each later change from when it was seen.
    let at = if last.is_some() { jiff::Timestamp::now().as_millisecond() } else { 0 };
    changes.push(Change { at, hub: hub.to_string(), body });
    if changes.len() > KEPT {
        changes.drain(..changes.len() - KEPT);
    }
    if let Ok(bytes) = serde_json::to_vec(&changes) {
        let _ = fastctx_platform::publish(&path(node), &bytes, fastctx_platform::Permissions::Private);
    }
}

/// What this machine was on `hub` at `at`: shared whole as a body, or not;
/// none when nothing is noted of that network.
pub(crate) fn at(node: &Node, hub: &HubId, at: jiff::Timestamp) -> Option<Option<String>> {
    let at = at.as_millisecond();
    read(node).into_iter().rev().find(|change| change.hub == hub.as_str() && change.at <= at).map(|change| change.body)
}
