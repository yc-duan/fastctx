//! Durable knowledge of scratch paths and the revisions actually delivered.

use fastctx_types::{ResidentId, layout::WorldDir, scratch::Descriptions};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    described: bool,
    known: BTreeMap<String, Entry>,
    seen: BTreeMap<String, u64>,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    revision: u64,
    path: Result<String, String>,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Receipt {
    revisions: BTreeMap<String, u64>,
    #[serde(skip)]
    pub(crate) shown: std::ops::Range<usize>,
    #[serde(skip)]
    pub(crate) reserved: bool,
}

fn read(world: &WorldDir, id: &ResidentId) -> io::Result<State> {
    let home = world.conversation(id);
    if world.deleted().join(format!("{id}.json")).try_exists()?
        || home.path().join("deleting").try_exists()?
        || !home.path().try_exists()?
    {
        return Err(io::Error::new(io::ErrorKind::NotFound, "the conversation is unavailable"));
    }
    match std::fs::read(home.path().join("scratch-locations.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error),
    }
}
fn write(world: &WorldDir, id: &ResidentId, state: &State) -> io::Result<()> {
    fastctx_platform::publish(
        &world.conversation(id).path().join("scratch-locations.json"),
        &serde_json::to_vec(state).map_err(io::Error::other)?,
        fastctx_platform::Permissions::Private,
    )
}

pub(super) fn observe(world: &WorldDir, id: &ResidentId, paths: Descriptions, described: bool) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let mut state = read(world, id)?;
    // Only the opening catalog proves a path reached the model. Hosts differ
    // on whether later tools/list responses replace their retained definitions.
    if described && state.described {
        return Ok(());
    }
    let mut changed = described;
    if described {
        state.described = true;
    }
    for (machine, path) in paths {
        let revision = match state.known.get(&machine) {
            Some(entry) if entry.path == path => entry.revision,
            previous => {
                let revision = previous.map_or(Ok(1), |entry| {
                    entry.revision.checked_add(1).ok_or_else(|| io::Error::other("scratch revision exhausted"))
                })?;
                state.known.insert(machine.clone(), Entry { revision, path });
                changed = true;
                revision
            }
        };
        if described && state.seen.get(&machine) != Some(&revision) {
            state.seen.insert(machine, revision);
            changed = true;
        }
    }
    if changed {
        write(world, id, &state)?;
    }
    Ok(())
}

pub(super) fn collect(world: &WorldDir, id: &ResidentId, start: usize) -> io::Result<(Vec<String>, Receipt)> {
    let _transaction = crate::resident_storage::lock();
    let state = read(world, id)?;
    let mut receipt = Receipt::default();
    let mut notes = Vec::new();
    let mut bytes = 0;
    for (machine, entry) in state.known {
        if state.seen.get(&machine).is_some_and(|seen| *seen >= entry.revision) {
            continue;
        }
        let note = fastctx_types::scratch::words::note(&machine, &entry.path);
        if bytes + note.len() > 32 * 1024 && !notes.is_empty() {
            break;
        }
        bytes += note.len();
        notes.push(note);
        receipt.revisions.insert(machine, entry.revision);
    }
    receipt.shown = start..start + notes.len();
    Ok((notes, receipt))
}

pub(super) fn acknowledge(world: &WorldDir, id: &ResidentId, receipt: &Receipt) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let mut state = read(world, id)?;
    let mut changed = false;
    for (machine, revision) in &receipt.revisions {
        let seen = state.seen.entry(machine.clone()).or_default();
        if *seen < *revision {
            *seen = *revision;
            changed = true;
        }
    }
    if changed {
        write(world, id, &state)?;
    }
    Ok(())
}
