//! Durable call snapshots and append-only late updates use explicit host identity.

use super::*;
use fastctx_types::run_facts::JobFacts;
use fastctx_types::{MachineName, ResidentId};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct StoredCall {
    call_id: String,
    facts: RunFacts,
    sequence: u64,
    launched: bool,
    #[serde(default)]
    script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unavailable: Option<String>,
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
    fastctx_types::run_facts::merge_steps(&mut stored.facts.steps, &update.steps);
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
    super::log::read(&path.with_extension("updates"), |row| {
        apply(&mut stored, &serde_json::from_slice(row).map_err(io::Error::other)?)
    })?;
    Ok(Some(stored))
}

pub(super) fn get(directory: &ConversationDir, id: &str) -> io::Result<Option<RunFacts>> {
    let _transaction = crate::resident_storage::lock();
    loaded(&path(directory, id)).map(|stored| stored.filter(|stored| stored.unavailable.is_none()).map(|stored| stored.facts))
}

pub(crate) fn begin(node: &Node, resident: &ResidentId, id: &str, machine: MachineName, turn: Option<String>, script: &str) -> io::Result<()> {
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
            script: script.to_string(),
            unavailable: None,
            facts: RunFacts {
                turn, record: None, machine,
                state: fastctx_types::run::RunState::Running,
                job: None,
                output: None,
                visited: Vec::new(),
                writes: Vec::new(),
                spans: Vec::new(),
                notes: Vec::new(),
                steps: Vec::new(),
            },
        },
    )
}

pub(super) fn keep(directory: &ConversationDir, id: &str, facts: &RunFacts, display: bool) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let file = path(directory, id);
    let previous = loaded(&file)?;
    let mut stored =
        StoredCall { call_id: id.into(), facts: facts.clone(), sequence: 0, launched: facts.job.is_some(), script: String::new(), unavailable: None };
    if !display {
        stored.facts.spans.clear();
    }
    if let Some(previous) = previous {
        stored.script = previous.script;
        stored.facts.turn = previous.facts.turn.or(stored.facts.turn);
        stored.facts.record = stored.facts.record.or(previous.facts.record);
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
        // Steps told in jobs an `on` sent elsewhere reach the call only as
        // updates.
        fastctx_types::run_facts::merge_steps(&mut stored.facts.steps, &previous.facts.steps);
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

/// Retains the original turn and script when an older executor has no detailed facts.
pub(crate) fn legacy_reply(node: &Node, resident: &ResidentId, id: &str, reply: &RunReply) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let file = path(&node.world.conversation(resident), id);
    let mut stored = loaded(&file)?.ok_or_else(|| io::Error::other("the original run call is unavailable"))?;
    stored.facts.machine = reply.machine.clone();
    stored.facts.state = reply.state.clone();
    stored.facts.job = Some(reply.job.clone());
    stored.launched = true;
    stored.unavailable = Some(format!("{} did not supply detailed run facts; update that executor before auditing this turn", reply.machine));
    save(&file, &stored)
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
        script: String::new(),
        unavailable: None,
        facts: RunFacts {
            turn: None, record: None, machine,
            state: fastctx_types::run::RunState::Running,
            job: None,
            output: None,
            visited: Vec::new(),
            writes: Vec::new(),
            spans: Vec::new(),
            notes: Vec::new(),
            steps: Vec::new(),
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
        let Some(stored) = read(&file)? else { return Ok(()) };
        let mut observed = update.clone();
        if observed.turn.is_some() && stored.facts.turn.is_some() && observed.turn != stored.facts.turn {
            return Err(io::Error::other("job facts belong to a different turn"));
        }
        observed.turn = stored.facts.turn.or(observed.turn);
        let jobs = file.with_extension("jobs");
        fastctx_platform::ensure_private_dir(&jobs)?;
        let key = fastctx_hands::files::content_hash(format!("{}:{}", update.machine, update.job).as_bytes());
        let latest = jobs.join(format!("{key}.json"));
        let previous: Option<JobFacts> = match std::fs::read(&latest) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if previous.is_none_or(|previous| previous.sequence < observed.sequence) {
            publish(&latest, &serde_json::to_vec(&observed)?, Permissions::Private)?;
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

/// Reads exact turn associations; absent associations are never guessed from timestamps.
pub(crate) fn evidence(directory: &ConversationDir, turn: &str) -> io::Result<Vec<fastctx_types::turn_evidence::CallEvidence>> {
    read_evidence(directory, Some(turn))
}

pub(crate) fn evidence_all(directory: &ConversationDir) -> io::Result<Vec<fastctx_types::turn_evidence::CallEvidence>> {
    read_evidence(directory, None)
}

fn read_evidence(directory: &ConversationDir, turn: Option<&str>) -> io::Result<Vec<fastctx_types::turn_evidence::CallEvidence>> {
    let _transaction = crate::resident_storage::lock();
    let mut result = Vec::new();
    let entries = match std::fs::read_dir(directory.calls()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(result),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") { continue; }
        let Some(stored) = loaded(&path)? else { continue };
        if turn.is_none_or(|turn| stored.facts.turn.as_deref() == Some(turn)) {
            if let Some(reason) = stored.unavailable {
                if turn.is_some() { return Err(io::Error::other(reason)); }
                continue;
            }
            result.push(fastctx_types::turn_evidence::CallEvidence {
                call_id: stored.call_id, script: stored.script, source: path.to_string_lossy().into_owned(), facts: stored.facts,
            });
        }
    }
    result.sort_by(|a, b| a.call_id.cmp(&b.call_id));
    Ok(result)
}

#[derive(Serialize)]
pub(crate) struct ObservedCall {
    pub call: fastctx_types::turn_evidence::CallEvidence,
    pub unavailable: Option<String>,
}

/// Reads each call once, releasing the record transaction between files.
/// Legacy executors' missing detail is retained as an explicit observation.
pub(crate) fn observations(directory: &ConversationDir) -> io::Result<Vec<ObservedCall>> {
    let entries = match std::fs::read_dir(directory.calls()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut result = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") { continue; }
        let stored = {
            let _transaction = crate::resident_storage::lock();
            loaded(&path)?
        };
        let Some(stored) = stored else { continue };
        result.push(ObservedCall {
            call: fastctx_types::turn_evidence::CallEvidence {
                call_id: stored.call_id, script: stored.script, source: path.to_string_lossy().into_owned(), facts: stored.facts,
            },
            unavailable: stored.unavailable,
        });
    }
    result.sort_by(|a, b| a.call.call_id.cmp(&b.call.call_id));
    Ok(result)
}

pub(crate) fn job_updates(directory: &ConversationDir) -> io::Result<Vec<JobFacts>> {
    let _transaction = crate::resident_storage::lock();
    let mut result = Vec::new();
    let entries = match std::fs::read_dir(directory.calls()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(result),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "jobs") { continue; }
        for job in std::fs::read_dir(path)? {
            let path = job?.path();
            if path.extension().is_some_and(|extension| extension == "json") {
                result.push(serde_json::from_slice(&std::fs::read(path)?)?);
            }
        }
    }
    Ok(result)
}
