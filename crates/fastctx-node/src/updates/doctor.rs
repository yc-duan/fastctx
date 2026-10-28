//! The hub's node delivers arranged repairs to legacy machines. Its former
//! automatic rescue is now an ordinary durable repair arrangement.

use crate::Node;
use fastctx_net::{ids::HubId, sign::PublicKey};
use fastctx_types::{
    repair::{Outcome, Report, Step},
    update::{Channel, REPAIR_SINCE, ViewCall, release},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

const PATIENCE: i64 = 15 * 60;
const AGAIN: i64 = 60 * 60;
const LOOK: Duration = Duration::from_secs(60);

type Member = (HubId, PublicKey);
#[derive(Clone, Serialize, Deserialize)]
struct Observed {
    hub: HubId,
    key: PublicKey,
    since: jiff::Timestamp,
    helped: Option<jiff::Timestamp>,
}

pub(super) struct Seen {
    loaded: bool,
    observed: Vec<Observed>,
    helping: BTreeSet<Member>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl Default for Seen {
    fn default() -> Self {
        Self {
            loaded: false,
            observed: Vec::new(),
            helping: BTreeSet::new(),
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
}

pub(super) fn start(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(LOOK).await;
            let Some(node) = weak.upgrade() else { return };
            heard(&node);
        }
    });
}

/// New arrangements are noticed on the authenticated versions push, not only
/// on the periodic scan. The per-machine guard also covers those two triggers.
pub(super) fn heard(node: &Arc<Node>) {
    if !fastctx_platform::follows_releases() {
        return;
    }
    let now = jiff::Timestamp::now();
    let file = super::dir(node).join("doctor-seen.json");
    let mut state = node.updates.doctor.lock().expect("legacy repairs");
    if !state.loaded {
        match fastctx_install::renewal::read_json(&file) {
            Ok(saved) => state.observed = saved.unwrap_or_default(),
            Err(error) => node
                .log
                .line(format_args!("reading {}: {error}; legacy repair observation starts again", file.display())),
        }
        state.loaded = true;
    }
    let mut changed = false;
    for link in node.links() {
        let Some(network) = link.current_versions().filter(|n| n.repair_supported) else { continue };
        if !matches!(link.status(), fastctx_net::link::Status::Online { .. }) {
            continue;
        }
        let view = link.view();
        let Some(me) = view.me() else { continue };
        if !network.machines.get(&me.name).is_some_and(|r| r.hub_here) {
            continue;
        }
        for member in &view.directory.members {
            if member.key == me.key || !member.is_machine() {
                continue;
            }
            let Some(owner) = me.logins.iter().find(|account| view.may_update(account, &member.name)) else {
                continue;
            };
            let Some(running) = network.machines.get(&member.name) else { continue };
            let key = (link.hub().clone(), member.key);
            if running.repair_supported {
                changed |= forget(&mut state.observed, &key);
                continue;
            }
            if let Some(due) = network.repairs.get(&member.name) {
                if !member.online
                    || running.away
                    || due.retry_at.is_some_and(|at| at > now)
                    || state.helping.contains(&key)
                {
                    continue;
                }
                state.helping.insert(key.clone());
                let (node, link, machine, owner, due, slots) =
                    (node.clone(), link.clone(), member.name.clone(), owner.clone(), due.clone(), state.slots.clone());
                tokio::spawn(async move {
                    let _slot = slots.acquire_owned().await.expect("repair slots remain open");
                    if let Err(why) = super::legacy_repair::help(&node, &link, &machine, &owner, &due).await {
                        node.log.line(format_args!("delivering repair {} to {machine}: {why}", due.id));
                        match fastctx_platform::current_identity() {
                            Ok(process) => {
                                let now = jiff::Timestamp::now();
                                let report = Report {
                                    id: due.id.clone(),
                                    version: due.version,
                                    by: due.by.to_string(),
                                    started: now,
                                    finished: Some(now),
                                    process,
                                    steps: vec![Step {
                                        layer: "delivery".into(),
                                        outcome: Outcome::Failed,
                                        detail: why.clone(),
                                        actions: Vec::new(),
                                    }],
                                    success: false,
                                    unchanged: false,
                                    error: Some(why),
                                };
                                if let Err(error) = link
                                    .network_updates(
                                        &owner,
                                        ViewCall::RepairResult { machine, result: Box::new(report) },
                                    )
                                    .await
                                {
                                    node.log
                                        .line(format_args!("reporting repair {} delivery failure: {error}", due.id));
                                }
                            }
                            Err(error) => node.log.line(format_args!("identifying the repair delivery: {error}")),
                        }
                    }
                    node.updates.doctor.lock().expect("legacy repairs").helping.remove(&key);
                });
                continue;
            }
            if !member.managed || !network.policy.managed_auto {
                continue;
            }
            let Some(offer) = network.on(Channel::of(&running.kernel)) else { continue };
            if release(&offer.version) < release(REPAIR_SINCE) || release(&offer.version) <= release(&running.kernel) {
                // A machine that caught up waits its full patience again
                // when a later release finds it stuck.
                changed |= forget(&mut state.observed, &key);
                continue;
            }
            let at = match state.observed.iter().position(|o| o.hub == key.0 && o.key == key.1) {
                Some(at) => at,
                None if member.online && !running.away => {
                    state.observed.push(Observed { hub: key.0.clone(), key: key.1, since: now, helped: None });
                    changed = true;
                    state.observed.len() - 1
                }
                None => continue,
            };
            let observed = &state.observed[at];
            if !member.online
                || running.away
                || now.as_second() - observed.since.as_second() < PATIENCE
                || observed.helped.is_some_and(|t| now.as_second() - t.as_second() < AGAIN)
                || state.helping.contains(&key)
            {
                continue;
            }
            state.observed[at].helped = Some(now);
            changed = true;
            state.helping.insert(key.clone());
            let (node, link, owner, machine, version) =
                (node.clone(), link.clone(), owner.clone(), member.name.clone(), offer.version.clone());
            tokio::spawn(async move {
                if let Err(error) = link
                    .network_updates(
                        &owner,
                        ViewCall::Repair { machines: vec![machine.clone()], version: Some(version) },
                    )
                    .await
                {
                    node.log.line(format_args!("arranging automatic repair for {machine}: {error}"));
                }
                node.updates.doctor.lock().expect("legacy repairs").helping.remove(&key);
            });
        }
    }
    if changed && let Err(error) = fastctx_install::renewal::write_json(&file, &state.observed) {
        node.log.line(format_args!("saving {}: {error}", file.display()));
    }
}

/// Drops what was observed of a machine no longer stuck; true when there was.
fn forget(observed: &mut Vec<Observed>, key: &Member) -> bool {
    let before = observed.len();
    observed.retain(|o| !(o.hub == key.0 && o.key == key.1));
    observed.len() != before
}
