//! The logins of the user's other machines: their records read, and their
//! scans, takings-up and changes asked of their own nodes. A machine's
//! holders scan it or change its logins (`world-network.md`, 共享机器与智能体协助);
//! another account that may see the machine
//! sees its listed logins, and one some of them assist sees those alone.

use fastctx_types::access::Written;
use std::path::PathBuf;
use std::sync::Arc;

use fastctx_types::AgentName;
use fastctx_types::access::{Acting, HubAccount, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{ChangePreview, FolderChange, LoginChange, MachineLogins, MachineLoginsHeard, WorldLogins};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::usage::UsageReport;

use crate::Node;
use crate::admission::Asker;

/// The record of `machine`, or of this one and every machine `acting` may
/// look at.
pub(crate) async fn query(node: &Arc<Node>, acting: &Acting, machine: Option<Written>) -> Result<WorldLogins, Failure> {
    let here = node.machine();
    let mut machines = Vec::new();
    if machine.as_ref().is_none_or(|machine| node.is_here(machine)) {
        // One that may not work here sees what an account it works for would
        // from another machine (`admission::local::logins`).
        let works_here = node.works_here(acting).is_ok();
        machines.push(match super::here(node).await {
            Ok(logins) if works_here => MachineLoginsHeard::Answered { observation: None, logins: Box::new(logins) },
            Ok(logins) => MachineLoginsHeard::Answered {
                observation: None,
                logins: Box::new(crate::admission::local::logins(node, acting, logins)),
            },
            Err(error) => {
                MachineLoginsHeard::NoAnswer { observation: None, machine: here.clone(), reason: error.message }
            }
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
    let heard = crate::around::ask_each(reachable, |_| RemoteCall::AgentLogins).await;
    machines.extend(fastctx_net::around::answers::collect(heard).into_iter().map(|answers| answers.logins()));
    Ok(WorldLogins { machine: here, machines })
}

/// Why an account a login only assists is not told the login's windows.
pub(crate) const OWNERS_WINDOWS: &str = "the windows of the login's account are its owner's to see";

/// What this machine's record shows another account: everything to one
/// that may work here; to one logins assist, those listed logins alone,
/// without their account's windows, which are their owner's to see.
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
            view.assisting(&asker.account, &here).any(|assist| {
                assist.agent.as_str() == agent
                    && match &assist.home {
                        None => login.default,
                        Some(dir) => crate::homes::same_dir(dir, &login.dir),
                    }
            })
        });
        for login in &mut side.listed {
            login.programs.clear();
            login.change = None;
            login.limits = fastctx_types::agents::Fact::unavailable(OWNERS_WINDOWS);
        }
    }
    logins
}

/// What this machine's record shows a caller that works for no account of
/// its hubs: no login, no folder, no scan.
pub(crate) fn seen_by_none(mut logins: MachineLogins) -> MachineLogins {
    logins.scan = None;
    logins.folders.clear();
    for agent in super::AGENTS {
        let side = logins.of_mut(agent).expect("an agent that keeps logins");
        side.programs.clear();
        side.candidates.clear();
        side.elsewhere.clear();
        side.listed.clear();
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
        || view.assisting(&asker.account, &view.me).any(|assist| {
            assist.agent == *agent
                && match &assist.home {
                    None => default,
                    Some(home) => crate::homes::same_dir(home, dir),
                }
        })
}

/// Asks `machine`'s node, of the user's own, for `call`, or this node when
/// it is this machine.
async fn ask(node: &Arc<Node>, machine: &Written, purpose: Purpose, call: RemoteCall) -> Result<RemoteReply, Failure> {
    node.call(&Acting::Here, machine, purpose, call).await.map_err(|error| Failure::new(error.to_string()))?
}

fn other(machine: &Written) -> Failure {
    Failure::new(format!("{machine} answered something other than its agents' logins; the World there may be older"))
}

pub(crate) async fn scan(node: &Arc<Node>, machine: Written, stop: bool) -> Result<MachineLogins, Failure> {
    if node.is_here(&machine) {
        return super::scan::scan(node, stop).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::ScanAgents { stop }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn add(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
) -> Result<MachineLogins, Failure> {
    if node.is_here(&machine) {
        return super::take_up(node, agent, dir).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::AddAgentLogin { agent, dir }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn folder(
    node: &Arc<Node>,
    machine: Written,
    dir: PathBuf,
    change: FolderChange,
) -> Result<MachineLogins, Failure> {
    if node.is_here(&machine) {
        return super::folders::change(node, dir, change).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::AgentFolder { dir, change }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn preview(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
    into: bool,
    take_back: bool,
) -> Result<ChangePreview, Failure> {
    if node.is_here(&machine) {
        return super::change::preview(node, agent, dir, into, take_back).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::PreviewAgentChange { agent, dir, into, take_back }).await? {
        RemoteReply::AgentChangePreview(preview) => Ok(*preview),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn change(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
    into: bool,
    raise_output_limit: bool,
    take_back: bool,
) -> Result<LoginChange, Failure> {
    if node.is_here(&machine) {
        return super::change::change(node, agent, dir, into, raise_output_limit, take_back).await;
    }
    let call = RemoteCall::ChangeAgentLogin { agent, dir, into, raise_output_limit, take_back };
    match ask(node, &machine, Purpose::Hands, call).await? {
        RemoteReply::AgentLoginChanged(change) => Ok(*change),
        _ => Err(other(&machine)),
    }
}

pub(crate) async fn read_limits(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
) -> Result<MachineLogins, Failure> {
    if node.is_here(&machine) {
        super::limits::read(node, &agent, &dir).await?;
        return super::here(node).await;
    }
    match ask(node, &machine, Purpose::Look, RemoteCall::ReadAgentLimits { agent, dir }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

/// Turns automatic naming of Codex conversations in a login on or off,
/// here or on another of the user's machines.
pub(crate) async fn set_title(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
    on: bool,
) -> Result<MachineLogins, Failure> {
    if node.is_here(&machine) {
        super::set_title(node, &agent, &dir, on)?;
        return super::here(node).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::SetAgentTitle { agent, dir, on }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

/// What `agent`'s login `dir` on `machine` was used for, as this computer's
/// user may see it.
pub(crate) async fn usage(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
) -> Result<UsageReport, Failure> {
    if node.is_here(&machine) {
        return here_usage(node, agent, dir, None).await;
    }
    match ask(node, &machine, Purpose::Look, RemoteCall::AgentUsage { agent, dir }).await? {
        RemoteReply::AgentUsage(report) => Ok(*report),
        _ => Err(other(&machine)),
    }
}

/// Request records with the financial scope and conversation ownership checked independently.
pub(crate) async fn calls(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
    query: fastctx_types::usage_calls::UsageCallsQuery,
) -> Result<fastctx_types::usage_calls::UsageCallsPage, Failure> {
    if node.is_here(&machine) {
        return here_calls(node, agent, dir, None, query).await;
    }
    match ask(node, &machine, Purpose::Look, RemoteCall::AgentCalls { agent, dir, query }).await? {
        RemoteReply::AgentCalls(page) => Ok(*page),
        _ => Err(other(&machine)),
    }
}

async fn here_calls(
    node: &Arc<Node>,
    agent: AgentName,
    dir: PathBuf,
    asker: Option<Asker>,
    query: fastctx_types::usage_calls::UsageCallsQuery,
) -> Result<fastctx_types::usage_calls::UsageCallsPage, Failure> {
    let node = node.clone();
    tokio::task::spawn_blocking(move || {
        use crate::admission::seen::Looker;
        let only =
            asker.as_ref().filter(|a| !a.hands).map(|a| HubAccount { hub: a.hub.clone(), account: a.account.clone() });
        let here = Acting::Here;
        let looker = asker.as_ref().map_or(Looker::Here(&here), Looker::Asker);
        crate::usage::calls(&node, agent.as_str(), &dir, only.as_ref(), looker, &query)
    })
    .await
    .map_err(|e| Failure::new(e.to_string()))?
    .map_err(|e| Failure::new(format!("reading the login's calls: {e}")))
}

/// What a login here was used for: everyone's, or `only`'s alone.
async fn here_usage(
    node: &Arc<Node>,
    agent: AgentName,
    dir: PathBuf,
    only: Option<HubAccount>,
) -> Result<UsageReport, Failure> {
    let node = node.clone();
    tokio::task::spawn_blocking(move || crate::usage::report(&node, agent.as_str(), &dir, only.as_ref()))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("reading what the login was used for: {error}")))
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
        RemoteCall::ScanAgents { stop } => super::scan::scan(node, stop).await.map(logins),
        RemoteCall::AddAgentLogin { agent, dir } => super::take_up(node, agent, dir).await.map(logins),
        RemoteCall::AgentFolder { dir, change } => super::folders::change(node, dir, change).await.map(logins),
        RemoteCall::SetAgentTitle { agent, dir, on } => {
            super::set_title(node, &agent, &dir, on)?;
            super::here(node).await.map(logins)
        }
        RemoteCall::PreviewAgentChange { agent, dir, into, take_back } => {
            super::change::preview(node, agent, dir, into, take_back)
                .await
                .map(|preview| RemoteReply::AgentChangePreview(Box::new(preview)))
        }
        RemoteCall::ChangeAgentLogin { agent, dir, into, raise_output_limit, take_back } => {
            super::change::change(node, agent, dir, into, raise_output_limit, take_back)
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
                // One a login only assists does not read its account's windows.
                .filter(|login| asker.hands && may_see(view, asker, &agent, &login.dir, login.default))
                .ok_or_else(|| {
                    Failure::not_run(format!("no {agent} login {} here is the account's to see", dir.display()))
                })?;
            let dir = login.dir.clone();
            super::limits::read(node, &agent, &dir).await?;
            super::here(node).await.map(logins)
        }
        RemoteCall::AgentUsage { agent, dir } => {
            let record = super::here(node).await?;
            let side = record.of(agent.as_str()).ok_or_else(|| Failure::not_run(format!("{agent} keeps no logins")))?;
            let login = side
                .listed
                .iter()
                .find(|login| crate::homes::same_dir(&login.dir, &dir))
                .filter(|login| may_see(view, asker, &agent, &login.dir, login.default))
                .ok_or_else(|| {
                    Failure::not_run(format!("no {agent} login {} here is the account's to see", dir.display()))
                })?;
            let only = (!asker.hands).then(|| HubAccount { hub: asker.hub.clone(), account: asker.account.clone() });
            here_usage(node, agent, login.dir.clone(), only)
                .await
                .map(|report| RemoteReply::AgentUsage(Box::new(report)))
        }
        RemoteCall::AgentCalls { agent, dir, query } => {
            let record = super::here(node).await?;
            let side = record.of(agent.as_str()).ok_or_else(|| Failure::not_run(format!("{agent} keeps no logins")))?;
            let login = side
                .listed
                .iter()
                .find(|login| crate::homes::same_dir(&login.dir, &dir))
                .filter(|login| may_see(view, asker, &agent, &login.dir, login.default))
                .ok_or_else(|| {
                    Failure::not_run(format!("no {agent} login {} here is the account's to see", dir.display()))
                })?;
            here_calls(node, agent, login.dir.clone(), Some(asker.clone()), query)
                .await
                .map(|page| RemoteReply::AgentCalls(Box::new(page)))
        }
        _ => Err(Failure::new("not a call about the agents' logins")),
    }
}
