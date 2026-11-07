//! Atomic block replacements and delivery cursors. No host or network I/O is
//! performed while the short conversation-record transaction is held.

use std::{collections::BTreeMap, io, path::PathBuf};

use fastctx_types::{Address, ResidentId, layout::WorldDir};
use serde::{Deserialize, Serialize};

use super::{Change, Receipt, words};

/// The limit for file-backed context blocks, including JSON escaping.
/// History is bounded at its source and always delivered whole.
const BLOCK_KEPT_BYTES: usize = 4 * 1024 * 1024 - 64 * 1024;
const BLOCK_COUNT: usize = 12;
const FACT_BYTES: usize = 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Block {
    pub revision: u64,
    pub written_epoch: u64,
    pub source: String,
    pub version: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transient: bool,
    pub by: Option<Address>,
    pub fact: String,
    pub full: bool,
}

impl Block {
    /// Whether `old` already says what this one would.
    fn same_as(&self, old: &Block) -> bool {
        old.source == self.source && old.version == self.version && old.transient == self.transient
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct State {
    epoch: u64,
    opening: BTreeMap<String, u64>,
    seen: BTreeMap<String, u64>,
    restored: BTreeMap<String, u64>,
    settled: bool,
    compacted: Option<String>,
}

pub(super) fn directory(world: &WorldDir, id: &ResidentId) -> PathBuf {
    world.conversation(id).path().join("context")
}

pub(super) fn state_path(world: &WorldDir, id: &ResidentId) -> PathBuf {
    world.conversation(id).path().join("context-state.json")
}

fn exists(world: &WorldDir, id: &ResidentId) -> io::Result<()> {
    let home = world.conversation(id);
    if fastctx_platform::disk::exists(world.deleted().join(format!("{id}.json")))?
        || fastctx_platform::disk::exists(home.path().join("deleting"))?
        || fastctx_platform::disk::exists(home.path().join("removed"))?
        || !fastctx_platform::disk::exists(home.path())?
    {
        return Err(io::Error::new(io::ErrorKind::NotFound, words::UNAVAILABLE));
    }
    Ok(())
}

pub(super) fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn with_bodies(
    id: &ResidentId,
    blocks: BTreeMap<String, Block>,
    bodies: &super::memory::Bodies,
) -> BTreeMap<String, Block> {
    blocks
        .into_iter()
        .filter_map(|(name, mut block)| {
            if block.transient {
                block.text = bodies.get(id, &name, &block.source)?;
                block.fact = bodies.get(id, &format!("{name}-fact"), &block.source).unwrap_or_default();
            }
            Some((name, block))
        })
        .collect()
}

fn kept(text: &str) -> usize {
    serde_json::to_string(text).map_or(usize::MAX, |kept| kept.len())
}

/// `text` whole where it fits within `limit` as kept; else as much of its
/// start as does, and a line that says how much that is.
fn bounded(text: &str, limit: usize) -> String {
    if kept(text) <= limit {
        return text.into();
    }
    let mut end = text.len().min(limit);
    loop {
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        // A byte takes at least one as kept, so cutting the excess away
        // brings the start within the limit in a few rounds.
        let excess = kept(&text[..end]).saturating_sub(limit.saturating_sub(128));
        if excess == 0 {
            break;
        }
        end -= excess.min(end);
    }
    format!("{}{}", &text[..end], words::truncated(end, text.len()))
}

pub(super) fn set(world: &WorldDir, id: &ResidentId, name: &str, text: &str, change: Change<'_>) -> io::Result<()> {
    set_mode(world, id, name, text, change, None)
}
pub(super) fn set_memory(
    node: &crate::Node,
    id: &ResidentId,
    name: &str,
    text: &str,
    change: Change<'_>,
) -> io::Result<()> {
    set_mode(&node.world, id, name, text, change, Some(&node.context_bodies))
}
fn set_mode(
    world: &WorldDir,
    id: &ResidentId,
    name: &str,
    text: &str,
    change: Change<'_>,
    bodies: Option<&super::memory::Bodies>,
) -> io::Result<()> {
    if !valid(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, words::INVALID_NAME));
    }
    exists(world, id)?;
    let source = crate::profile::session::hash(text);
    let (text, fact) = if name == "history" {
        (text.to_string(), change.fact.to_string())
    } else {
        (bounded(text, BLOCK_KEPT_BYTES), bounded(change.fact, FACT_BYTES))
    };
    let incoming = Block {
        revision: 0,
        written_epoch: 0,
        source: source.clone(),
        version: change.version.into(),
        text: if bodies.is_some() { String::new() } else { text.clone() },
        transient: bodies.is_some(),
        by: change.by.cloned(),
        fact: if bodies.is_some() { String::new() } else { fact.clone() },
        full: change.full,
    };
    let key = name.to_string();
    let withdraw = bodies.is_some() || (text.is_empty() && matches!(name, "project" | "map" | "history"));
    let empty = text.is_empty() && change.version.is_empty();
    // What is written already is not written again: a refresh that finds
    // nothing changed opens no transaction and projects nothing, once the
    // block's migration originals, if it withdraws them, are withdrawn. A body
    // kept only in memory is put back, as after the node started again.
    let withdrawal = (directory(world, id), key.clone());
    let withdrawn = !withdraw || WITHDRAWN.lock().expect("never poisoned").contains(&withdrawal);
    let (unchanged, looking) = (incoming.clone(), key.clone());
    let records = super::records::prepare(world, id)?;
    let written = crate::records::read(&records, move |rows| {
        let old = rows.get::<Block>(super::records::BLOCKS, &looking)?;
        Ok(old.as_ref().is_some_and(|old| unchanged.same_as(old)) || (old.is_none() && empty))
    })?;
    if written && withdrawn {
        if let Some(bodies) = bodies {
            bodies.put(id, name, &source, &text);
            bodies.put(id, &format!("{name}-fact"), &source, &fact);
        }
        return Ok(());
    }
    let committed = super::records::commit(
        world,
        id,
        text.len() + fact.len() + 1024,
        super::projection::Selection::Block(key.clone()),
        |_| {
            if let Some(bodies) = bodies {
                bodies.put(id, name, &source, &text);
                bodies.put(id, &format!("{name}-fact"), &source, &fact);
            }
        },
        move |rows| {
            if withdraw {
                super::archived::mark(rows, &key)?;
            }
            set_block(rows, &key, incoming, empty)
        },
    );
    if committed.is_ok() && withdraw {
        let mut withdrawn = WITHDRAWN.lock().expect("never poisoned");
        if withdrawn.len() >= WITHDRAWN_KEPT {
            withdrawn.clear();
        }
        withdrawn.insert(withdrawal);
    }
    committed
}

/// Blocks whose migration originals this process has withdrawn already. The
/// originals are made only as a conversation's records move in, before any of
/// its blocks is written, so a block withdraws them once and needs it no more.
static WITHDRAWN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<(std::path::PathBuf, String)>>> =
    std::sync::LazyLock::new(Default::default);

/// Blocks remembered as withdrawn; past it, each withdraws once more.
const WITHDRAWN_KEPT: usize = 1 << 16;

fn set_block(
    rows: &crate::records::Rows<'_>,
    key: &str,
    mut incoming: Block,
    empty: bool,
) -> fastctx_storage::Result<()> {
    let state: State = rows.get(super::records::STATE, "current")?.unwrap_or_default();
    let old = rows.get::<Block>(super::records::BLOCKS, key)?;
    if old.as_ref().is_some_and(|old| incoming.same_as(old)) || (old.is_none() && empty) {
        return Ok(());
    }
    if old.is_none() && super::records::blocks(rows)?.len() >= BLOCK_COUNT {
        return Err(fastctx_storage::Error::new(words::TOO_MANY));
    }
    incoming.revision = old.map_or(Ok(1), |old| {
        old.revision.checked_add(1).ok_or_else(|| fastctx_storage::Error::new(words::REVISION_EXHAUSTED))
    })?;
    incoming.written_epoch = state.epoch;
    rows.put(super::records::BLOCKS, key, &incoming)
}

pub(super) fn opening(
    world: &WorldDir,
    id: &ResidentId,
    bodies: &super::memory::Bodies,
) -> io::Result<(String, Receipt)> {
    exists(world, id)?;
    super::records::read(world, id, |state, blocks| {
        let mut text = Vec::new();
        let mut revisions = BTreeMap::new();
        let mut versions = BTreeMap::new();
        for (name, block) in with_bodies(id, blocks, bodies) {
            revisions.insert(name.clone(), block.revision);
            versions.insert(name.clone(), block.version);
            if !block.text.is_empty() {
                text.push(words::heading(&name, &block.text));
            }
        }
        Ok((text.join("\n\n"), Receipt { epoch: state.epoch, revisions, versions, reserved: false, shown: 0..0 }))
    })
}
pub(super) fn instructed(world: &WorldDir, id: &ResidentId, receipt: &Receipt) -> io::Result<()> {
    exists(world, id)?;
    let receipt = receipt.clone();
    super::records::state(world, id, move |_, state| {
        state.epoch = next_epoch(state.epoch)?;
        state.opening = receipt.revisions.clone();
        state.seen = receipt.revisions;
        state.restored.clear();
        state.settled = true;
        Ok(())
    })
}
fn next_epoch(epoch: u64) -> fastctx_storage::Result<u64> {
    epoch.checked_add(1).ok_or_else(|| fastctx_storage::Error::new(words::EPOCH_EXHAUSTED))
}
pub(super) fn compacted(world: &WorldDir, id: &ResidentId, marker: &str) -> io::Result<()> {
    reset(world, id, marker, true)
}
pub(super) fn reopened(
    world: &WorldDir,
    id: &ResidentId,
    marker: &str,
    receipt: &Receipt,
) -> io::Result<Option<Receipt>> {
    exists(world, id)?;
    let (marker, receipt) = (marker.to_string(), receipt.clone());
    super::records::state(world, id, move |_, state| {
        if state.compacted.as_deref() == Some(&marker) {
            return Ok(None);
        }
        state.epoch = next_epoch(state.epoch)?;
        state.opening = receipt.revisions.clone();
        state.seen = receipt.revisions.clone();
        state.restored.clear();
        state.settled = false;
        state.compacted = Some(marker);
        Ok(Some(receipt))
    })
}
pub(super) fn forked(world: &WorldDir, id: &ResidentId) -> io::Result<()> {
    reset(world, id, "fork", false)
}
fn reset(world: &WorldDir, id: &ResidentId, marker: &str, opening: bool) -> io::Result<()> {
    exists(world, id)?;
    let marker = marker.to_string();
    super::records::state(world, id, move |_, state| reset_state(state, &marker, opening))
}
fn reset_state(state: &mut State, marker: &str, opening: bool) -> fastctx_storage::Result<()> {
    if state.compacted.as_deref() == Some(marker) {
        return Ok(());
    }
    state.epoch = next_epoch(state.epoch)?;
    state.seen = if opening { state.opening.clone() } else { BTreeMap::new() };
    state.restored.clear();
    state.settled = false;
    state.compacted = Some(marker.into());
    Ok(())
}
pub(super) fn reset_in(rows: &crate::records::Rows<'_>, marker: &str) -> fastctx_storage::Result<()> {
    let mut state = rows.get(super::records::STATE, "current")?.unwrap_or_default();
    reset_state(&mut state, marker, true)?;
    rows.put(super::records::STATE, "current", &state)
}

pub(super) fn collect(
    world: &WorldDir,
    me: &Address,
    bodies: &super::memory::Bodies,
) -> io::Result<(Vec<String>, Receipt)> {
    exists(world, &me.resident)?;
    super::records::read(world, &me.resident, |state, blocks| {
        let mut notes = Vec::new();
        let mut revisions = BTreeMap::new();
        let mut versions = BTreeMap::new();
        for (name, block) in with_bodies(&me.resident, blocks, bodies) {
            revisions.insert(name.clone(), block.revision);
            versions.insert(name.clone(), block.version.clone());
            if (block.text.is_empty() && !state.seen.contains_key(&name) && state.compacted.is_none())
                || state.seen.get(&name) == Some(&block.revision)
            {
                continue;
            }
            let restoring =
                state.compacted.is_some() && !state.settled && state.restored.get(&name) != Some(&state.epoch);
            if block.by.as_ref() == Some(me) && (!restoring || block.written_epoch == state.epoch) {
                continue;
            }
            let fact = if restoring { words::restored(&name) } else { block.fact };
            let text = if block.text.is_empty() {
                words::removed(&name)
            } else if restoring || block.full {
                block.text
            } else {
                String::new()
            };
            notes.push(words::heading(&name, &format!("{fact}\n{text}")));
        }
        let shown = 0..notes.len();
        Ok((notes, Receipt { epoch: state.epoch, revisions, versions, reserved: false, shown }))
    })
}

pub(super) fn seen_revision(rows: &crate::records::Rows<'_>, name: &str) -> fastctx_storage::Result<Option<u64>> {
    Ok(rows.get::<State>(super::records::STATE, "current")?.and_then(|state| state.seen.get(name).copied()))
}

pub(super) fn acknowledge(world: &WorldDir, id: &ResidentId, receipt: &Receipt) -> io::Result<Option<Receipt>> {
    exists(world, id)?;
    let receipt = receipt.clone();
    super::records::state(world, id, move |rows, state| {
        if state.epoch != receipt.epoch {
            return Ok(None);
        }
        let mut accepted = receipt.clone();
        let current = super::records::blocks(rows)?;
        for (name, revision) in &receipt.revisions {
            let seen = state.seen.entry(name.clone()).or_default();
            if *seen > *revision {
                accepted.versions.remove(name);
            }
            *seen = (*seen).max(*revision);
            if current.get(name).is_some_and(|block| block.revision <= *revision) {
                state.restored.insert(name.clone(), state.epoch);
            }
        }
        state.settled |=
            current.iter().all(|(name, block)| state.seen.get(name).is_some_and(|seen| *seen >= block.revision));
        Ok(Some(accepted))
    })
}
pub(super) fn migrate(node: &crate::Node, id: &ResidentId) -> io::Result<()> {
    super::records::commit(
        &node.world,
        id,
        4096,
        super::projection::Selection::All,
        |bodies: &Vec<(String, Block)>| {
            for (name, block) in bodies {
                node.context_bodies.put(id, name, &block.source, &block.text);
                node.context_bodies.put(id, &format!("{name}-fact"), &block.source, &block.fact);
            }
        },
        |rows| {
            let mut bodies = Vec::new();
            for name in ["plan", "skills"] {
                super::archived::mark(rows, name)?;
                if let Some(mut block) = rows.get::<Block>(super::records::BLOCKS, name)?
                    && !block.transient
                {
                    bodies.push((name.to_string(), block.clone()));
                    block.text.clear();
                    block.fact.clear();
                    block.transient = true;
                    set_block(rows, name, block, false)?;
                }
            }
            Ok(bodies)
        },
    )
    .map(|_| ())
}
pub(super) fn initialize(world: &WorldDir, id: &ResidentId) -> io::Result<()> {
    exists(world, id)?;
    super::records::state(world, id, |_, _| Ok(()))
}
