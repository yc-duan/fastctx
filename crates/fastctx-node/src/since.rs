//! Ask each machine now, as `who` does, and merge what its records establish.

mod records;

use std::sync::Arc;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::RemoteCall;
use fastctx_types::since::*;
use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;

use crate::Node;

pub async fn here(node: &Node, since: Timestamp, town: Option<TownName>) -> Result<SinceReply, Failure> {
    let (world, machine) = (node.world.clone(), node.machine());
    let failure = |error: &dyn std::fmt::Display| Failure::new(format!("reading history: {error}"));
    // The conversations' records are each published whole, and there may be
    // many: they are read without holding up what syncs the ledgers.
    let (reading, named) = (world.clone(), machine.clone());
    let mut partial = tokio::task::spawn_blocking(move || records::conversations(&reading, &named, since))
        .await
        .map_err(|error| failure(&error))?
        .map_err(|error| failure(&error))?;
    // Intake publishes the signed ledger before its readable files. Reading
    // signed records under the same lock also excludes carrying a town in.
    let _syncing = node.syncing.lock().await;
    tokio::task::spawn_blocking(move || {
        records::ledgers(&mut partial, &world, &machine, town.as_ref())?;
        Ok(records::reply(partial, &machine, since, town.as_ref()))
    })
    .await
    .map_err(|error| failure(&error))?
    .map_err(|error: std::io::Error| failure(&error))
}

/// What happened since `since`, as each machine whoever asks (`acting`) may
/// look at tells it, in `town` or everywhere.
pub async fn query(
    node: &Arc<Node>,
    acting: &Acting,
    works_here: bool,
    since: Timestamp,
    town: Option<TownName>,
) -> Result<WorldSince, Failure> {
    let machine = node.machine();
    let mut issues = Vec::new();
    match &town {
        Some(town) => crate::town::access::ready(node, acting, town).await?,
        None => {
            if let Err(error) = crate::ledger::query(node, None).await {
                issues.push(SinceIssue { source: "the projects' ledgers".to_string(), reason: error });
            }
        }
    }

    let mut answers = Vec::new();
    let mut events = Vec::new();
    let mut undated = Vec::new();
    // This machine answers only to whom works on it, as for `who`.
    if works_here || crate::around::looks_here(node, acting) {
        // This machine's own records are filtered as the other machines
        // filter theirs: by whom each conversation works for.
        let looker = crate::admission::seen::Looker::Here(acting);
        match here(node, since, town.clone()).await.map(|reply| crate::admission::seen::since(node, looker, reply)) {
            Ok(mut reply) => {
                events.append(&mut reply.events);
                undated.append(&mut reply.undated);
                answers.push(MachineSince::Answered { observation: None, reply: reply.coverage });
            }
            Err(failure) => answers.push(MachineSince::NoAnswer {
                observation: None,
                machine: machine.clone(),
                reason: failure.message,
            }),
        }
    } else if let Some(town) = &town {
        // The queried ledger is available even when all of its member
        // machines are offline. Their resident records remain unavailable.
        events.extend(ledger_events(node, since, town, &mut issues).await?);
    }
    let reachable = crate::around::reachable_each(node, acting, Purpose::Look);
    let heard = crate::around::ask_each(reachable, |_| RemoteCall::Since { since, town: town.clone() }).await;
    for group in fastctx_net::around::answers::collect(heard) {
        let source = group.source.clone();
        let (status, read) = group.since();
        if !matches!(status, MachineSince::NoAnswer { .. })
            && let Some(link) = node.link_of(&source.hub)
            && let Some(seen) = link.view().member(&source.written.name)
        {
            events.extend(fastctx_net::around::answers::presence(&source, seen.online, seen.since, since));
        }
        if let Some(mut reply) = read {
            events.append(&mut reply.events);
            fastctx_types::observation::union(&mut undated, reply.undated);
        }
        answers.push(status);
    }
    answers.sort_by(|a, b| a.machine().cmp(b.machine()));
    let events = merge_events(events, &mut issues);
    Ok(WorldSince { machine, since, town, machines: answers, events, undated, issues, limitations: limitations() })
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
        if matches!(machine, MachineWho::NoAnswer { .. }) {
            continue;
        }
        let Some(observation) = machine.observation() else { continue };
        let source = &observation.source;
        if let Some(link) = node.link_of(&source.hub)
            && let Some(seen) = link.view().member(&source.written.name)
        {
            history.events.extend(fastctx_net::around::answers::presence(
                source,
                seen.online,
                seen.since,
                history.since,
            ));
        }
    }
    // A queried town's ledger can be present even when none of its machines
    // answered. Include that copy just as the standalone Since does.
    if !who
        .machines
        .iter()
        .any(|machine| matches!(machine, MachineWho::Answered { reply, .. } if reply.machine == who.machine))
        && let Some(town) = &history.town
    {
        let town = town.clone();
        let events = ledger_events(node, history.since, &town, &mut history.issues).await?;
        history.events.extend(events);
    }
    history.events = merge_events(std::mem::take(&mut history.events), &mut history.issues);
    Ok(())
}
