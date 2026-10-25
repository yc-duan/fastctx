//! Atomic block replacements and delivery cursors. No host or network I/O is
//! performed while the short conversation-record transaction is held.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use fastctx_types::{Address, ResidentId, layout::WorldDir};
use serde::{Deserialize, Serialize};

use super::{Change, Receipt, words};

pub(super) const BLOCK_BYTES: usize = 16 * 1024;
const BLOCK_COUNT: usize = 8;
const FACT_BYTES: usize = 1024;

#[derive(Serialize, Deserialize)]
pub(super) struct Block {
    pub revision: u64,
    pub written_epoch: u64,
    pub source: String,
    pub version: String,
    pub text: String,
    pub by: Option<Address>,
    pub fact: String,
    pub full: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
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

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    fastctx_platform::publish(
        path,
        &serde_json::to_vec(value).map_err(io::Error::other)?,
        fastctx_platform::Permissions::Private,
    )
}

fn exists(world: &WorldDir, id: &ResidentId) -> io::Result<()> {
    let home = world.conversation(id);
    if world.deleted().join(format!("{id}.json")).try_exists()?
        || home.path().join("deleting").try_exists()?
        || home.path().join("removed").try_exists()?
        || !home.path().try_exists()?
    {
        return Err(io::Error::new(io::ErrorKind::NotFound, words::UNAVAILABLE));
    }
    Ok(())
}

fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn blocks(world: &WorldDir, id: &ResidentId) -> io::Result<BTreeMap<String, Block>> {
    let entries = match std::fs::read_dir(directory(world, id)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error),
    };
    let mut blocks = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|name| name.to_str()).filter(|name| valid(name)) else { continue };
        if let Some(block) = read(&path)? {
            blocks.insert(name.to_string(), block);
        }
    }
    Ok(blocks)
}

fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut end = limit.saturating_sub(128);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &text[..end], words::truncated(end, text.len()))
}

pub(super) fn set(world: &WorldDir, id: &ResidentId, name: &str, text: &str, change: Change<'_>) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    set_locked(world, id, name, text, change)
}

/// The caller holds resident_storage across reading its local source and publishing it.
pub(super) fn set_locked(
    world: &WorldDir,
    id: &ResidentId,
    name: &str,
    text: &str,
    change: Change<'_>,
) -> io::Result<()> {
    if !valid(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, words::INVALID_NAME));
    }
    let source = crate::profile::session::hash(text);
    exists(world, id)?;
    let state: State = read(&state_path(world, id))?.unwrap_or_default();
    let path = directory(world, id).join(format!("{name}.json"));
    let old: Option<Block> = read(&path)?;
    if old.as_ref().is_some_and(|old| old.source == source && old.version == change.version)
        || (old.is_none() && text.is_empty() && change.version.is_empty())
    {
        return Ok(());
    }
    if old.is_none() && blocks(world, id)?.len() >= BLOCK_COUNT {
        return Err(io::Error::other(words::TOO_MANY));
    }
    let revision = old
        .map_or(Ok(1), |old| old.revision.checked_add(1).ok_or_else(|| io::Error::other(words::REVISION_EXHAUSTED)))?;
    fastctx_platform::ensure_private_dir(&directory(world, id))?;
    write(
        &path,
        &Block {
            revision,
            written_epoch: state.epoch,
            source,
            version: change.version.into(),
            text: bounded(text, BLOCK_BYTES),
            by: change.by.cloned(),
            fact: bounded(change.fact, FACT_BYTES),
            full: change.full,
        },
    )
}

pub(super) fn opening(world: &WorldDir, id: &ResidentId) -> io::Result<(String, Receipt)> {
    let _transaction = crate::resident_storage::lock();
    exists(world, id)?;
    let state: State = read(&state_path(world, id))?.unwrap_or_default();
    let mut text = Vec::new();
    let mut revisions = BTreeMap::new();
    let mut versions = BTreeMap::new();
    for (name, block) in blocks(world, id)? {
        revisions.insert(name.clone(), block.revision);
        versions.insert(name.clone(), block.version.clone());
        if !block.text.is_empty() {
            text.push(words::heading(&name, &block.text));
        }
    }
    Ok((text.join("\n\n"), Receipt { epoch: state.epoch, revisions, versions, reserved: false, shown: 0..0 }))
}

pub(super) fn instructed(world: &WorldDir, id: &ResidentId, receipt: &Receipt) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    exists(world, id)?;
    let mut state: State = read(&state_path(world, id))?.unwrap_or_default();
    // A host configuration is a new opening, including when a thread resumes.
    state.epoch = state.epoch.checked_add(1).ok_or_else(|| io::Error::other(words::EPOCH_EXHAUSTED))?;
    state.opening = receipt.revisions.clone();
    state.seen = receipt.revisions.clone();
    state.restored.clear();
    state.settled = true;
    write(&state_path(world, id), &state)
}

pub(super) fn compacted(world: &WorldDir, id: &ResidentId, marker: &str) -> io::Result<()> {
    reset(world, id, marker, true)
}

/// The host compacted the conversation and began it again with `receipt`'s
/// revisions, which is all it holds of its blocks now: what tool answers
/// told it before is gone, and their late acknowledgements belong to the
/// old epoch. Applied once for each `marker`.
pub(super) fn reopened(
    world: &WorldDir,
    id: &ResidentId,
    marker: &str,
    receipt: &Receipt,
) -> io::Result<Option<Receipt>> {
    let _transaction = crate::resident_storage::lock();
    exists(world, id)?;
    let mut state: State = read(&state_path(world, id))?.unwrap_or_default();
    if state.compacted.as_deref() == Some(marker) {
        return Ok(None);
    }
    state.epoch = state.epoch.checked_add(1).ok_or_else(|| io::Error::other(words::EPOCH_EXHAUSTED))?;
    state.opening = receipt.revisions.clone();
    state.seen = receipt.revisions.clone();
    state.restored.clear();
    state.settled = false;
    state.compacted = Some(marker.into());
    write(&state_path(world, id), &state)?;
    Ok(Some(receipt.clone()))
}

pub(super) fn forked(world: &WorldDir, id: &ResidentId) -> io::Result<()> {
    reset(world, id, "fork", false)
}

fn reset(world: &WorldDir, id: &ResidentId, marker: &str, opening: bool) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    exists(world, id)?;
    let mut state: State = read(&state_path(world, id))?.unwrap_or_default();
    if state.compacted.as_deref() == Some(marker) {
        return Ok(());
    }
    state.epoch = state.epoch.checked_add(1).ok_or_else(|| io::Error::other(words::EPOCH_EXHAUSTED))?;
    state.seen = if opening { state.opening.clone() } else { BTreeMap::new() };
    state.restored.clear();
    state.settled = false;
    state.compacted = Some(marker.into());
    write(&state_path(world, id), &state)
}

pub(super) fn collect(world: &WorldDir, me: &Address) -> io::Result<(Vec<String>, Receipt)> {
    let _transaction = crate::resident_storage::lock();
    exists(world, &me.resident)?;
    let state: State = read(&state_path(world, &me.resident))?.unwrap_or_default();
    let mut notes = Vec::new();
    let mut revisions = BTreeMap::new();
    let mut versions = BTreeMap::new();
    for (name, block) in blocks(world, &me.resident)? {
        revisions.insert(name.clone(), block.revision);
        versions.insert(name.clone(), block.version.clone());
        if (block.text.is_empty() && !state.seen.contains_key(&name) && state.compacted.is_none())
            || state.seen.get(&name) == Some(&block.revision)
        {
            continue;
        }
        let restoring = state.compacted.is_some() && !state.settled && state.restored.get(&name) != Some(&state.epoch);
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
}

pub(super) fn acknowledge(world: &WorldDir, id: &ResidentId, receipt: &Receipt) -> io::Result<Option<Receipt>> {
    let _transaction = crate::resident_storage::lock();
    exists(world, id)?;
    let path = state_path(world, id);
    let mut state: State = read(&path)?.unwrap_or_default();
    if state.epoch != receipt.epoch {
        return Ok(None);
    }
    let mut accepted = receipt.clone();
    let current = blocks(world, id)?;
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
    write(&path, &state)?;
    Ok(Some(accepted))
}

pub(super) fn initialize(world: &WorldDir, id: &ResidentId) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    exists(world, id)?;
    fastctx_platform::ensure_private_dir(&directory(world, id))?;
    let path = state_path(world, id);
    if read::<State>(&path)?.is_none() {
        write(&path, &State::default())?;
    }
    Ok(())
}
