//! One Claude scan per machine: shared login evidence, then serial fresh rechecks.

use super::{ReleaseReason, adapters::Candidate, retention};
use crate::Node;
use fastctx_types::{ipc::Failure, residents::About};
use std::{sync::Arc, time::Duration};

fn eligible(node: &Node, about: &About) -> Result<bool, Failure> {
    Ok(about.world_started
        && about.host.agent.as_str() == "claude"
        && node.residents.removed(&about.id).is_none()
        && node.residents.host_present(&about.id)
        && !retention::held(node, &about.id)?
        && node.residents.now(&about.id).is_some_and(|now| {
            jiff::Timestamp::now().as_second().saturating_sub(now.since.as_second())
                >= retention::idle_delay().as_secs() as i64
        }))
}

async fn consider(node: &Arc<Node>, about: &About, scan: &mut crate::claude::lifecycle::Scan) -> Result<(), Failure> {
    if !eligible(node, about)? {
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
    let action = crate::control_queue::gate(node, id);
    let Ok(_action) = action.try_lock() else { return Ok(()) };
    let management = node.residents.management(id)?;
    let Ok(_exclusive) = management.try_write() else { return Ok(()) };
    let Some((current, _)) = node.residents.about(id) else { return Ok(()) };
    if current.session != about.session || current.home != about.home || !eligible(node, &current)? {
        return Ok(());
    }
    let Some(token) = node.idle_probes.begin(id) else { return Ok(()) };
    let worker = match crate::claude::lifecycle::idle(node, &current).await {
        Ok(None) => return Ok(()),
        result => {
            node.idle_probes.clear(id, &token);
            result?.expect("a positive idle observation")
        }
    };
    if eligible(node, &current)? {
        super::release_locked(node, &current, ReleaseReason::Idle, Some(&Candidate::Worker(worker))).await?;
    }
    Ok(())
}

pub(super) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
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
