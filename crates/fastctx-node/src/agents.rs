//! Agent discovery, requested on demand and fanned out like `who`.

use std::sync::Arc;
use std::time::Instant;

use fastctx_hosts::agents::{elapsed, measured};
use fastctx_install::apply::{self, Places, Standing};
use fastctx_net::link::CallFailure;
use fastctx_types::MachineName;
use fastctx_types::agents::*;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use jiff::Timestamp;

use crate::Node;

pub async fn here(node: &Arc<Node>) -> Result<AgentsReply, Failure> {
    let start = Instant::now();
    let checked_at = Timestamp::now();
    let local = node.clone();
    let claude = tokio::task::spawn_blocking(move || {
        let env = crate::claude::environment().map_err(|failure| failure.message)?;
        let home = fastctx_platform::user_home().map_err(|error| error.to_string())?;
        let mut claude = fastctx_hosts::claude::agents::probe(&home, &env, std::env::var_os("PATH").as_ref());
        let claude_home = env
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
            .filter(|(_, value)| !value.is_empty())
            .map(|(_, value)| value.into())
            .unwrap_or_else(|| home.join(".claude"));
        let program = match &claude.program {
            Fact::Known { value: Some(program) } => Some(program.path.clone()),
            _ => None,
        };
        let places = Places {
            root: local.root.clone(),
            world: local.world.path().into(),
            codex_home: local.codex.config_home().into(),
            claude_home,
            claude: program,
            fastctx: local.fastctx.clone(),
        };
        let status = measured(&mut claude.timings, "apply status", || apply::status(&places));
        let (codex_integration, claude_integration) = match status {
            Ok(status) => (Fact::known(integration(status.codex)), Fact::known(integration(status.claude))),
            Err(error) => (Fact::failed(error.to_string()), Fact::failed(error.to_string())),
        };
        claude.integration = claude_integration;
        Ok::<_, String>((claude, codex_integration))
    });
    let (mut codex, claude) = tokio::join!(node.codex.agents(), claude);
    let (claude, integration) =
        claude.map_err(|error| Failure::new(format!("probing agents: {error}")))?.map_err(Failure::new)?;
    codex.integration = integration;
    Ok(AgentsReply { machine: node.machine(), checked_at, elapsed_ms: elapsed(start), agents: vec![codex, claude] })
}

fn integration(standing: Standing) -> Integration {
    match standing {
        Standing::Absent => Integration::Absent,
        Standing::Out => Integration::Out,
        Standing::In => Integration::In,
        Standing::Old => Integration::Old,
    }
}

pub async fn query(node: &Arc<Node>, machine: Option<MachineName>) -> Result<WorldAgents, Failure> {
    let here = node.machine();
    let mut machines = Vec::new();
    let mut asks = Vec::new();
    if machine.as_ref().is_none_or(|machine| *machine == here) {
        let node = node.clone();
        asks.push(tokio::spawn(async move {
            match self::here(&node).await {
                Ok(reply) => MachineAgents::Answered { reply },
                Err(error) => MachineAgents::NoAnswer { machine: node.machine(), reason: error.message },
            }
        }));
    }
    if machine.as_ref().is_some_and(|machine| *machine == here) {
        // Asking this machine does not require reaching the hub.
    } else if let Some(link) = node.link() {
        let refreshed = link.refresh().await;
        let (table, presence) = link.members();
        let names: Vec<_> = match &machine {
            Some(name) => vec![name.clone()],
            None => table.machines().filter(|member| member.name != here).map(|member| member.name.clone()).collect(),
        };
        for name in names {
            if let Err(error) = &refreshed {
                machines.push(MachineAgents::NoAnswer {
                    machine: name,
                    reason: format!("whether it is connected is not known: the hub could not be asked ({error})"),
                });
                continue;
            }
            if !table.machines().any(|member| member.name == name) {
                return Err(Failure::new(format!("no machine {name} belongs to this World")));
            }
            match presence.iter().find(|seen| seen.name == name) {
                Some(seen) if seen.online => {
                    let link = link.clone();
                    asks.push(tokio::spawn(async move {
                        match link.call(&name, RemoteCall::Agents).await {
                            Ok(Ok(RemoteReply::Agents(reply))) if reply.machine == name => {
                                MachineAgents::Answered { reply }
                            }
                            Ok(Ok(_)) => {
                                MachineAgents::NoAnswer { machine: name, reason: "it answered something else".into() }
                            }
                            Ok(Err(error)) => MachineAgents::NoAnswer { machine: name, reason: error.message },
                            Err(CallFailure::Offline { name, last_seen }) => {
                                MachineAgents::Offline { machine: name, since: last_seen }
                            }
                            Err(error) => MachineAgents::NoAnswer { machine: name, reason: error.to_string() },
                        }
                    }));
                }
                Some(seen) => machines.push(MachineAgents::Offline { machine: name, since: seen.since }),
                None => machines.push(MachineAgents::NoAnswer {
                    machine: name,
                    reason: "the hub did not say whether it is connected".into(),
                }),
            }
        }
    } else if let Some(machine) = machine {
        return Err(Failure::new(format!("there is no machine {machine}: {here} is in no World with other machines")));
    }
    for ask in asks {
        machines.push(ask.await.map_err(|error| Failure::new(format!("asking agents: {error}")))?);
    }
    Ok(WorldAgents { machine: here, machines })
}
