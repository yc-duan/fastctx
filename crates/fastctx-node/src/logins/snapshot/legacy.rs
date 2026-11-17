//! The older reply is persisted with the projection, including file existence.
use super::part;
use crate::{Node, agents};
use fastctx_hosts::inventory::walk;
use fastctx_types::{
    agents::{AgentProgram, AgentsReply, Fact, ProgramSource},
    logins::MachineLogins,
};
use std::{collections::HashMap, sync::Arc, time::Duration};

pub(super) async fn project(node: &Arc<Node>, value: &MachineLogins) -> AgentsReply {
    let reading = value.clone();
    match part(node, "projecting the older login reply", Duration::from_secs(2), move || {
        Ok(agents::reply(reading, |login| {
            if login.launcher.is_none()
                && let Some(fastctx_types::logins::Drift::Changed { reason, .. }) = &login.drift
                && reason.starts_with("reading the login's program failed:")
            {
                return Fact::failed(reason);
            }
            Fact::known(login.launcher.as_ref().map(|launcher| AgentProgram {
                path: launcher.path.clone(),
                source: if launcher.desktop_app { ProgramSource::DesktopApp } else { ProgramSource::Path },
                exists: fastctx_platform::disk::is_file(&launcher.path),
            }))
        }))
    })
    .await
    {
        Ok(reply) => reply,
        Err(error) => agents::reply(value.clone(), |_| Fact::failed(&error.message)),
    }
}

pub(super) fn overlay(value: &MachineLogins, prior: Option<&AgentsReply>) -> AgentsReply {
    let Some(prior) = prior else {
        return agents::reply(value.clone(), |_| {
            Fact::unavailable("the program's existence has not been projected yet")
        });
    };
    let mut reply = prior.clone();
    reply.machine = value.machine.clone();
    if let Some(scan) = &value.scan {
        reply.checked_at = scan.started_at;
    }
    let mut rows = HashMap::new();
    for agent in crate::logins::AGENTS {
        let side = value.of(agent).expect("a known agent");
        rows.extend(side.listed.iter().chain(&side.candidates).map(|login| ((agent, walk::key(&login.dir)), login)));
    }
    for facts in &mut reply.agents {
        let Some(home) = &facts.home else { continue };
        if let Some(login) = rows.get(&(facts.agent.as_str(), walk::key(&home.dir))) {
            let (identity, account) = agents::identity_facts(login);
            facts.login = identity;
            facts.account = account;
            facts.limits = login.limits.clone();
            facts.drift = login.drift.clone();
            facts.workbench = login.workbench;
        }
    }
    reply
}
