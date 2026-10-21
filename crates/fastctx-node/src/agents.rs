//! The agents of each machine in the shape interfaces and nodes before the
//! logins record read (`AgentsReply`): built from what the node keeps of its
//! logins (`logins`), with no program started for the asking but to ask
//! again a login whose sign-in changed.

use fastctx_types::access::Written;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_install::apply::Places;
use fastctx_types::AgentName;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::agents::*;
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{Found, Login, MachineLogins};
use fastctx_types::remote::RemoteCall;

use crate::Node;

/// The agents on this machine, one for each login of each, listed and
/// candidates, as the node keeps them.
pub async fn here(node: &Arc<Node>) -> Result<AgentsReply, Failure> {
    let logins = crate::logins::here(node).await?;
    Ok(reply(logins))
}

/// The record in the shape of an older reply: a login a scan has not asked
/// is said to be unknown, never taken for logged out.
pub(crate) fn reply(logins: MachineLogins) -> AgentsReply {
    let checked_at = logins.scan.as_ref().map(|scan| scan.started_at).unwrap_or_else(jiff::Timestamp::now);
    let mut agents = Vec::new();
    for agent in crate::logins::AGENTS {
        let side = logins.of(agent).expect("an agent that keeps logins");
        for login in side.listed.iter().chain(&side.candidates) {
            agents.push(facts(agent, login));
        }
    }
    AgentsReply { machine: logins.machine, checked_at, elapsed_ms: 0, agents }
}

fn facts(agent: &str, login: &Login) -> AgentFacts {
    let unasked = || "not asked yet: scan this machine on the agents page".to_string();
    let who = login.identity.as_ref().map(|identified| identified.who.clone());
    let (login_status, account) = match &who {
        Some(Fact::Known { value }) if value.logged_in => (
            Fact::known(LoginStatus::LoggedIn { method: value.method.clone() }),
            Fact::known(AgentAccount {
                email: value.email.clone(),
                organization: value.organization.clone(),
                plan: value.plan.clone(),
            }),
        ),
        Some(Fact::Known { .. }) => {
            (Fact::known(LoginStatus::LoggedOut), Fact::unavailable("no one is logged in to this login"))
        }
        Some(Fact::Unavailable { reason }) => (Fact::unavailable(reason.clone()), Fact::unavailable(reason.clone())),
        Some(Fact::Failed { reason }) => (Fact::failed(reason.clone()), Fact::failed(reason.clone())),
        None => (Fact::unavailable(unasked()), Fact::unavailable(unasked())),
    };
    // The program the login starts now, which no asking is needed for.
    let program = Fact::known(login.launcher.as_ref().map(|launcher| AgentProgram {
        path: launcher.path.clone(),
        source: if launcher.desktop_app { ProgramSource::DesktopApp } else { ProgramSource::Path },
        exists: launcher.path.is_file(),
    }));
    let version = match login.launcher.as_ref().and_then(|launcher| launcher.version.clone()) {
        Some(version) => Fact::known(version),
        None => Fact::unavailable("its version is not known"),
    };
    AgentFacts {
        agent: AgentName::try_from(agent).expect("an agent name"),
        home: Some(AgentHome {
            dir: login.dir.clone(),
            found: match login.found {
                Found::Default => HomeFound::Default,
                Found::Home | Found::Elsewhere => HomeFound::Found,
                Found::Added => HomeFound::Added,
            },
        }),
        account,
        limits: login.limits.clone(),
        program,
        version,
        launch: Fact::unavailable("not checked: nothing is started to look at an agent"),
        login: login_status,
        models: Fact::known(login.models.clone()),
        integration: Fact::known(login.integration),
        unattended: match (agent, login.unattended) {
            ("codex", _) => Fact::known(true),
            (_, Some(unattended)) => Fact::known(unattended),
            (_, None) => Fact::unavailable("granted when the World's tools are put in"),
        },
        timings: Vec::new(),
        drift: login.drift.clone(),
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

/// The agents on `machine`, or on this machine and each machine whoever
/// asks (`acting`) may use.
pub async fn query(node: &Arc<Node>, acting: &Acting, machine: Option<Written>) -> Result<WorldAgents, Failure> {
    let here = node.machine();
    let mut machines = Vec::new();
    if machine.as_ref().is_none_or(|machine| node.is_here(machine)) {
        // One that may not work here sees the logins that assist an account
        // it works for alone (`admission::local::agents`).
        let works_here = node.works_here(acting).is_ok();
        machines.push(match self::here(node).await {
            Ok(reply) if works_here => MachineAgents::Answered { observation: None, reply },
            Ok(reply) => MachineAgents::Answered {
                observation: None,
                reply: crate::admission::local::agents(node, acting, reply),
            },
            Err(error) => MachineAgents::NoAnswer { observation: None, machine: here.clone(), reason: error.message },
        });
    }
    let reachable = crate::around::reachable_each(node, acting, Purpose::Look);
    let reachable = match machine.clone().filter(|machine| !node.is_here(machine)) {
        Some(name) => {
            let links = node.locate(&name)?;
            let selected: Vec<_> = reachable
                .into_iter()
                .filter(|target| {
                    links.iter().any(|link| link.membership().id == target.link.membership().id)
                        && target.link.view().same(&target.name, &name.name)
                })
                .collect();
            if selected.is_empty() {
                return Err(Failure::new(format!("{name} is not a machine this caller may ask")));
            }
            selected
        }
        None if machine.is_none() => reachable,
        None => Vec::new(),
    };
    let heard = crate::around::ask_each(reachable, |_| RemoteCall::Agents).await;
    machines.extend(fastctx_net::around::answers::collect(heard).into_iter().map(|answers| answers.agents()));
    Ok(WorldAgents { machine: here, machines })
}
