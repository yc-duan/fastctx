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
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Why a worker is let go now.
#[derive(Clone, Copy)]
enum Going {
    /// Its model rested long enough (`retention::idle_delay`).
    Rested,
    /// Memory runs short, and of the idle ones its model acted longest ago,
    /// at `acted` as seen as they were ranked: acted since, it is not the
    /// longest idle any more, and stays this time (codex-4bf06087 #5).
    ForMemory { acted: jiff::Timestamp },
}

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
/// Let go for memory, one found busy a moment ago is asked again all the
/// same, and one found busy now is asked again at the next look: memory
/// short waits for no while (codex-4bf06087 #6).
async fn release_idle(node: &Arc<Node>, about: &About, going: Going) -> Result<bool, Failure> {
    let id = &about.id;
    let timed = matches!(going, Going::Rested);
    let action = crate::control_queue::gate(node, id);
    let Ok(_action) = action.try_lock() else { return Ok(false) };
    let management = node.residents.management(id)?;
    let Ok(_exclusive) = management.try_write() else { return Ok(false) };
    let Some((current, _)) = node.residents.about(id) else { return Ok(false) };
    if current.session != about.session || current.home != about.home || !eligible(node, &current, timed).await? {
        return Ok(false);
    }
    let worker = if timed {
        let Some(token) = node.idle_probes.begin(id) else { return Ok(false) };
        match crate::claude::lifecycle::idle(node, &current).await {
            Ok(None) => return Ok(false),
            result => {
                node.idle_probes.clear(id, &token);
                result?.expect("a positive idle observation")
            }
        }
    } else {
        let Some(worker) = crate::claude::lifecycle::idle(node, &current).await? else { return Ok(false) };
        worker
    };
    let Some(now) = node.residents.now(id) else { return Ok(false) };
    match going {
        // Its own work — a subagent, a monitor — moved since the last turn:
        // the model acted then (2026-11-16).
        Going::Rested if !retention::rested(now.since, worker.updated) => return Ok(false),
        Going::ForMemory { acted } if acted_at(now.since, worker.updated) > acted => return Ok(false),
        _ => {}
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
    release_idle(node, about, Going::Rested).await.map(|_| ())
}

/// When the model last acted: as the World saw its turns, and as the host
/// last wrote the work of its own it runs between them.
fn acted_at(since: jiff::Timestamp, host: Option<jiff::Timestamp>) -> jiff::Timestamp {
    host.map_or(since, |host| host.max(since))
}

/// Who asks for memory.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asking {
    /// The look every few seconds, memory short: one running already is it.
    Pressed,
    /// A host about to be woken or started: it waits a while for one running
    /// already, then frees what is still wanted itself, for no longer than
    /// that while in all — never set going on what another is doing
    /// (codex-4bf06087 #3, #4).
    Waking,
}

/// The longest a wake waits for memory, in all.
const WAKING_WAITS: Duration = Duration::from_secs(20);

/// One relief at a time.
static RELIEVING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

tokio::task_local! {
    /// Set while a relief runs in this task: letting go of a host may wake
    /// one, which asks for relief again, and that one returns at once.
    static RELIEVING_HERE: ();
}

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
    asking: Asking,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = usize> + Send + 'a>> {
    Box::pin(async move {
        if RELIEVING_HERE.try_with(|_| ()).is_ok() {
            return 0;
        }
        let deadline = (asking == Asking::Waking).then(|| Instant::now() + WAKING_WAITS);
        let _relieving = match deadline {
            None => match RELIEVING.try_lock() {
                Ok(held) => held,
                Err(_) => return 0,
            },
            Some(deadline) => match tokio::time::timeout_at(deadline.into(), RELIEVING.lock()).await {
                Ok(held) => held,
                Err(_) => return 0,
            },
        };
        RELIEVING_HERE.scope((), relieve_now(node, target, except, deadline)).await
    })
}

async fn relieve_now(node: &Arc<Node>, target: u64, except: Option<&ResidentId>, deadline: Option<Instant>) -> usize {
    let late = || deadline.is_some_and(|deadline| Instant::now() >= deadline);
    if crate::memory_room::available().is_none_or(|available| available >= target) {
        return 0;
    }
    let mut scan = deadline.map_or_else(Default::default, crate::claude::lifecycle::Scan::until);
    let mut idle = Vec::new();
    for resident in node.residents.everyone() {
        if late() {
            break;
        }
        let about = resident.about;
        if except == Some(&about.id) || !eligible(node, &about, false).await.unwrap_or(false) {
            continue;
        }
        let Ok(Some(worker)) = scan.idle(node, &about).await else { continue };
        let Some(now) = node.residents.now(&about.id) else { continue };
        idle.push((acted_at(now.since, worker.updated), about));
    }
    idle.sort_by_key(|(acted, _)| *acted);
    let mut went = 0;
    for (acted, about) in idle {
        let Some(available) = crate::memory_room::available() else { break };
        if available >= target || late() {
            break;
        }
        match release_idle(node, &about, Going::ForMemory { acted }).await {
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
