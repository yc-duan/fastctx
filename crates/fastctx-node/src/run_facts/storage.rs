//! Indexed snapshots and per-key updates; no conversation log is replayed by a read.

use super::snapshot::{StoredCall, terminal};
use super::sql;
use super::*;
use fastctx_storage::{
    Database, Result as StoredResult,
    rusqlite::{Connection, OptionalExtension},
};
use fastctx_types::run_facts::JobFacts;
use fastctx_types::{MachineName, ResidentId};
use serde::Serialize;

pub(crate) fn database(directory: &ConversationDir) -> io::Result<(Database, String)> {
    let (database, conversation) = crate::record_database::get(directory)?;
    super::migrate::ensure(directory, &database, &conversation)?;
    Ok((database, conversation))
}

pub(super) fn get(directory: &ConversationDir, id: &str) -> io::Result<Option<RunFacts>> {
    let (database, conversation) = database(directory)?;
    database
        .read_now(|db| {
            let transaction = db.unchecked_transaction()?;
            let result = sql::load(&transaction, &conversation, id)?;
            transaction.commit()?;
            Ok(result.filter(|stored| stored.unavailable.is_none()).map(|stored| stored.facts))
        })
        .map_err(io::Error::other)
}

fn empty(call: &str, machine: MachineName, turn: Option<String>, script: &str) -> StoredCall {
    StoredCall {
        call_id: call.into(),
        sequence: 0,
        launched: false,
        script: script.into(),
        unavailable: None,
        facts: RunFacts {
            turn,
            record: None,
            machine,
            state: fastctx_types::run::RunState::Running,
            job: None,
            output: None,
            visited: Vec::new(),
            writes: Vec::new(),
            spans: Vec::new(),
            notes: Vec::new(),
            steps: Vec::new(),
            folded_steps: Vec::new(),
        },
    }
}

fn mutate<T: Send + 'static>(
    directory: &ConversationDir,
    bytes: usize,
    action: impl FnOnce(&Connection, &str) -> StoredResult<T> + Send + 'static,
) -> io::Result<T> {
    let (database, conversation) = database(directory)?;
    database
        .write_now(bytes, move |db| {
            let before = db.total_changes();
            let result = action(db, &conversation)?;
            if db.total_changes() != before {
                sql::changed(db, &conversation)?;
            }
            Ok(result)
        })
        .map_err(io::Error::other)
}

pub(crate) fn begin(
    node: &Node,
    resident: &ResidentId,
    id: &str,
    machine: MachineName,
    turn: Option<String>,
    script: &str,
) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    let stored = empty(id, machine, turn, script);
    mutate(&node.world.conversation(resident), script.len() + id.len(), move |db, conversation| {
        if sql::head(db, conversation, &stored.call_id)?.is_none() {
            sql::save(db, conversation, &stored)?;
        }
        Ok(())
    })
}

pub(super) fn keep(directory: &ConversationDir, id: &str, facts: &RunFacts, display: bool) -> io::Result<()> {
    let mut stored = StoredCall {
        call_id: id.into(),
        facts: facts.clone(),
        sequence: 0,
        launched: facts.job.is_some(),
        script: String::new(),
        unavailable: None,
    };
    if !display {
        stored.facts.spans.clear();
    }
    let (database, conversation) = database(directory)?;
    let projections =
        super::projections::Call::prepare(&database, &conversation, &mut stored, fastctx_storage::Priority::Normal)
            .map_err(io::Error::other)?;
    let members = database
        .read_now(|db| {
            sql::difference(db, &conversation, id, &stored.facts.writes, &stored.facts.steps, &stored.facts.visited)
        })
        .map_err(io::Error::other)?;
    super::acceptance::keep_members(&database, &conversation, id, members)?;
    stored.facts.writes.clear();
    stored.facts.steps.clear();
    stored.facts.visited.clear();
    let bytes = sql::encode(&stored).map_err(io::Error::other)?.len();
    mutate(directory, bytes, move |db, conversation| {
        let mut preserve_notes = false;
        if let Some(previous) = sql::head(db, conversation, &stored.call_id)? {
            stored.script = previous.script;
            stored.facts.turn = previous.facts.turn.or(stored.facts.turn);
            stored.facts.record = stored.facts.record.or(previous.facts.record);
            if let (Some(output), Some(kept)) = (&mut stored.facts.output, &previous.facts.output) {
                preserve_notes |= super::measurements::preserve(output, kept);
            }
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
                preserve_notes = true;
            }
            if terminal(&previous.facts.state) && !terminal(&stored.facts.state) {
                stored.facts.state = previous.facts.state;
            }
            stored.sequence = previous.sequence;
            stored.launched |= previous.launched;
        }
        projections.install(db, conversation, &stored.call_id, preserve_notes)?;
        sql::save(db, conversation, &stored)
    })
}

pub(crate) fn failed(node: &Node, resident: &ResidentId, id: &str, error: &str) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    let (id, error) = (id.to_string(), error.to_string());
    mutate(&node.world.conversation(resident), id.len() + error.len(), move |db, conversation| {
        if let Some(mut stored) = sql::head(db, conversation, &id)?
            && !stored.launched
        {
            stored.facts.state = fastctx_types::run::RunState::Failed { error };
            sql::save(db, conversation, &stored)?;
        }
        Ok(())
    })
}

pub(crate) fn legacy_reply(node: &Node, resident: &ResidentId, id: &str, reply: &RunReply) -> io::Result<()> {
    let (id, reply) = (id.to_string(), reply.clone());
    mutate(&node.world.conversation(resident), id.len() + 1024, move |db, conversation| {
        let mut stored = sql::head(db, conversation, &id)?
            .ok_or_else(|| fastctx_storage::Error::new("the original run call is unavailable"))?;
        stored.facts.machine = reply.machine.clone();
        stored.facts.state = reply.state;
        stored.facts.job = Some(reply.job);
        stored.launched = true;
        stored.unavailable = Some(format!(
            "{} did not supply detailed run facts; update that executor before auditing this turn",
            reply.machine
        ));
        sql::save(db, conversation, &stored)
    })
}

pub(crate) fn record_failure(node: &Node, resident: &ResidentId, id: &str, error: &str) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    let (id, error, machine) = (id.to_string(), error.to_string(), node.machine());
    mutate(&node.world.conversation(resident), id.len() + error.len(), move |db, conversation| {
        let mut stored = sql::head(db, conversation, &id)?.unwrap_or_else(|| empty(&id, machine, None, ""));
        if stored.launched || matches!(stored.facts.state, fastctx_types::run::RunState::Failed { .. }) {
            return Ok(());
        }
        stored.facts.state = fastctx_types::run::RunState::Failed { error };
        sql::save(db, conversation, &stored)
    })
}

pub(crate) fn receive(node: &Node, resident: &ResidentId, update: &JobFacts) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    let (database, conversation) = database(&node.world.conversation(resident))?;
    super::acceptance::receive(&database, &conversation, update, |observed| {
        node.watchers.publish(
            resident,
            &SessionEvent {
                at: jiff::Timestamp::now(),
                happened: Happened::Changed {
                    turn: None,
                    change: fastctx_types::session::SessionChange::RunFactsUpdated {
                        call_id: observed.call_id.clone(),
                        update: Box::new(observed),
                    },
                },
            },
        );
    })?;
    Ok(())
}

pub(crate) fn stamp(directory: &ConversationDir, digest: &mut impl FnMut(&[u8])) -> io::Result<()> {
    let (database, conversation) = database(directory)?;
    let version: i64 = database
        .read_now(|db| {
            Ok(db
                .query_row("SELECT version FROM run_versions WHERE conversation=?1", [conversation], |row| row.get(0))
                .optional()?
                .unwrap_or(0))
        })
        .map_err(io::Error::other)?;
    digest(&version.to_be_bytes());
    Ok(())
}

pub(crate) fn evidence(
    directory: &ConversationDir,
    turn: &str,
) -> io::Result<Vec<fastctx_types::turn_evidence::CallEvidence>> {
    read_evidence(directory, Some(turn))
}
fn read_evidence(
    directory: &ConversationDir,
    turn: Option<&str>,
) -> io::Result<Vec<fastctx_types::turn_evidence::CallEvidence>> {
    let observations = read_calls(directory, turn)?;
    let mut result = Vec::new();
    for observed in observations {
        if let Some(reason) = observed.unavailable {
            if turn.is_some() {
                return Err(io::Error::other(reason));
            }
        } else {
            result.push(observed.call);
        }
    }
    Ok(result)
}
#[derive(Serialize)]
pub(crate) struct ObservedCall {
    pub call: fastctx_types::turn_evidence::CallEvidence,
    pub unavailable: Option<String>,
}
pub(crate) fn observations(directory: &ConversationDir) -> io::Result<Vec<ObservedCall>> {
    read_calls(directory, None)
}
fn read_calls(directory: &ConversationDir, turn: Option<&str>) -> io::Result<Vec<ObservedCall>> {
    let (database, conversation) = database(directory)?;
    database
        .read_now(|db| {
            let transaction = db.unchecked_transaction()?;
            let calls: Vec<String> = if let Some(turn) = turn {
                transaction
                    .prepare_cached("SELECT call FROM run_calls WHERE conversation=?1 AND turn=?2 ORDER BY call")?
                    .query_map((&conversation, turn), |row| row.get(0))?
                    .collect::<std::result::Result<_, _>>()?
            } else {
                transaction
                    .prepare_cached("SELECT call FROM run_calls WHERE conversation=?1 ORDER BY call")?
                    .query_map([&conversation], |row| row.get(0))?
                    .collect::<std::result::Result<_, _>>()?
            };
            let mut result = Vec::new();
            for call in calls {
                let stored = sql::load(&transaction, &conversation, &call)?.expect("call in read snapshot");
                result.push(ObservedCall {
                    call: fastctx_types::turn_evidence::CallEvidence {
                        source: format!("conversation {conversation}, call {}", stored.call_id),
                        call_id: stored.call_id,
                        script: stored.script,
                        facts: stored.facts,
                    },
                    unavailable: stored.unavailable,
                });
            }
            transaction.commit()?;
            Ok(result)
        })
        .map_err(io::Error::other)
}

pub(crate) fn job_updates(directory: &ConversationDir, turn: Option<&str>) -> io::Result<Vec<JobFacts>> {
    let (database, conversation) = database(directory)?;
    database
        .read_now(|db| {
            let tx = db.unchecked_transaction()?;
            if tx.query_row("SELECT call FROM run_jobs WHERE conversation=?1 AND NOT json_valid(body) LIMIT 1",[&conversation],|row|row.get::<_,String>(0)).optional()?.is_some(){return Err(fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Corrupt,"job metadata is unreadable"));}
            let query=if turn.is_some(){"SELECT body FROM run_jobs WHERE conversation=?1 AND json_valid(body) AND json_extract(body,'$.turn')=?2 ORDER BY call,machine,job"}else{"SELECT body FROM run_jobs WHERE conversation=?1 AND ?2 IS NULL ORDER BY call,machine,job"};
            let mut query=tx.prepare_cached(query)?;
            query.query_map((&conversation,turn), |row| row.get::<_, Vec<u8>>(0))?
                .map(|row| {
                    let mut update=sql::decode(&row?)?;
                    sql::job_source(&tx,&conversation,&mut update)?;
                    Ok(update)
                }).collect()
        })
        .map_err(io::Error::other)
}

/// The wait detector needs only unfinished execution locations, not retained
/// output, write details or every historical call. Partial indices exclude
/// completed history before any application deserialization.
pub(crate) fn pending_machines(directory: &ConversationDir) -> io::Result<Vec<(MachineName, Option<jiff::Timestamp>)>> {
    let (database, conversation) = database(directory)?;
    database.read_now(|db| {
        for table in ["run_calls","run_jobs"] {
            if db.query_row(&format!("SELECT call FROM {table} WHERE conversation=?1 AND NOT json_valid(body) LIMIT 1"),[&conversation],|row|row.get::<_,String>(0)).optional()?.is_some() {
                return Err(fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Corrupt,format!("conversation {conversation} has unreadable call metadata")));
            }
        }
        let mut result=std::collections::BTreeMap::new();
        let mut query=db.prepare_cached("SELECT json_extract(body,'$.facts.machine') FROM run_calls WHERE conversation=?1 AND json_valid(body) AND json_extract(body,'$.unavailable') IS NULL AND (json_extract(body,'$.facts.state.kind')='running' OR (json_extract(body,'$.facts.state.kind')='exited' AND json_extract(body,'$.facts.state.remaining')>0)) ORDER BY json_extract(body,'$.facts.machine'),call")?;
        for machine in query.query_map([&conversation],|row|row.get::<_,String>(0))? {
            let machine=machine?.parse::<MachineName>().map_err(|e|fastctx_storage::Error::new(e.to_string()))?;
            result.entry(machine).or_insert(None);
        }
        let mut query=db.prepare_cached("SELECT j.machine,s.started FROM run_jobs j LEFT JOIN run_job_sources s ON s.conversation=j.conversation AND s.call=j.call AND s.machine=j.machine AND s.job=j.job WHERE j.conversation=?1 AND json_valid(j.body) AND (json_extract(j.body,'$.state.kind')='running' OR (json_extract(j.body,'$.state.kind')='exited' AND json_extract(j.body,'$.state.remaining')>0)) ORDER BY j.call,j.machine,j.job")?;
        for row in query.query_map([&conversation],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?)))? {
            let (machine,started)=row?;
            let machine=machine.parse::<MachineName>().map_err(|e|fastctx_storage::Error::new(e.to_string()))?;
            let started=started.map(|at|at.parse::<jiff::Timestamp>().map_err(|e|fastctx_storage::Error::new(e.to_string()))).transpose()?;
            result.entry(machine).or_insert(started);
        }
        Ok(result.into_iter().collect())
    }).map_err(io::Error::other)
}
