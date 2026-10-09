//! Agent discovery, requested on demand and fanned out like `who`.

use std::sync::Arc;
use std::time::Instant;

use fastctx_hosts::agents::{elapsed, measured};
use fastctx_install::apply::{self, Places, Standing};
use fastctx_net::link::CallFailure;
use fastctx_types::MachineName;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::agents::*;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use jiff::Timestamp;

use crate::Node;
use crate::around::Heard;

pub async fn here(node: &Arc<Node>) -> Result<AgentsReply, Failure> {
    let start = Instant::now();
    let checked_at = Timestamp::now();
    let local = node.clone();
    let claude = tokio::task::spawn_blocking(move || {
        let env = crate::claude::environment().map_err(|failure| failure.message)?;
        let home = fastctx_platform::user_home().map_err(|error| error.to_string())?;
        let mut claude = fastctx_hosts::claude::agents::probe(&home, &env, std::env::var_os("PATH").as_ref());
        let program = match &claude.program {
            Fact::Known { value: Some(program) } => Some(program.path.clone()),
            _ => None,
        };
        let places = places(&local, &env, &home, program);
        let status = measured(&mut claude.timings, "apply status", || apply::status(&places));
        let (codex_integration, claude_integration) = match status {
            Ok(status) => (Fact::known(integration(status.codex)), Fact::known(integration(status.claude))),
            Err(error) => (Fact::failed(error.to_string()), Fact::failed(error.to_string())),
        };
        claude.integration = claude_integration;
        claude.unattended = match measured(&mut claude.timings, "consent", || apply::claude_consent(&places)) {
            Ok(accepted) => Fact::known(accepted),
            Err(error) => Fact::failed(error.to_string()),
        };
        Ok::<_, String>((claude, codex_integration))
    });
    let (mut codex, claude) = tokio::join!(node.codex.agents(), claude);
    let (claude, integration) =
        claude.map_err(|error| Failure::new(format!("probing agents: {error}")))?.map_err(Failure::new)?;
    codex.integration = integration;
    Ok(AgentsReply { machine: node.machine(), checked_at, elapsed_ms: elapsed(start), agents: vec![codex, claude] })
}

/// Where this user's hosts keep what an apply and the user's consent touch,
/// as the node's own Claude environment finds Claude Code's directory.
pub(crate) fn places(
    node: &Node,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
    home: &std::path::Path,
    claude: Option<std::path::PathBuf>,
) -> Places {
    let claude_home = env
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
        .filter(|(_, value)| !value.is_empty())
        .map(|(_, value)| value.into())
        .unwrap_or_else(|| home.join(".claude"));
    Places {
        root: node.root.clone(),
        world: node.world.path().into(),
        codex_home: node.codex.config_home().into(),
        claude_home,
        claude,
        fastctx: node.fastctx.clone(),
    }
}

fn integration(standing: Standing) -> Integration {
    match standing {
        Standing::Absent => Integration::Absent,
        Standing::Out => Integration::Out,
        Standing::In => Integration::In,
        Standing::Old => Integration::Old,
    }
}

/// The agents on `machine`, or on this machine and each machine whoever
/// asks (`acting`) may use.
pub async fn query(node: &Arc<Node>, acting: &Acting, machine: Option<MachineName>) -> Result<WorldAgents, Failure> {
    let here = node.machine();
    let mut machines = Vec::new();
    if machine.as_ref().is_none_or(|machine| *machine == here) {
        machines.push(match self::here(node).await {
            Ok(reply) => MachineAgents::Answered { reply },
            Err(error) => MachineAgents::NoAnswer { machine: here.clone(), reason: error.message },
        });
    }
    let heard = match machine.filter(|machine| *machine != here) {
        None => crate::around::ask_each(crate::around::reachable(node, acting, Purpose::Look), |_| RemoteCall::Agents).await,
        Some(name) => {
            let heard = match node.call(acting, &(&name).into(), Purpose::Look, RemoteCall::Agents).await {
                Ok(Ok(reply)) => Heard::Answered(reply),
                Ok(Err(failure)) => Heard::NoAnswer(failure.message),
                Err(CallFailure::Offline { last_seen, .. }) => Heard::Offline(last_seen),
                Err(failure) => return Err(Failure::new(failure.to_string())),
            };
            vec![(name, heard)]
        }
    };
    for (name, heard) in heard {
        machines.push(match heard {
            Heard::Answered(RemoteReply::Agents(reply)) if reply.machine == name => MachineAgents::Answered { reply },
            Heard::Answered(_) => MachineAgents::NoAnswer { machine: name, reason: "it answered something else".into() },
            Heard::Offline(since) => MachineAgents::Offline { machine: name, since },
            Heard::NoAnswer(reason) => MachineAgents::NoAnswer { machine: name, reason },
        });
    }
    Ok(WorldAgents { machine: here, machines })
}
