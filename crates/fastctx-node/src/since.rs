//! Ask each machine now, as `who` does, and merge what its records establish.

mod records;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use fastctx_net::link::CallFailure;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::since::*;
use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;

use crate::Node;

pub async fn here(node: &Node, since: Timestamp, town: Option<TownName>) -> Result<SinceReply, Failure> {
    let (world, machine) = (node.world.clone(), node.machine());
    let membership = node.link().map(|link| link.membership().world.clone());
    // Intake publishes the signed ledger before its readable files. Reading
    // signed records under the same lock also excludes carrying a town in.
    let _syncing = node.syncing.lock().await;
    tokio::task::spawn_blocking(move || records::read(&world, &machine, since, town.as_ref(), membership.as_ref()))
        .await
        .map_err(|error| Failure::new(format!("reading history: {error}")))?
        .map_err(|error| Failure::new(format!("reading history: {error}")))
}

pub async fn query(node: &Arc<Node>, since: Timestamp, town: Option<TownName>) -> Result<WorldSince, Failure> {
    let machine = node.machine();
    let mut issues = Vec::new();
    match &town {
        Some(town) => crate::town::map::query(node, town).await?,
        None => {
            if let Err(error) = crate::ledger::query(node, None).await {
                issues.push(SinceIssue { source: "World ledgers".to_string(), reason: error });
            }
        }
    }
    let asked = match &town {
        Some(town) => {
            let (world, town) = (node.world.clone(), town.clone());
            tokio::task::spawn_blocking(move || records::machines(&world, &town))
                .await
                .map_err(|error| Failure::new(format!("reading the town: {error}")))?
                .map_err(|error| Failure::new(format!("reading the town: {error}")))?
        }
        None => None,
    };
    let asks_for = |name: &MachineName| asked.as_ref().is_none_or(|asked| asked.contains(name));
    let mut answers = Vec::new();
    let mut queried = BTreeSet::new();
    let mut events = Vec::new();
    let mut undated = Vec::new();
    let mut tasks = Vec::new();
    if !asks_for(&machine)
        && let Some(town) = &town
    {
        // The queried ledger is available even when all of its member
        // machines are offline. Their resident records remain unavailable.
        let (world, machine, town) = (node.world.clone(), machine.clone(), town.clone());
        let membership = node.link().map(|link| link.membership().world.clone());
        let _syncing = node.syncing.lock().await;
        let (ledger_events, ledger_issues) =
            tokio::task::spawn_blocking(move || records::ledger(&world, &machine, since, &town, membership.as_ref()))
                .await
                .map_err(|error| Failure::new(format!("reading the queried ledger: {error}")))?;
        events.extend(ledger_events);
        issues.extend(ledger_issues);
    }
    if asks_for(&machine) {
        queried.insert(machine.clone());
        let (node, town) = (node.clone(), town.clone());
        let name = machine.clone();
        tasks.push((
            machine.clone(),
            tokio::spawn(async move {
                here(&node, since, town)
                    .await
                    .map_err(|failure| MachineSince::NoAnswer { machine: name, reason: failure.message })
            }),
        ));
    }
    if let Some(link) = node.link() {
        let refreshed = link.refresh().await;
        let (table, presence) = link.members();
        for member in table.machines().filter(|member| asks_for(&member.name)) {
            let name = member.name.clone();
            let seen = presence.iter().find(|seen| seen.name == name);
            if refreshed.is_ok()
                && let Some(seen) = seen
                && let Some(at) = seen.since.filter(|at| *at > since)
            {
                events.push(SinceEvent {
                    id: records::identity(
                        "presence",
                        &[name.as_str(), &at.to_string(), if seen.online { "on" } else { "off" }],
                    ),
                    at,
                    machine: name.clone(),
                    town: SinceTown::Unrecorded { recorded_in: Vec::new() },
                    sources: Vec::new(),
                    what: if seen.online { SinceKind::MachineConnected } else { SinceKind::MachineOffline },
                });
            }
            if name == machine {
                continue;
            }
            queried.insert(name.clone());
            if let Err(error) = &refreshed {
                answers.push(MachineSince::NoAnswer {
                    machine: name,
                    reason: format!("whether it is connected is not known: the hub could not be asked ({error})"),
                });
                continue;
            }
            match seen {
                Some(seen) if seen.online => {
                    let (link, target, town) = (link.clone(), name.clone(), town.clone());
                    tasks.push((
                        name,
                        tokio::spawn(async move {
                            match link.call(&target, RemoteCall::Since { since, town }).await {
                                Ok(Ok(RemoteReply::Since(reply))) if reply.coverage.machine == target => Ok(reply),
                                Ok(Ok(_)) => Err(MachineSince::NoAnswer {
                                    machine: target,
                                    reason: "it answered something else".to_string(),
                                }),
                                Ok(Err(failure)) => {
                                    Err(MachineSince::NoAnswer { machine: target, reason: failure.message })
                                }
                                Err(CallFailure::Offline { name, last_seen }) => {
                                    Err(MachineSince::Offline { machine: name, since: last_seen })
                                }
                                Err(failure) => {
                                    Err(MachineSince::NoAnswer { machine: target, reason: failure.to_string() })
                                }
                            }
                        }),
                    ));
                }
                Some(seen) => answers.push(MachineSince::Offline { machine: name, since: seen.since }),
                None => answers.push(MachineSince::NoAnswer {
                    machine: name,
                    reason: "the hub did not say whether it is connected".to_string(),
                }),
            }
        }
        if let Err(error) = refreshed {
            issues.push(SinceIssue { source: "hub presence".to_string(), reason: error });
        }
    }
    if let Some(asked) = asked {
        for name in asked.into_iter().filter(|name| !queried.contains(name)) {
            answers.push(MachineSince::NoAnswer {
                machine: name,
                reason: "the town names this machine, but it is not a known member of this World".to_string(),
            });
        }
    }
    for (name, task) in tasks {
        match task.await {
            Ok(Ok(mut reply)) => {
                events.append(&mut reply.events);
                undated.append(&mut reply.undated);
                answers.push(MachineSince::Answered { reply: reply.coverage });
            }
            Ok(Err(answer)) => answers.push(answer),
            Err(error) => answers.push(MachineSince::NoAnswer { machine: name, reason: format!("asking: {error}") }),
        }
    }
    answers.sort_by(|a, b| name(a).cmp(name(b)));
    let events = merge(events, &mut issues);
    Ok(WorldSince { machine, since, town, machines: answers, events, undated, issues, limitations: limitations() })
}

fn name(answer: &MachineSince) -> &MachineName {
    match answer {
        MachineSince::Answered { reply } => &reply.machine,
        MachineSince::Offline { machine, .. } | MachineSince::NoAnswer { machine, .. } => machine,
    }
}

fn merge(events: Vec<SinceEvent>, issues: &mut Vec<SinceIssue>) -> Vec<SinceEvent> {
    let mut unique = BTreeMap::<String, SinceEvent>::new();
    let mut conflicts = Vec::new();
    for mut event in events {
        match unique.get_mut(&event.id) {
            None => {
                unique.insert(event.id.clone(), event);
            }
            Some(prior) if prior.at == event.at && prior.machine == event.machine && prior.what == event.what => {
                for source in event.sources.drain(..) {
                    if !prior.sources.contains(&source) {
                        prior.sources.push(source);
                    }
                }
                records::merge_town(&mut prior.town, &event.town);
            }
            Some(_) => {
                issues.push(SinceIssue {
                    source: event.id.clone(),
                    reason: "copies disagree; both recorded facts are returned".to_string(),
                });
                conflicts.push(event);
            }
        }
    }
    let mut events: Vec<_> = unique.into_values().chain(conflicts).collect();
    events.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
    events
}

fn limitations() -> Vec<String> {
    [
        "Resident births are retained, but repeated arrivals and departures have no complete history. Current sleep and Claude closure markers have no reliable event time and are returned separately.",
        "Returning to a town deletes its departure marker; leaving it again replaces the marker. Only surviving moves can be reported. Copied births and turns do not record their original town; uncertain town matches are marked unrecorded.",
        "Turn timestamps are the node's first settlement, not the host's completion time. Host stop and interruption share an outcome; only a retained reason can distinguish them. Sessions whose turns the World never observes have no turn history.",
        "Job elapsed time includes launch overhead. An interrupted or failed supervisor's final account does not establish when, or whether, its processes stopped.",
        "The hub supplies only the current connection endpoint: connected since, or last observed offline. Earlier intervals are unavailable; after a hub restart an offline time can be unknown.",
        "Each machine reads its existing records and ledger copies at query time. Clock readings are preserved without cross-machine correction; this is not a distributed snapshot. Timestamps retain their source precision, not a promise of nanosecond accuracy.",
    ].into_iter().map(str::to_string).collect()
}
