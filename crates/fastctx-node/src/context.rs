//! Named context shared by both hosts. Setting a block never starts a turn.

pub(crate) mod input;
pub(crate) mod memory;
pub(crate) mod sources;
mod storage;
pub(crate) mod turn;
mod updates;
pub(crate) mod words;
pub(crate) use updates::{Updates, refresh, request_plan, start};

use crate::Node;
use fastctx_types::{Address, ResidentId};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, path::Path};

/// Origin and delivery policy for a replacement. `by` suppresses an echo to
/// that resident. `full = false` sends only `fact` on an ordinary change;
/// initial instructions and compaction recovery still include the full text.
pub struct Change<'a> {
    pub by: Option<&'a Address>,
    /// A provider revision, also returned with delivery acknowledgements.
    pub version: &'a str,
    pub fact: &'a str,
    pub full: bool,
}

/// Revisions actually included in one tool response or opening instructions.
/// An acknowledgement from before a compaction cannot advance the new epoch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    epoch: u64,
    revisions: BTreeMap<String, u64>,
    versions: BTreeMap<String, String>,
    #[serde(skip)]
    reserved: bool,
    #[serde(skip)]
    shown: std::ops::Range<usize>,
}

impl Receipt {
    pub(crate) fn preceded_by_identity(&mut self) {
        self.shown.start += 1;
        self.shown.end += 1;
    }
}

#[derive(Default)]
pub(crate) struct Deliveries(std::sync::Mutex<std::collections::BTreeSet<(ResidentId, u64)>>);

/// Replaces a named block; empty text removes it. Text is capped at 16 KiB
/// with an explicit truncation notice. Up to eight blocks fit in one reply.
/// Callers must hold the conversation's management gate across source reads
/// and this update, or otherwise serialize their source's revision order.
pub(crate) fn set(node: &Node, id: &ResidentId, name: &str, text: &str, change: Change<'_>) -> io::Result<()> {
    node.residents.active(id).map_err(|error| io::Error::other(error.message))?;
    storage::set(&node.world, id, name, text, change)
}

/// Removes a named block from a conversation that still has its record here,
/// an archived one included. A deleted conversation has nothing left to clear.
/// Sets a block that holds an account's record (the plan, the skills): on
/// this disk for a conversation working for this computer's own accounts,
/// whose disk it is, and only in memory for one assisting another account,
/// whose things never land on the owner's disk (`world-placement.md`, 一条规则).
/// While the account's view has not come it is held in memory.
pub(crate) fn set_account(node: &Node, id: &ResidentId, name: &str, text: &str, change: Change<'_>) -> io::Result<()> {
    let Some((about, _)) = node.residents.about(id) else {
        return storage::set(&node.world, id, name, text, change);
    };
    if crate::assists::assisting_known(node, &about.acting).unwrap_or(true) {
        return storage::set_memory(node, id, name, text, change);
    }
    set(node, id, name, text, change)
}

pub(crate) fn clear(node: &Node, id: &ResidentId, name: &str, change: Change<'_>) -> io::Result<()> {
    if node.residents.about(id).is_none() {
        return Err(io::Error::other(format!("no resident {id} lives here")));
    }
    storage::set(&node.world, id, name, "", change)
}

pub(crate) use sources::{map, plan, plan_unavailable, prepare, refresh_local};

/// Moves the account blocks of conversations known to assist another account
/// off this disk; the owner's own keep theirs here (`set_account`).
pub(crate) fn migrate(node: &Node) {
    for resident in node.residents.everyone() {
        if !crate::assists::assisting_known(node, &resident.about.acting).unwrap_or(false) {
            continue;
        }
        if let Err(error) = storage::migrate(node, &resident.id) {
            node.log.line(format_args!("moving {}'s record projections to memory: {error}", resident.id));
        }
    }
}

pub(crate) fn instructed(node: &Node, id: &ResidentId, heard: &crate::residents::Heard) -> io::Result<()> {
    if let Some(receipt) = &heard.context {
        storage::instructed(&node.world, id, receipt)?;
        sources::observed(node, id, receipt)?;
    }
    Ok(())
}

pub(crate) fn compacted(node: &Node, id: &ResidentId, marker: &str) -> io::Result<()> {
    storage::compacted(&node.world, id, marker)
}

pub(crate) fn forked(node: &Node, id: &ResidentId) -> io::Result<()> {
    storage::forked(&node.world, id)
}

pub(crate) fn collect(node: &Node, id: &ResidentId) -> io::Result<(Vec<String>, Receipt)> {
    storage::collect(&node.world, &Address { machine: node.machine(), resident: id.clone() }, &node.context_bodies)
}

/// Reserve only after every fallible part of reply collection has completed.
/// A concurrent answer omits these blocks and cannot acknowledge them.
pub(crate) fn reserve(
    node: &Node,
    id: &ResidentId,
    heard: &mut crate::residents::Heard,
) -> Option<std::ops::Range<usize>> {
    let receipt = heard.context.as_mut()?;
    if receipt.shown.is_empty() {
        return None;
    }
    if !node.context_deliveries.0.lock().expect("never poisoned").insert((id.clone(), receipt.epoch)) {
        let shown = receipt.shown.clone();
        heard.context = None;
        return Some(shown);
    }
    receipt.reserved = true;
    None
}

/// A failed transport leaves the revisions unheard and releases its reservation.
pub(crate) fn release(node: &Node, id: &ResidentId, heard: &crate::residents::Heard) {
    if let Some(receipt) = &heard.context
        && receipt.reserved
    {
        node.context_deliveries.0.lock().expect("never poisoned").remove(&(id.clone(), receipt.epoch));
    }
}

pub(crate) fn acknowledge(node: &Node, id: &ResidentId, heard: &mut crate::residents::Heard) -> io::Result<()> {
    if let Some(receipt) = &heard.context
        && let Some(accepted) = storage::acknowledge(&node.world, id, receipt)?
    {
        sources::observed(node, id, &accepted)?;
    }
    release(node, id, heard);
    if let Some(receipt) = &mut heard.context {
        receipt.reserved = false;
    }
    Ok(())
}

/// The plugin reads the canonical per-block files through this private pointer.
/// No body is copied to a second place that could lag after a failed refresh.
pub(crate) fn hand_to_session(node: &Node, id: &ResidentId, session: &Path) -> io::Result<()> {
    storage::initialize(&node.world, id)?;
    let path = session.join("context.json");
    let bytes = serde_json::to_vec(&serde_json::json!({
        "directory": storage::directory(&node.world, id),
        "state": storage::state_path(&node.world, id),
        "memory": true,
    }))
    .map_err(io::Error::other)?;
    match std::fs::read(&path) {
        Ok(kept) if kept == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
}

/// What the plugin reports a session's model holds: the block versions a
/// fresh first message read, or, after Claude Code compacted the
/// conversation, those of the read its new first message was built from,
/// with `compacted` naming that compaction.
#[derive(Deserialize)]
struct PluginHeard {
    #[serde(flatten)]
    receipt: Receipt,
    #[serde(default)]
    compacted: Option<String>,
}

/// A prompt.context rebuild actually read these block versions. This is also
/// consumed before a plan command so its compare-and-swap uses what was shown.
pub(crate) fn plugin_heard(node: &Node, id: &ResidentId) -> io::Result<()> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    if about.host.agent.as_str() != "claude" {
        return Ok(());
    }
    let Some(session) = about.session else { return Ok(()) };
    let path = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session).join("context-heard.json");
    let heard: PluginHeard = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let accepted = match &heard.compacted {
        Some(compaction) => storage::reopened(&node.world, id, compaction, &heard.receipt)?,
        None => storage::acknowledge(&node.world, id, &heard.receipt)?,
    };
    if let Some(accepted) = accepted {
        sources::observed(node, id, &accepted)?;
    }
    Ok(())
}
