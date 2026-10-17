//! Ask each machine now, as `who` does, and merge what its records establish.

mod records;

use std::sync::Arc;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::since::*;
use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;

use crate::Node;
use crate::around::Heard;

pub async fn here(node: &Node, since: Timestamp, town: Option<TownName>) -> Result<SinceReply, Failure> {
    let (world, machine) = (node.world.clone(), node.machine());
    // Intake publishes the signed ledger before its readable files. Reading
    // signed records under the same lock also excludes carrying a town in.
    let _syncing = node.syncing.lock().await;
    tokio::task::spawn_blocking(move || records::read(&world, &machine, since, town.as_ref()))
        .await
        .map_err(|error| Failure::new(format!("reading history: {error}")))?
        .map_err(|error| Failure::new(format!("reading history: {error}")))
}

/// What happened since `since`, as each machine whoever asks (`acting`) may
/// look at tells it, in `town` or everywhere.
pub async fn query(
    node: &Arc<Node>,
    acting: &Acting,
    since: Timestamp,
    town: Option<TownName>,
) -> Result<WorldSince, Failure> {
    let machine = node.machine();
    let mut issues = Vec::new();
    match &town {
        Some(town) => crate::town::map::query(node, town).await?,
        None => {
            if let Err(error) = crate::ledger::query(node, None).await {
                issues.push(SinceIssue { source: "the projects' ledgers".to_string(), reason: error });
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
    let mut events = Vec::new();
    let mut undated = Vec::new();
    // This machine answers only to whom may work on it, as for `who`.
    let works_here = node.works_here(acting).is_ok();
    if works_here && asks_for(&machine) {
        // This machine's own records are filtered as the other machines
        // filter theirs: by whom each conversation works for.
        let looker = crate::admission::seen::Looker::Here(acting);
        match here(node, since, town.clone()).await.map(|reply| crate::admission::seen::since(node, looker, reply)) {
            Ok(mut reply) => {
                events.append(&mut reply.events);
                undated.append(&mut reply.undated);
                answers.push(MachineSince::Answered { reply: reply.coverage });
            }
            Err(failure) => answers.push(MachineSince::NoAnswer { machine: machine.clone(), reason: failure.message }),
        }
    } else if let Some(town) = &town {
        // The queried ledger is available even when all of its member
        // machines are offline. Their resident records remain unavailable.
        events.extend(ledger_events(node, since, town, &mut issues).await?);
    }
    let reachable: Vec<_> = crate::around::reachable_each(node, acting, Purpose::Look)
        .into_iter()
        .filter(|machine| asks_for(&machine.name))
        .collect();
    let mut asked_here: Vec<_> = reachable.iter().map(|machine| (machine.link.clone(), machine.name.clone())).collect();
    asked_here.dedup_by(|a, b| a.1 == b.1);
    let heard = crate::around::ask_each(reachable, |_| RemoteCall::Since { since, town: town.clone() }).await;
    // Who connected and left, as the hubs had it when they were just asked.
    for (link, name) in &asked_here {
        if let Some(seen) = link.view().member(name) {
            events.extend(presence(name, seen.online, seen.since, since));
        }
    }
    for (name, heard) in heard {
        // A machine asked as several accounts answers each with what works
        // for it; their events merge below, and one answer stands for it.
        if let Some(at) = answers.iter().position(|answer| answer.machine() == &name) {
            if let Heard::Answered(RemoteReply::Since(mut reply)) = heard
                && reply.coverage.machine == name
            {
                events.append(&mut reply.events);
                undated.append(&mut reply.undated);
                if !matches!(answers[at], MachineSince::Answered { .. }) {
                    answers[at] = MachineSince::Answered { reply: reply.coverage };
                }
            }
            continue;
        }
        answers.push(match heard {
            Heard::Answered(RemoteReply::Since(mut reply)) if reply.coverage.machine == name => {
                events.append(&mut reply.events);
                undated.append(&mut reply.undated);
                MachineSince::Answered { reply: reply.coverage }
            }
            Heard::Answered(_) => {
                MachineSince::NoAnswer { machine: name, reason: "it answered something else".to_string() }
            }
            Heard::Offline(since) => MachineSince::Offline { machine: name, since },
            Heard::NoAnswer(reason) => MachineSince::NoAnswer { machine: name, reason },
        });
    }
    // A machine the town names that is out of the asker's reach goes
    // unnamed (`world-network.md`, 一个居民替谁干活); one within it that did
    // not answer is said so.
    if let Some(asked) = asked {
        let unasked: Vec<_> = asked
            .into_iter()
            .filter(|name| asked_here.iter().any(|(_, asked)| asked == name) || (works_here && *name == machine))
            .filter(|name| !answers.iter().any(|answer| answer.machine() == name))
            .collect();
        for name in unasked {
            answers.push(MachineSince::NoAnswer { machine: name, reason: "it did not answer".to_string() });
        }
    }
    answers.sort_by(|a, b| a.machine().cmp(b.machine()));
    let events = merge_events(events, &mut issues);
    Ok(WorldSince { machine, since, town, machines: answers, events, undated, issues, limitations: limitations() })
}

/// A machine connecting or leaving after `since`, as its hub saw it.
fn presence(name: &MachineName, online: bool, at: Option<Timestamp>, since: Timestamp) -> Option<SinceEvent> {
    let at = at.filter(|at| *at > since)?;
    Some(SinceEvent {
        id: records::identity("presence", &[name.key(), &at.to_string(), if online { "on" } else { "off" }]),
        at,
        machine: name.clone(),
        town: SinceTown::Unrecorded { recorded_in: Vec::new() },
        sources: Vec::new(),
        what: if online { SinceKind::MachineConnected } else { SinceKind::MachineOffline },
    })
}

async fn ledger_events(
    node: &Node,
    since: Timestamp,
    town: &TownName,
    issues: &mut Vec<SinceIssue>,
) -> Result<Vec<SinceEvent>, Failure> {
    let (world, machine, town) = (node.world.clone(), node.machine(), town.clone());
    let _syncing = node.syncing.lock().await;
    let (events, found) = tokio::task::spawn_blocking(move || records::ledger(&world, &machine, since, &town))
        .await
        .map_err(|error| Failure::new(format!("reading the queried ledger: {error}")))?;
    issues.extend(found);
    Ok(events)
}

/// Reuses the GUI's local record reader without starting a node.
pub(crate) fn local_records(
    world: &fastctx_types::layout::WorldDir,
    machine: &MachineName,
    since: Timestamp,
    town: Option<&TownName>,
) -> std::io::Result<SinceReply> {
    records::read(world, machine, since, town)
}

/// What the hubs say of who connected and left supplements the history
/// carried with Who replies.
pub(crate) async fn presence_for_who(node: &Node, who: &mut fastctx_types::town::WorldWho) -> Result<(), Failure> {
    use fastctx_types::town::MachineWho;
    let Some(history) = &mut who.history else { return Ok(()) };
    for machine in &who.machines {
        let name = match machine {
            MachineWho::Answered { reply } => &reply.machine,
            MachineWho::Offline { machine, .. } => machine,
            MachineWho::NoAnswer { .. } => continue,
        };
        if *name == who.machine {
            continue;
        }
        let seen = node.links().iter().find_map(|link| link.view().member(name).map(|seen| (seen.online, seen.since)));
        if let Some((online, at)) = seen {
            history.events.extend(presence(name, online, at, history.since));
        }
    }
    // A queried town's ledger can be present even when none of its machines
    // answered. Include that copy just as the standalone Since does.
    if !who
        .machines
        .iter()
        .any(|machine| matches!(machine, MachineWho::Answered { reply } if reply.machine == who.machine))
        && let Some(town) = &history.town
    {
        let town = town.clone();
        let events = ledger_events(node, history.since, &town, &mut history.issues).await?;
        history.events.extend(events);
    }
    history.events = merge_events(std::mem::take(&mut history.events), &mut history.issues);
    Ok(())
}
