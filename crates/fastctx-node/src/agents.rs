//! Agent discovery, requested on demand and fanned out like `who`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use fastctx_hosts::agents::{elapsed, measured};
use fastctx_install::apply::{self, Host, Places, Standing};
use fastctx_net::link::CallFailure;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::agents::*;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AgentName, MachineName};
use jiff::Timestamp;

use crate::Node;
use crate::around::Heard;

/// The agents on this machine, one for each home of each: every account
/// answers for itself, its program, login, models, limits, and whether the
/// World is put into it.
pub async fn here(node: &Arc<Node>) -> Result<AgentsReply, Failure> {
    let start = Instant::now();
    let checked_at = Timestamp::now();
    let env = crate::claude::environment()?;
    let user_home =
        fastctx_platform::user_home().map_err(|error| Failure::new(format!("finding the home directory: {error}")))?;
    let homes = crate::homes::catalog(node, &env, &user_home)
        .map_err(|error| Failure::new(format!("listing the agents' homes: {error}")))?;
    let world = node.world.path().to_path_buf();
    let limits = tokio::task::spawn_blocking(move || fastctx_hosts::claude::exchange::limits(&world));
    let limits = limits.await.map_err(|error| Failure::new(format!("reading Claude's limits: {error}")))?;
    let claude_homes: Vec<_> =
        homes.iter().filter(|(agent, _, _)| agent.as_str() == "claude").map(|(_, home, _)| home.clone()).collect();
    let limits = Arc::new(
        limits
            .into_iter()
            .filter_map(|(transcript, limits)| {
                crate::homes::limits_home(&claude_homes, &transcript).map(|home| (home.dir.clone(), limits))
            })
            .collect::<Vec<_>>(),
    );
    // Each home is asked at once: a slow account does not hold the others.
    let probes: Vec<_> = homes
        .into_iter()
        .filter_map(|(agent, home, visible)| visible.then_some((agent, home)))
        .map(|(agent, home)| {
            let (node, env, user_home, limits) = (node.clone(), env.clone(), user_home.clone(), limits.clone());
            tokio::spawn(async move {
                if home.found != HomeFound::Default && !home.dir.is_dir() {
                    let mut facts = failed(
                        agent,
                        format!("{} is no longer a home directory on {}", home.dir.display(), node.machine()),
                    );
                    facts.home = Some(home);
                    return facts;
                }
                match agent.as_str() {
                    "codex" => codex(&node, &env, &user_home, home).await,
                    _ => tokio::task::spawn_blocking(move || claude(&node, &env, &user_home, home, &limits))
                        .await
                        .unwrap_or_else(|error| failed(agent, format!("probing Claude Code: {error}"))),
                }
            })
        })
        .collect();
    let mut agents = Vec::with_capacity(probes.len());
    for probe in probes {
        agents.push(probe.await.map_err(|error| Failure::new(format!("probing agents: {error}")))?);
    }
    Ok(AgentsReply { machine: node.machine(), checked_at, elapsed_ms: elapsed(start), agents })
}

/// Codex in `home`, through the app-server of that home.
async fn codex(node: &Arc<Node>, env: &[(OsString, OsString)], user_home: &Path, home: AgentHome) -> AgentFacts {
    let dir = (home.found != HomeFound::Default).then(|| home.dir.clone());
    let mut facts = node.codex.home(dir.as_deref()).agents().await;
    let places = places(node, env, user_home, None);
    let at = home.dir.clone();
    facts.integration = match tokio::task::spawn_blocking(move || apply::status_at(&places, Host::Codex, &at)).await {
        Ok(Ok(standing)) => Fact::known(integration(standing)),
        Ok(Err(error)) => Fact::failed(error.to_string()),
        Err(error) => Fact::failed(error.to_string()),
    };
    facts.home = Some(home);
    facts
}

/// Claude Code in `home`: its commands run with the home named, and its
/// limits are what its sessions here last reported.
fn claude(
    node: &Node,
    env: &[(OsString, OsString)],
    user_home: &Path,
    home: AgentHome,
    limits: &[(PathBuf, AgentLimits)],
) -> AgentFacts {
    let mut env = env.to_vec();
    if home.found != HomeFound::Default {
        env.retain(|(name, _)| !name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"));
        env.push(("CLAUDE_CONFIG_DIR".into(), home.dir.as_os_str().to_owned()));
    }
    let mut claude = fastctx_hosts::claude::agents::probe(user_home, &env, std::env::var_os("PATH").as_ref());
    let program = match &claude.program {
        Fact::Known { value: Some(program) } => Some(program.path.clone()),
        _ => None,
    };
    let places = places(node, &env, user_home, program);
    claude.integration =
        match measured(&mut claude.timings, "apply status", || apply::status_at(&places, Host::Claude, &home.dir)) {
            Ok(standing) => Fact::known(integration(standing)),
            Err(error) => Fact::failed(error.to_string()),
        };
    claude.unattended = match measured(&mut claude.timings, "consent", || apply::claude_consent_at(&places, &home.dir))
    {
        Ok(accepted) => Fact::known(accepted),
        Err(error) => Fact::failed(error.to_string()),
    };
    claude.limits = match limits.iter().find(|(owner, _)| crate::homes::same_dir(&home.dir, owner)) {
        Some((_, limits)) => Fact::known(limits.clone()),
        None => Fact::unavailable("no session in this home has reported its limits yet"),
    };
    claude.home = Some(home);
    claude
}

fn failed(agent: AgentName, reason: String) -> AgentFacts {
    AgentFacts {
        agent,
        home: None,
        account: Fact::failed(reason.clone()),
        limits: Fact::failed(reason.clone()),
        program: Fact::failed(reason.clone()),
        version: Fact::failed(reason.clone()),
        launch: Fact::failed(reason.clone()),
        login: Fact::failed(reason.clone()),
        models: Fact::failed(reason.clone()),
        integration: Fact::failed(reason.clone()),
        unattended: Fact::failed(reason),
        timings: Vec::new(),
    }
}

/// Where this user's hosts keep what an apply and the user's consent touch:
/// the default homes, Claude Code's as `env` names it. A call about one
/// other home selects it on these.
pub(crate) fn places(node: &Node, env: &[(OsString, OsString)], home: &Path, claude: Option<PathBuf>) -> Places {
    Places {
        root: node.root.clone(),
        world: node.world.path().into(),
        codex_home: node.codex.default().config_home().into(),
        claude_home: crate::homes::claude_default(env, home),
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
        None => {
            crate::around::ask_each(crate::around::reachable(node, acting, Purpose::Look), |_| RemoteCall::Agents).await
        }
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
            Heard::Answered(_) => {
                MachineAgents::NoAnswer { machine: name, reason: "it answered something else".into() }
            }
            Heard::Offline(since) => MachineAgents::Offline { machine: name, since },
            Heard::NoAnswer(reason) => MachineAgents::NoAnswer { machine: name, reason },
        });
    }
    Ok(WorldAgents { machine: here, machines })
}
