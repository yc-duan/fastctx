//! One Claude scan per machine: shared login evidence, then serial fresh rechecks.
//!
//! A worker goes once its model rested `retention::idle_delay` (2026-11-16);
//! and, when the machine runs short of memory, before that: the idle ones,
//! the one whose model acted longest ago first, until enough is free again
//! (`relieve`, `memory_room`). Claude Code's daemon would otherwise retire
//! idle workers in no order once memory runs short, the ones just used among
//! them.

use super::{ReleaseReason, adapters::Candidate, retention};
use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure, residents::About};
use std::{sync::Arc, time::Duration};

/// Whether `about`'s worker may go: a Claude conversation the World
/// started, its host present and nothing holding it; `timed`, once its model
/// rested long enough too.
async fn eligible(node: &Node, about: &About, timed: bool) -> Result<bool, Failure> {
    Ok(about.world_started
        && about.host.agent.as_str() == "claude"
        && node.residents.removed(&about.id).is_none()
        && node.residents.host_present(&about.id)
        && !retention::held(node, &about.id).await?
        && (!timed || node.residents.now(&about.id).is_some_and(|now| retention::rested(now.since, None))))
}

/// Lets go of `about`'s worker under its gates, everything looked at again
/// there and Claude Code asked afresh whether it is idle: whether it went.
async fn release_idle(node: &Arc<Node>, about: &About, timed: bool) -> Result<bool, Failure> {
    let id = &about.id;
    let action = crate::control_queue::gate(node, id);
    let Ok(_action) = action.try_lock() else { return Ok(false) };
    let management = node.residents.management(id)?;
    let Ok(_exclusive) = management.try_write() else { return Ok(false) };
    let Some((current, _)) = node.residents.about(id) else { return Ok(false) };
    if current.session != about.session || current.home != about.home || !eligible(node, &current, timed).await? {
        return Ok(false);
    }
    let Some(token) = node.idle_probes.begin(id) else { return Ok(false) };
    let worker = match crate::claude::lifecycle::idle(node, &current).await {
        Ok(None) => return Ok(false),
        result => {
            node.idle_probes.clear(id, &token);
            result?.expect("a positive idle observation")
        }
    };
    // Its own work — a subagent, a monitor — moved since the last turn: the
    // model acted then (2026-11-16).
    if timed && !node.residents.now(id).is_some_and(|now| retention::rested(now.since, worker.updated)) {
        return Ok(false);
    }
    if !eligible(node, &current, timed).await? {
        return Ok(false);
    }
    super::release_locked(node, &current, ReleaseReason::Idle, Some(&Candidate::Worker(worker))).await
}

async fn consider(node: &Arc<Node>, about: &About, scan: &mut crate::claude::lifecycle::Scan) -> Result<(), Failure> {
    if !eligible(node, about, true).await? {
        return Ok(());
    }
    let id = &about.id;
    let Some(token) = node.idle_probes.begin(id) else { return Ok(()) };
    // Listing holds no conversation gate and is shared by this login's candidates.
    match scan.idle(node, about).await {
        Ok(None) => return Ok(()),
        result => {
            node.idle_probes.clear(id, &token);
            result?;
        }
    }
    release_idle(node, about, true).await.map(|_| ())
}

/// One relief at a time: one asked for while another runs is that one —
/// it frees memory already — and never waits for it, as letting go of a
/// host may wake one, which asks for relief again.
static RELIEVING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Frees memory until `target` bytes are available: lets go of idle
/// workers, the one whose model acted longest ago first, but `except`'s,
/// looking again after each. A worker whose model works — a turn, a task of
/// its own — is never let go. How many went.
///
/// Boxed: letting go of a host may come back here, through what wakes one.
pub(crate) fn relieve<'a>(
    node: &'a Arc<Node>,
    target: u64,
    except: Option<&'a ResidentId>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = usize> + Send + 'a>> {
    Box::pin(relieve_now(node, target, except))
}

async fn relieve_now(node: &Arc<Node>, target: u64, except: Option<&ResidentId>) -> usize {
    let Ok(_relieving) = RELIEVING.try_lock() else { return 0 };
    if crate::memory_room::available().is_none_or(|available| available >= target) {
        return 0;
    }
    let mut scan = crate::claude::lifecycle::Scan::default();
    let mut idle = Vec::new();
    for resident in node.residents.everyone() {
        let about = resident.about;
        if except == Some(&about.id) || !eligible(node, &about, false).await.unwrap_or(false) {
            continue;
        }
        let Some(token) = node.idle_probes.begin(&about.id) else { continue };
        let worker = match scan.idle(node, &about).await {
            Ok(None) => continue,
            result => {
                node.idle_probes.clear(&about.id, &token);
                match result {
                    Ok(worker) => worker.expect("a positive idle observation"),
                    Err(_) => continue,
                }
            }
        };
        let Some(now) = node.residents.now(&about.id) else { continue };
        let acted = worker.updated.map_or(now.since, |updated| updated.max(now.since));
        idle.push((acted, about));
    }
    idle.sort_by_key(|(acted, _)| *acted);
    let mut went = 0;
    for (acted, about) in idle {
        let Some(available) = crate::memory_room::available() else { break };
        if available >= target {
            break;
        }
        match release_idle(node, &about, false).await {
            Ok(true) => {
                went += 1;
                node.log.line(format_args!(
                    "lifecycle {}: let go for memory, {} available, its model last acted at {acted}",
                    about.id,
                    crate::memory_room::size(available)
                ));
            }
            Ok(false) => {}
            Err(error) => node.log.line(format_args!("lifecycle {}: letting go for memory: {error}", about.id)),
        }
    }
    went
}

pub(super) fn start(node: &Arc<Node>) {
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        node.ready().await;
        let mut timer = tokio::time::interval(Duration::from_secs(60));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let mut scan = crate::claude::lifecycle::Scan::default();
            for resident in node.residents.everyone() {
                if let Err(error) = consider(&node, &resident.about, &mut scan).await {
                    node.log.line(format_args!("lifecycle {}: retaining host; {error}", resident.id));
                }
            }
        }
    });
}
