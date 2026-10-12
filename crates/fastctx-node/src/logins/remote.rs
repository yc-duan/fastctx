//! The logins of the user's other machines: their records read, and their
//! scans, takings-up and changes asked of their own nodes. Only a machine's
//! owner scans it or changes its logins (`world-agent-logins.md`: the list is
//! the machine owner's to change); another account that may see the machine
//! sees its listed logins, and one lent some of them sees those alone.

use std::path::PathBuf;
use std::sync::Arc;

use fastctx_net::link::CallFailure;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{ChangePreview, FolderChange, LoginChange, MachineLogins, MachineLoginsHeard, WorldLogins};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AgentName, MachineName};

use crate::Node;
use crate::admission::Asker;
use crate::around::Heard;

/// The record of `machine`, or of this one and every machine `acting` may
/// look at.
pub(crate) async fn query(
    node: &Arc<Node>,
    acting: &Acting,
    machine: Option<MachineName>,
) -> Result<WorldLogins, Failure> {
    let here = node.machine();
    let mut machines = Vec::new();
    if machine.as_ref().is_none_or(|machine| *machine == here) {
        machines.push(match super::here(node).await {
            Ok(logins) => MachineLoginsHeard::Answered { logins: Box::new(logins) },
            Err(error) => MachineLoginsHeard::NoAnswer { machine: here.clone(), reason: error.message },
        });
    }
    let heard = match machine.filter(|machine| *machine != here) {
        None => {
            crate::around::ask_each(crate::around::reachable(node, acting, Purpose::Look), |_| RemoteCall::AgentLogins)
                .await
        }
        Some(name) => {
            let heard = match node.call(acting, &(&name).into(), Purpose::Look, RemoteCall::AgentLogins).await {
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
            Heard::Answered(RemoteReply::AgentLogins(logins)) if logins.machine == name => {
                MachineLoginsHeard::Answered { logins }
            }
            Heard::Answered(_) => MachineLoginsHeard::NoAnswer {
                machine: name,
                reason: "it answered something else; the World there may be older".into(),
            },
            Heard::Offline(since) => MachineLoginsHeard::Offline { machine: name, since },
            Heard::NoAnswer(reason) => MachineLoginsHeard::NoAnswer { machine: name, reason },
        });
    }
    Ok(WorldLogins { machine: here, machines })
}

/// What this machine's record shows another account: everything to one
/// that may work here; to one lent logins, those listed logins alone.
pub(crate) fn seen(view: &fastctx_net::directory::View, asker: &Asker, mut logins: MachineLogins) -> MachineLogins {
    if asker.hands {
        return logins;
    }
    let here = view.me.clone();
    logins.scan = None;
    logins.folders.clear();
    for agent in super::AGENTS {
        let side = logins.of_mut(agent).expect("an agent that keeps logins");
        side.programs.clear();
        side.candidates.clear();
        side.elsewhere.clear();
        side.listed.retain(|login| {
            view.lent(&asker.account, &here).any(|lend| {
                lend.agent.as_str() == agent
                    && match &lend.home {
                        None => login.default,
                        Some(dir) => crate::homes::same_dir(dir, &login.dir),
                    }
            })
        });
        for login in &mut side.listed {
            login.programs.clear();
            login.change = None;
        }
    }
    logins
}

/// Whether `agent`'s login `dir` here is one `asker` may see.
pub(crate) fn may_see(
    view: &fastctx_net::directory::View,
    asker: &Asker,
    agent: &AgentName,
    dir: &std::path::Path,
    default: bool,
) -> bool {
    asker.hands
        || view.lent(&asker.account, &view.me).any(|lend| {
            lend.agent == *agent
                && match &lend.home {
                    None => default,
                    Some(lent) => crate::homes::same_dir(lent, dir),
                }
        })
}

/// Asks `machine`'s node, of the user's own, for `call`, or this node when
/// it is this machine.
async fn ask(
    node: &Arc<Node>,
    machine: &MachineName,
    purpose: Purpose,
    call: RemoteCall,
) -> Result<RemoteReply, Failure> {
    node.call(&Acting::Here, &machine.into(), purpose, call).await.map_err(|error| Failure::new(error.to_string()))?
}

fn other(machine: &MachineName) -> Failure {
    Failure::new(format!("{machine} answered something other than its agents' logins; the World there may be older"))
}

pub(crate) async fn scan(
    node: &Arc<Node>,
    machine: MachineName,
    stop: bool,
    limits: bool,
) -> Result<MachineLogins, Failure> {
    if machine == node.machine() {
        return super::scan::scan(node, stop, limits).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::ScanAgents { stop, limits }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn add(
    node: &Arc<Node>,
    machine: MachineName,
    agent: AgentName,
    dir: PathBuf,
    limits: bool,
) -> Result<MachineLogins, Failure> {
    if machine == node.machine() {
        return super::take_up(node, agent, dir, limits).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::AddAgentLogin { agent, dir, limits }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn folder(
    node: &Arc<Node>,
    machine: MachineName,
    dir: PathBuf,
    change: FolderChange,
) -> Result<MachineLogins, Failure> {
    if machine == node.machine() {
        return super::folders::change(node, dir, change).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::AgentFolder { dir, change }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn preview(
    node: &Arc<Node>,
    machine: MachineName,
    agent: AgentName,
    dir: PathBuf,
    into: bool,
) -> Result<ChangePreview, Failure> {
    if machine == node.machine() {
        return super::change::preview(node, agent, dir, into).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::PreviewAgentChange { agent, dir, into }).await? {
        RemoteReply::AgentChangePreview(preview) => Ok(*preview),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn change(
    node: &Arc<Node>,
    machine: MachineName,
    agent: AgentName,
    dir: PathBuf,
    into: bool,
    raise_output_limit: bool,
) -> Result<LoginChange, Failure> {
    if machine == node.machine() {
        return super::change::change(node, agent, dir, into, raise_output_limit).await;
    }
    let call = RemoteCall::ChangeAgentLogin { agent, dir, into, raise_output_limit };
    match ask(node, &machine, Purpose::Hands, call).await? {
        RemoteReply::AgentLoginChanged(change) => Ok(*change),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn read_limits(
    node: &Arc<Node>,
    machine: MachineName,
    agent: AgentName,
    dir: PathBuf,
) -> Result<MachineLogins, Failure> {
    if machine == node.machine() {
        super::limits::read(node, &agent, &dir).await?;
        return super::here(node).await;
    }
    match ask(node, &machine, Purpose::Look, RemoteCall::ReadAgentLimits { agent, dir }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

/// Answers another machine's call about this machine's logins, for
/// `asker`, whom the door let in.
pub(crate) async fn answer(
    node: &Arc<Node>,
    view: &fastctx_net::directory::View,
    asker: &Asker,
    call: RemoteCall,
) -> Result<RemoteReply, Failure> {
    let logins = |logins: MachineLogins| RemoteReply::AgentLogins(Box::new(seen(view, asker, logins)));
    match call {
        RemoteCall::AgentLogins => super::here(node).await.map(logins),
        RemoteCall::ScanAgents { stop, limits } => super::scan::scan(node, stop, limits).await.map(logins),
        RemoteCall::AddAgentLogin { agent, dir, limits } => super::take_up(node, agent, dir, limits).await.map(logins),
        RemoteCall::AgentFolder { dir, change } => super::folders::change(node, dir, change).await.map(logins),
        RemoteCall::PreviewAgentChange { agent, dir, into } => super::change::preview(node, agent, dir, into)
            .await
            .map(|preview| RemoteReply::AgentChangePreview(Box::new(preview))),
        RemoteCall::ChangeAgentLogin { agent, dir, into, raise_output_limit } => {
            super::change::change(node, agent, dir, into, raise_output_limit)
                .await
                .map(|change| RemoteReply::AgentLoginChanged(Box::new(change)))
        }
        RemoteCall::ReadAgentLimits { agent, dir } => {
            let record = super::here(node).await?;
            let side = record.of(agent.as_str()).ok_or_else(|| Failure::not_run(format!("{agent} keeps no logins")))?;
            let login = side
                .listed
                .iter()
                .chain(&side.candidates)
                .find(|login| crate::homes::same_dir(&login.dir, &dir))
                // One lent a login sees it only while it is listed.
                .filter(|login| {
                    may_see(view, asker, &agent, &login.dir, login.default)
                        && (asker.hands || login.integration == fastctx_types::agents::Integration::In)
                })
                .ok_or_else(|| {
                    Failure::not_run(format!("no {agent} login {} here is the account's to see", dir.display()))
                })?;
            let dir = login.dir.clone();
            super::limits::read(node, &agent, &dir).await?;
            super::here(node).await.map(logins)
        }
        _ => Err(Failure::new("not a call about the agents' logins")),
    }
}
