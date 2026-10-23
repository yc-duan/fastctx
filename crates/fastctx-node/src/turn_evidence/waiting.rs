//! Durable things that will wake a conversation, with liveness checked at their executor.

use crate::Node;
use fastctx_types::access::Purpose;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::Action;
use fastctx_types::timers::Owner;
use fastctx_types::turn_evidence::WaitOutput;
use fastctx_types::turn_evidence::{Liveness, WaitHandle, WaitKind};
use fastctx_types::{Address, MachineName, ResidentId};
use jiff::Timestamp;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

/// The sentinels a conversation left (`sentinels::goal_seams`); an observation failure is never an empty list.
pub(crate) trait SentinelWaits: Send + Sync {
    fn waiting<'a>(
        &'a self,
        owner: &'a ResidentId,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Vec<WaitHandle>, Failure>> + Send + 'a>>;
}

/// Reads only this owner's completion debts, even when the owner lives elsewhere.
pub(crate) fn jobs_here(
    node: &Node,
    owner: &Address,
    asker: Option<&crate::admission::Asker>,
) -> io::Result<Vec<WaitHandle>> {
    let names = node.names();
    let _disk = node.disk.lock();
    let mut result = Vec::new();
    for directory in node.world.job_directories(&names, owner)? {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let owed = entry?.path();
            if owed.extension().is_none_or(|extension| extension != "owed") {
                continue;
            }
            let record = fastctx_hands::run::record::read(&owed.with_extension("json"))?;
            if !record.owner.resident().is_some_and(|recorded| super::identity::same(&names, recorded, owner)) {
                continue;
            }
            if let Some(asker) = asker {
                let view = fastctx_types::town::RunningJob {
                    source: None,
                    job: record.id.clone(),
                    town: record.town.clone(),
                    machine: record.machine.clone(),
                    owner: record.owner.clone(),
                    started_at: record.started_at,
                    pid: record.pid,
                    command: crate::jobs::first_line(&record.script),
                    output: record.output.clone(),
                    stderr: record.stderr.clone(),
                    account: record.on.as_ref().and_then(|on| on.account.clone()).or_else(|| record.account.clone()),
                };
                if !crate::admission::seen::job(node, asker, &view) {
                    continue;
                }
            }
            let liveness = if record.end.is_some() {
                Liveness::Ended
            } else {
                match &record.supervisor {
                    Some(process) => {
                        if fastctx_platform::is_alive(process)? {
                            Liveness::Alive
                        } else {
                            Liveness::Ended
                        }
                    }
                    None => Liveness::Unknown { reason: "the supervisor has not been identified".into() },
                }
            };
            let bytes = std::fs::metadata(&record.output)?.len();
            result.push(WaitHandle {
                id: record.id.to_string(),
                completion: Some(crate::inbox::job_notice_id(&record)),
                kind: WaitKind::Job,
                machine: record.machine,
                since: Some(record.started_at),
                liveness,
                will_wake: true,
                summary: crate::jobs::first_line(&record.script),
                output: Some(WaitOutput { path: record.output, bytes }),
                output_growth: None,
            });
        }
    }
    Ok(result)
}

pub(crate) async fn collect(
    node: &Arc<Node>,
    resident: &ResidentId,
    actions: &[Action],
) -> Result<Vec<WaitHandle>, Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown conversation"))?;
    let owner = Address { machine: node.machine(), resident: resident.clone() };
    let mut result = jobs_here(node, &owner, None).map_err(failure)?;
    let mut colleagues = BTreeMap::<Address, Timestamp>::new();
    let mut machines = BTreeMap::<MachineName, Option<Timestamp>>::new();
    for action in actions {
        if action.hand == "ask_colleague"
            && let Ok(id) = action.result.parse()
        {
            colleagues.entry(Address { machine: action.machine.clone(), resident: id }).or_insert(action.at);
        }
    }
    let unfinished = |state: &fastctx_types::run::RunState| {
        matches!(
            state,
            fastctx_types::run::RunState::Running | fastctx_types::run::RunState::Exited { remaining: 1.., .. }
        )
    };
    for call in crate::run_facts::evidence_all(&node.world.conversation(resident)).map_err(failure)? {
        if !node.is_me(&call.facts.machine) && unfinished(&call.facts.state) {
            machines.entry(call.facts.machine).or_insert(None);
        }
    }
    for update in crate::run_facts::job_updates(&node.world.conversation(resident)).map_err(failure)? {
        if !node.is_me(&update.machine) && update.state.as_ref().is_some_and(unfinished) {
            machines.entry(update.machine).or_insert(update.started_at);
        }
    }
    // A single-on run may not have returned its action yet. Its durable remote receipt also identifies the executor.
    match std::fs::read_dir(node.world.conversation(resident).elsewhere()) {
        Ok(entries) => {
            for entry in entries {
                let path = entry.map_err(failure)?.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let kept: crate::here::Elsewhere = serde_json::from_slice(&std::fs::read(path).map_err(failure)?)
                    .map_err(|error| failure(io::Error::other(error)))?;
                machines.entry(kept.machine.name).or_insert(Some(kept.started_at));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(failure(error)),
    }
    let received =
        Arc::new(if colleagues.is_empty() { Vec::new() } else { crate::goals::waiting::received(node, resident)? });
    let mut queries = tokio::task::JoinSet::new();
    for (colleague, since) in colleagues {
        let (node, acting, owner, received) = (node.clone(), about.acting.clone(), owner.clone(), received.clone());
        queries.spawn(async move {
            match super::debts::read(&node, &acting, &colleague, &owner).await {
                Ok(debt) => {
                    let mut handles = Vec::new();
                    if let Some(goal) = &debt.goal
                        && let Some(handle) = crate::goals::waiting::handle(&node, &colleague, goal, &received)
                    {
                        handles.push(handle);
                    }
                    if debt.since().is_some() {
                        handles.push(WaitHandle {
                            id: colleague.resident.to_string(),
                            completion: None,
                            kind: WaitKind::Colleague,
                            machine: colleague.machine,
                            since: debt.since().or(Some(since)),
                            liveness: if debt.uncertain.is_empty() {
                                Liveness::Alive
                            } else {
                                Liveness::Unknown { reason: "host acceptance is uncertain".into() }
                            },
                            will_wake: !debt.pending.is_empty() || !debt.running.is_empty() || !debt.answers.is_empty(),
                            summary: format!(
                                "{} pending inputs, {} running turns, {} undelivered answers, {} uncertain inputs",
                                debt.pending.len(),
                                debt.running.len(),
                                debt.answers.len(),
                                debt.uncertain.len()
                            ),
                            output: None,
                            output_growth: None,
                        });
                    }
                    handles
                }
                Err(error) => vec![unknown(
                    colleague.resident.to_string(),
                    WaitKind::Colleague,
                    colleague.machine,
                    Some(since),
                    error.message,
                )],
            }
        });
    }
    for (machine, since) in machines {
        let (node, acting, owner) = (node.clone(), about.acting.clone(), owner.clone());
        queries.spawn(async move {
            let read = async {
                let target = (&machine).into();
                let facts = node
                    .call(&acting, &target, Purpose::Look, RemoteCall::Facts)
                    .await
                    .map_err(|error| Failure::new(error.to_string()))??;
                if !matches!(facts, RemoteReply::Facts(facts) if facts.turn_debts) {
                    return Err(Failure::new(format!("{machine} is too old to report verified background work")));
                }
                match node
                    .call(&acting, &target, Purpose::Look, RemoteCall::WaitingJobs { owner })
                    .await
                    .map_err(|error| Failure::new(error.to_string()))??
                {
                    RemoteReply::WaitingJobs { jobs } => Ok(jobs),
                    _ => Err(Failure::new("expected verified background work")),
                }
            }
            .await;
            read.unwrap_or_else(|error| {
                vec![unknown(format!("jobs@{machine}"), WaitKind::Job, machine, since, error.message)]
            })
        });
    }
    while let Some(query) = queries.join_next().await {
        result.extend(query.map_err(|error| Failure::new(error.to_string()))?);
    }
    for timer in crate::timers::table(node)?.list(Some(&Owner::Conversation { resident: resident.clone() })) {
        result.push(WaitHandle {
            id: timer.id,
            completion: Some(timer.delivery),
            kind: WaitKind::Timer,
            machine: node.machine(),
            since: timer.created_at,
            liveness: Liveness::Alive,
            will_wake: true,
            summary: format!("due at {}", timer.due),
            output: None,
            output_growth: None,
        });
    }
    result.extend(node.sentinel_waits.waiting(resident).await?);
    result.sort_by(|a, b| a.since.cmp(&b.since).then(a.id.cmp(&b.id)));
    Ok(result)
}

fn unknown(id: String, kind: WaitKind, machine: MachineName, since: Option<Timestamp>, reason: String) -> WaitHandle {
    WaitHandle {
        id,
        kind,
        machine,
        since,
        liveness: Liveness::Unknown { reason: reason.clone() },
        will_wake: false,
        completion: None,
        summary: reason,
        output: None,
        output_growth: None,
    }
}

pub(super) fn measure_growth(node: &Node, resident: &ResidentId, handles: &mut [WaitHandle]) -> io::Result<()> {
    let path = node.world.conversation(resident).books().join("waiting-output.json");
    let _transaction = crate::resident_storage::lock();
    let previous: BTreeMap<String, u64> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error),
    };
    let mut next = BTreeMap::new();
    for handle in handles {
        if let Some(output) = &handle.output {
            let key = format!("{}:{}", handle.machine, handle.id);
            handle.output_growth = previous.get(&key).map(|bytes| output.bytes.saturating_sub(*bytes));
            next.insert(key, output.bytes);
        }
    }
    fastctx_platform::ensure_private_dir(path.parent().unwrap_or(Path::new(".")))?;
    fastctx_platform::publish(&path, &serde_json::to_vec(&next)?, fastctx_platform::Permissions::Private)
}

fn failure(error: io::Error) -> Failure {
    Failure::new(format!("reading background work: {error}"))
}
