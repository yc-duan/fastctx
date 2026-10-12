//! Durable call snapshots and append-only late updates use explicit host identity.

use super::*;
use fastctx_types::run_facts::JobFacts;
use fastctx_types::{MachineName, ResidentId};
use serde::{Deserialize, Serialize};
use std::io::BufRead;

#[derive(Serialize, Deserialize)]
struct StoredCall {
    call_id: String,
    facts: RunFacts,
    sequence: u64,
    launched: bool,
}

fn read(path: &Path) -> io::Result<Option<StoredCall>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn save(path: &Path, stored: &StoredCall) -> io::Result<()> {
    fastctx_platform::ensure_private_dir(path.parent().expect("call storage has a parent"))?;
    publish(path, &serde_json::to_vec(stored)?, Permissions::Private)
}

fn delete_log(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn merge_writes(target: &mut Vec<WriteFact>, incoming: &[WriteFact]) -> io::Result<()> {
    let mut known: std::collections::HashMap<_, _> =
        target.iter().enumerate().map(|(i, fact)| (fact.id.clone(), i)).collect();
    for fact in incoming {
        if let Some(&i) = known.get(&fact.id) {
            if target[i] != *fact {
                return Err(io::Error::other("write fact identity changed"));
            }
        } else {
            known.insert(fact.id.clone(), target.len());
            target.push(fact.clone());
        }
    }
    Ok(())
}

fn terminal(state: &fastctx_types::run::RunState) -> bool {
    !matches!(state, fastctx_types::run::RunState::Running | fastctx_types::run::RunState::Exited { .. })
}

fn apply(stored: &mut StoredCall, update: &JobFacts) -> io::Result<()> {
    if stored.call_id != update.call_id {
        return Err(io::Error::other("call fact identity changed"));
    }
    stored.launched = true;
    merge_writes(&mut stored.facts.writes, &update.writes)?;
    for visit in &update.visited {
        if !stored.facts.visited.contains(visit) {
            stored.facts.visited.push(visit.clone());
        }
    }
    if update.primary && stored.facts.job.is_none() {
        stored.facts.machine = update.machine.clone();
        stored.facts.job = Some(update.job.clone());
        stored.facts.output = update.output.clone();
    }
    if update.primary
        && stored.facts.machine == update.machine
        && stored.facts.job.as_ref() == Some(&update.job)
        && update.sequence >= stored.sequence
    {
        if let Some(output) = &update.output {
            if let Some(previous) = &mut stored.facts.output {
                if previous.machine == output.machine && previous.path == output.path {
                    previous.cut |= output.bytes > previous.bytes;
                    previous.content = output.content.clone();
                    previous.policy = output.policy.clone();
                    previous.bytes = output.bytes;
                    previous.lines = output.lines;
                }
            } else {
                stored.facts.output = Some(output.clone());
            }
        }
        if let Some(notes) = &update.notes {
            stored.facts.notes = notes.clone();
        }
        if let Some(state) = &update.state
            && (!terminal(&stored.facts.state) || terminal(state))
        {
            stored.facts.state = state.clone();
        }
        stored.sequence = update.sequence;
    }
    Ok(())
}

fn loaded(path: &Path) -> io::Result<Option<StoredCall>> {
    let Some(mut stored) = read(path)? else { return Ok(None) };
    let input = match std::fs::File::open(path.with_extension("updates")) {
        Ok(input) => Some(input),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(input) = input {
        for line in io::BufReader::new(input).lines() {
            apply(&mut stored, &serde_json::from_str(&line?).map_err(io::Error::other)?)?;
        }
    }
    Ok(Some(stored))
}

pub(super) fn get(directory: &ConversationDir, id: &str) -> io::Result<Option<RunFacts>> {
    let _transaction = crate::resident_storage::lock();
    loaded(&path(directory, id)).map(|stored| stored.map(|stored| stored.facts))
}

pub(crate) fn begin(node: &Node, resident: &ResidentId, id: &str, machine: MachineName) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    let directory = node.world.conversation(resident);
    let _transaction = crate::resident_storage::lock();
    save(
        &path(&directory, id),
        &StoredCall {
            call_id: id.into(),
            sequence: 0,
            launched: false,
            facts: RunFacts {
                machine,
                state: fastctx_types::run::RunState::Running,
                job: None,
                output: None,
                visited: Vec::new(),
                writes: Vec::new(),
                spans: Vec::new(),
                notes: Vec::new(),
            },
        },
    )
}

pub(super) fn keep(directory: &ConversationDir, id: &str, facts: &RunFacts, display: bool) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let file = path(directory, id);
    let previous = loaded(&file)?;
    let mut stored =
        StoredCall { call_id: id.into(), facts: facts.clone(), sequence: 0, launched: facts.job.is_some() };
    if !display {
        stored.facts.spans.clear();
    }
    if let Some(previous) = previous {
        if let (Some(output), Some(kept)) = (&mut stored.facts.output, &previous.facts.output)
            && output.content.is_none()
            && kept.content.is_some()
            && output.machine == kept.machine
            && output.path == kept.path
        {
            output.content = kept.content.clone();
            output.policy = kept.policy.clone();
            output.bytes = kept.bytes;
            output.lines = kept.lines;
            output.cut |= kept.cut;
            stored.facts.notes = previous.facts.notes.clone();
        }
        merge_writes(&mut stored.facts.writes, &previous.facts.writes)?;
        if terminal(&previous.facts.state) && !terminal(&stored.facts.state) {
            stored.facts.state = previous.facts.state;
        }
        for visit in previous.facts.visited {
            if !stored.facts.visited.contains(&visit) {
                stored.facts.visited.push(visit);
            }
        }
        stored.sequence = previous.sequence;
        stored.launched |= previous.launched;
    }
    save(&file, &stored)?;
    delete_log(&file.with_extension("updates"))
}

pub(crate) fn failed(node: &Node, resident: &ResidentId, id: &str, error: &str) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    let directory = node.world.conversation(resident);
    let _transaction = crate::resident_storage::lock();
    let file = path(&directory, id);
    if let Some(mut stored) = loaded(&file)?
        && !stored.launched
    {
        stored.facts.state = fastctx_types::run::RunState::Failed { error: error.into() };
        save(&file, &stored)?;
    }
    Ok(())
}

pub(crate) fn discard_unlaunched(node: &Node, resident: &ResidentId, id: &str) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    let directory = node.world.conversation(resident);
    let _transaction = crate::resident_storage::lock();
    let file = path(&directory, id);
    if loaded(&file)?.is_some_and(|stored| !stored.launched) {
        delete_log(&file)?;
        delete_log(&file.with_extension("updates"))?;
    }
    Ok(())
}

pub(crate) fn record_failure(node: &Node, resident: &ResidentId, id: &str, error: &str) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    let machine = node.machine();
    let directory = node.world.conversation(resident);
    let _transaction = crate::resident_storage::lock();
    let file = path(&directory, id);
    let previous = loaded(&file)?;
    if previous.as_ref().is_some_and(|stored| {
        stored.launched || matches!(stored.facts.state, fastctx_types::run::RunState::Failed { .. })
    }) {
        return Ok(());
    }
    let mut stored = previous.unwrap_or_else(|| StoredCall {
        call_id: id.into(),
        sequence: 0,
        launched: false,
        facts: RunFacts {
            machine,
            state: fastctx_types::run::RunState::Running,
            job: None,
            output: None,
            visited: Vec::new(),
            writes: Vec::new(),
            spans: Vec::new(),
            notes: Vec::new(),
        },
    });
    stored.facts.state = fastctx_types::run::RunState::Failed { error: error.into() };
    save(&file, &stored)
}

pub(crate) fn receive(node: &Node, resident: &ResidentId, update: &JobFacts) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    let directory = node.world.conversation(resident);
    {
        let _transaction = crate::resident_storage::lock();
        let file = path(&directory, &update.call_id);
        if read(&file)?.is_none() {
            return Ok(());
        }
        let mut bytes = serde_json::to_vec(update)?;
        bytes.push(b'\n');
        super::log::append(&file.with_extension("updates"), &bytes)?;
        if update.primary
            && update.state.as_ref().is_some_and(terminal)
            && let Some(stored) = loaded(&file)?
        {
            save(&file, &stored)?;
            delete_log(&file.with_extension("updates"))?;
        }
    }
    node.watchers.publish(
        resident,
        &SessionEvent {
            at: jiff::Timestamp::now(),
            happened: Happened::Changed {
                turn: None,
                change: fastctx_types::session::SessionChange::RunFactsUpdated {
                    call_id: update.call_id.clone(),
                    update: Box::new(update.clone()),
                },
            },
        },
    );
    Ok(())
}

/// What `get` reads for any call, told by each file's name, size and time
/// of change: a call's facts are replaced whole or appended to.
pub(crate) fn stamp(directory: &ConversationDir, digest: &mut impl FnMut(&[u8])) -> io::Result<()> {
    crate::session_record::stamp_files(&directory.calls(), digest)
}
