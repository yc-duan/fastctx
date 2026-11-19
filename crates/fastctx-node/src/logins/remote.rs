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
use fastctx_types::logins::models::ModelChange;
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
/// that may work here; to one logins assist, those listed logins alone.
/// One a login assists sees nothing of its account's windows, which are
/// their owner's, unless they manage the login with its owner: such a one
/// sees its windows as its owner does, only to look at, whatever share they
/// took for themselves (用户 2026-11-19: 共管，与份额分开；an assistance kept
/// before the word manages where it has no limit, `Assist::manages`).
pub(crate) fn seen(view: &fastctx_net::directory::View, asker: &Asker, mut logins: MachineLogins) -> MachineLogins {
    if asker.hands {
        return logins;
    }
    let here = view.me.clone();
    logins.scan = None;
    logins.folders.clear();
    // No login of the other agents assists anyone.
    logins.others.clear();
    for agent in super::AGENTS {
        let side = logins.of_mut(agent).expect("an agent that keeps logins");
        side.programs.clear();
        side.candidates.clear();
        side.elsewhere.clear();
        let assists = |login: &fastctx_types::logins::Login| -> Vec<fastctx_net::directory::Assist> {
            view.assisting(&asker.account, &here)
                .filter(|assist| {
                    assist.agent.as_str() == agent
                        && match &assist.home {
                            None => login.default,
                            Some(dir) => crate::homes::same_dir(dir, &login.dir),
                        }
                })
                .cloned()
                .collect()
        };
        side.listed.retain(|login| !assists(login).is_empty());
        for login in &mut side.listed {
            login.programs.clear();
            login.change = None;
            // Codex keeps no shares: a Codex login that assists with no limit
            // shows its windows as it did before there were shares.
            let sees = |assist: &fastctx_net::directory::Assist| {
                assist.manages() || (assist.agent.as_str() != "claude" && !limited(assist))
            };
            if assists(login).iter().any(|assist| !sees(assist)) {
                login.limits = fastctx_types::agents::Fact::unavailable(OWNERS_WINDOWS);
            }
        }
    }
    logins
}

/// Whether an assistance holds the account to a part of the login: a share
/// of its week, or dollars a week or a day.
fn limited(assist: &fastctx_net::directory::Assist) -> bool {
    assist.share.is_some() || assist.weekly.is_some() || assist.daily.is_some()
}

/// Whether `asker` manages `agent`'s login `dir` here with its owner: it sees
/// everyone's use of it and the account's windows, as its owner does.
fn manages(
    view: &fastctx_net::directory::View,
    asker: &Asker,
    agent: &AgentName,
    dir: &std::path::Path,
    default: bool,
) -> bool {
    view.assisting(&asker.account, &view.me).any(|assist| {
        assist.agent == *agent
            && assist.manages()
            && match &assist.home {
                None => default,
                Some(home) => crate::homes::same_dir(home, dir),
            }
    })
}

/// What this machine's record shows a caller that works for no account of
/// its hubs: no login, no folder, no scan.
pub(crate) fn seen_by_none(mut logins: MachineLogins) -> MachineLogins {
    logins.scan = None;
    logins.folders.clear();
    logins.others.clear();
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
        return super::submitted::scan(node, stop).await;
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
        return super::submitted::add(node, agent, dir).await;
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
        return super::submitted::folder(node, dir, change).await;
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
    workbench: Option<bool>,
) -> Result<ChangePreview, Failure> {
    if node.is_here(&machine) {
        return super::change::preview(node, agent, dir, into, take_back, workbench).await;
    }
    let call = RemoteCall::PreviewAgentChange { agent, dir, into, take_back, workbench };
    match ask(node, &machine, Purpose::Hands, call).await? {
        RemoteReply::AgentChangePreview(preview) => Ok(*preview),
        _ => Err(other(&machine)),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn change(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
    into: bool,
    raise_output_limit: bool,
    take_back: bool,
    expected: Option<String>,
    workbench: Option<bool>,
) -> Result<LoginChange, Failure> {
    if node.is_here(&machine) {
        return super::submitted::change(node, agent, dir, into, raise_output_limit, take_back, expected, workbench)
            .await;
    }
    let call = RemoteCall::ChangeAgentLogin { agent, dir, into, raise_output_limit, take_back, expected, workbench };
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
        return super::submitted::read_limits(node, agent, dir, false).await;
    }
    match ask(node, &machine, Purpose::Look, RemoteCall::ReadAgentLimits { agent, dir }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

/// Turns on or off whether a week cleared before its reset counts anew in a
/// login, here or on another of the user's machines; the holds of those it
/// assists are looked at again.
pub(crate) async fn set_resets_in_place(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
    on: bool,
) -> Result<MachineLogins, Failure> {
    if node.is_here(&machine) {
        return super::submitted::set_resets_in_place(node, agent, dir, on).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::SetAgentResetInPlace { agent, dir, on }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

/// Sets what a login's week is expected to hold, in quota dollars, and how
/// far a week's figure may stand from it, here or on another of the user's
/// machines; the holds of those it assists are looked at again.
pub(crate) async fn set_week(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
    week: Option<fastctx_types::usage::Usd>,
    swing: Option<u8>,
) -> Result<MachineLogins, Failure> {
    if node.is_here(&machine) {
        super::set_week(node, &agent, &dir, week, swing)?;
        crate::usage::after_reading(node, agent.as_str(), &dir).await;
        return super::changed_here(node).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::SetAgentWeek { agent, dir, week, swing }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

/// The week a reset was asked of, as its maker looked at it.
pub(crate) struct Looked {
    pub week_end: Option<jiff::Timestamp>,
    pub holder: Option<String>,
}

/// Resets what everyone has used of this week of a login, or takes such a
/// reset back, here or on another of the user's machines; the holds of
/// those it assists are looked at again.
pub(crate) async fn reset_usage(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
    id: String,
    to_tenths: Option<u32>,
    looked: Looked,
) -> Result<MachineLogins, Failure> {
    let Looked { week_end, holder } = looked;
    if node.is_here(&machine) {
        let asked = crate::usage::ResetAsked { id: &id, to_tenths, week_end, holder, by: None };
        super::reset_usage(node, &agent, &dir, asked)?;
        crate::usage::after_reading(node, agent.as_str(), &dir).await;
        return super::changed_here(node).await;
    }
    let call = RemoteCall::ResetAgentUsage { agent, dir, id, to_tenths, week_end, holder };
    match ask(node, &machine, Purpose::Hands, call).await? {
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
        return super::changed_here(node).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::SetAgentTitle { agent, dir, on }).await? {
        RemoteReply::AgentLogins(logins) => Ok(*logins),
        _ => Err(other(&machine)),
    }
}

/// Changes the models a Claude Code login offers, here or on another of the
/// user's machines.
pub(crate) async fn change_models(
    node: &Arc<Node>,
    machine: Written,
    agent: AgentName,
    dir: PathBuf,
    change: ModelChange,
) -> Result<MachineLogins, Failure> {
    if node.is_here(&machine) {
        super::models::change(node, &agent, &dir, &change)?;
        return super::changed_here(node).await;
    }
    match ask(node, &machine, Purpose::Hands, RemoteCall::ChangeAgentModels { agent, dir, change }).await? {
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
        return here_usage(node, agent, dir, crate::usage::Reader::WHOLE).await;
    }
    // The machine, which tells all of it as the asker may see it; from the
    // copies here where it is away or does not answer (§28.58): they hold
    // only the lines of the accounts logged in here, never another's the
    // asker may see on that machine (codex-357de52a #5).
    let copied = || {
        let (asking, written, named, path) = (node.clone(), machine.clone(), agent.clone(), dir.clone());
        fastctx_platform::task::spawn_blocking(move || {
            crate::usage::copied::report(&asking, &written, named.as_str(), &path)
        })
    };
    if crate::timeline::copy::away(node, &machine.name)
        && let Ok(Some(report)) = copied().await
    {
        return Ok(report);
    }
    match ask(node, &machine, Purpose::Look, RemoteCall::AgentUsage { agent: agent.clone(), dir: dir.clone() }).await {
        Ok(RemoteReply::AgentUsage(report)) => Ok(*report),
        Ok(_) => Err(other(&machine)),
        Err(failure) => match copied().await {
            Ok(Some(report)) => Ok(report),
            _ => Err(failure),
        },
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
    // The machine, or the copies here where it is away, as the report
    // (`usage`).
    let copied = || {
        let (asking, written, named, path, asked) =
            (node.clone(), machine.clone(), agent.clone(), dir.clone(), query.clone());
        fastctx_platform::task::spawn_blocking(move || {
            crate::usage::copied::calls(&asking, &written, named.as_str(), &path, &asked)
        })
    };
    if crate::timeline::copy::away(node, &machine.name)
        && let Ok(Some(page)) = copied().await
    {
        return Ok(page);
    }
    match ask(
        node,
        &machine,
        Purpose::Look,
        RemoteCall::AgentCalls { agent: agent.clone(), dir: dir.clone(), query: query.clone() },
    )
    .await
    {
        Ok(RemoteReply::AgentCalls(page)) => Ok(*page),
        Ok(_) => Err(other(&machine)),
        Err(failure) => match copied().await {
            Ok(Some(page)) => Ok(page),
            _ => Err(failure),
        },
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
    fastctx_platform::task::spawn_blocking(move || {
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

/// What a login here was used for, as `reader` may see it.
async fn here_usage(
    node: &Arc<Node>,
    agent: AgentName,
    dir: PathBuf,
    reader: crate::usage::Reader,
) -> Result<UsageReport, Failure> {
    crate::usage::stir(node, agent.as_str(), &dir);
    let node = node.clone();
    fastctx_platform::task::spawn_blocking(move || crate::usage::report(&node, agent.as_str(), &dir, &reader))
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
        RemoteCall::ScanAgents { stop } => super::submitted::scan(node, stop).await.map(logins),
        RemoteCall::AddAgentLogin { agent, dir } => super::submitted::add(node, agent, dir).await.map(logins),
        RemoteCall::AgentFolder { dir, change } => super::submitted::folder(node, dir, change).await.map(logins),
        RemoteCall::SetAgentTitle { agent, dir, on } => {
            super::set_title(node, &agent, &dir, on)?;
            super::changed_here(node).await.map(logins)
        }
        RemoteCall::SetAgentResetInPlace { agent, dir, on } => {
            super::submitted::set_resets_in_place(node, agent, dir, on).await.map(logins)
        }
        RemoteCall::SetAgentWeek { agent, dir, week, swing } => {
            super::set_week(node, &agent, &dir, week, swing)?;
            crate::usage::after_reading(node, agent.as_str(), &dir).await;
            super::changed_here(node).await.map(logins)
        }
        RemoteCall::ResetAgentUsage { agent, dir, id, to_tenths, week_end, holder } => {
            // Only one who may work here asks this (`admission`).
            let by = Some(asker.account.to_string());
            let asked = crate::usage::ResetAsked { id: &id, to_tenths, week_end, holder, by };
            super::reset_usage(node, &agent, &dir, asked)?;
            crate::usage::after_reading(node, agent.as_str(), &dir).await;
            super::changed_here(node).await.map(logins)
        }
        RemoteCall::ChangeAgentModels { agent, dir, change } => {
            super::models::change(node, &agent, &dir, &change)?;
            super::changed_here(node).await.map(logins)
        }
        RemoteCall::PreviewAgentChange { agent, dir, into, take_back, workbench } => {
            super::change::preview(node, agent, dir, into, take_back, workbench)
                .await
                .map(|preview| RemoteReply::AgentChangePreview(Box::new(preview)))
        }
        RemoteCall::ChangeAgentLogin { agent, dir, into, raise_output_limit, take_back, expected, workbench } => {
            super::submitted::change(node, agent, dir, into, raise_output_limit, take_back, expected, workbench)
                .await
                .map(|change| RemoteReply::AgentLoginChanged(Box::new(change)))
        }
        RemoteCall::ReadAgentLimits { agent, dir } => {
            // Only a holder reads account windows. Check before recording work;
            // the job resolves the requested login from the machine's record.
            if !asker.hands {
                return Err(Failure::not_run(format!(
                    "no {agent} login {} here is the account's to see",
                    dir.display()
                )));
            }
            super::submitted::read_limits(node, agent, dir, true).await.map(logins)
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
            // Everyone's use to one who may work here; to one who manages the
            // login with its owner, their own and what each other person used
            // of the week (用户 2026-11-19); their own alone to anyone else.
            let managing = !asker.hands && manages(view, asker, &agent, &login.dir, login.default);
            let reader = crate::usage::Reader {
                only: (!asker.hands).then(|| HubAccount { hub: asker.hub.clone(), account: asker.account.clone() }),
                managing,
                owner: crate::placement::owned(view.member(&view.me), &asker.account),
                may_reset: asker.hands,
            };
            here_usage(node, agent, login.dir.clone(), reader)
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
