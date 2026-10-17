//! What another machine may ask of this one (`world-network.md`, 一台机器
//! 给几个账号用): every call is checked at this door before anything of it
//! runs, a second look after the hub's.
//!
//! The door works out for itself what a call asks of this machine
//! (`RemoteCall::purpose`) and checks that against the view: that the caller may act
//! for the account it names, and that the account may do that here. A call
//! that starts a session goes on only when the account may use that agent's
//! login here; a call about one resident, only when that resident works for
//! the account and its login is still the account's to use; a call about a
//! project, only when the account may use the project. What asks of the
//! whole machine (who is here, what happened since, its agents) is answered
//! with what the account may see (`seen`).

use std::sync::Arc;

use fastctx_net::link::{Caller, Link};
use fastctx_types::access::HubId;
use fastctx_types::ipc::Failure;
use fastctx_types::management::Management;
use fastctx_types::remote::RemoteCall;
use fastctx_types::{AccountName, MachineName, ResidentId, TownName};

use crate::Node;

pub mod letters;
pub mod local;
pub mod seen;

/// Who asked, and what that lets it see here.
pub struct Asker {
    pub machine: MachineName,
    pub hub: HubId,
    pub account: AccountName,
    /// Whether the account may work on this machine: it sees the machine's
    /// agents and pages, never a conversation that works for another (`seen`).
    pub hands: bool,
}

/// The resident of this machine `call` is about, if it is about one.
fn resident_of(call: &RemoteCall) -> Option<&ResidentId> {
    match call {
        RemoteCall::Talk { to: who, .. }
        | RemoteCall::Composer { resident: who, .. }
        | RemoteCall::Watch { resident: who, .. }
        | RemoteCall::Stop { resident: who }
        | RemoteCall::Control { resident: who, .. }
        | RemoteCall::Manage { resident: who, .. }
        | RemoteCall::Offers { resident: who }
        | RemoteCall::ItemDetail { resident: who, .. }
        | RemoteCall::AgentActivity { resident: who, .. }
        | RemoteCall::Upload {
            target:
                fastctx_types::attachments::UploadTarget::Resident { resident: who }
                | fastctx_types::attachments::UploadTarget::Draft { resident: who, .. },
            ..
        } => Some(who),
        _ => None,
    }
}

/// The project `call` is about, if it names one.
fn town_of(call: &RemoteCall) -> Option<&TownName> {
    match call {
        RemoteCall::Run { town, .. }
        | RemoteCall::Write { town, .. }
        | RemoteCall::Ask { town, .. }
        | RemoteCall::Who { town, .. }
        | RemoteCall::Since { town, .. }
        | RemoteCall::Open { town, .. } => town.as_ref(),
        RemoteCall::On { origin, .. }
        | RemoteCall::Follow { origin, .. }
        | RemoteCall::StopOn { origin, .. }
        | RemoteCall::PipeOut { origin, .. } => origin.town.as_ref(),
        RemoteCall::PageAction { source, .. } => Some(&source.town),
        RemoteCall::Place { attachment, .. } => attachment.town.as_ref(),
        RemoteCall::Upload { target: fastctx_types::attachments::UploadTarget::New { town, .. }, .. } => town.as_ref(),
        RemoteCall::Manage { action: Management::Move { town: Some(town) }, .. } => Some(town),
        _ => None,
    }
}

/// Refuses a call `caller` may not make here; otherwise, who asked.
pub fn admit(node: &Arc<Node>, link: &Arc<Link>, caller: &Caller, call: &RemoteCall) -> Result<Asker, Failure> {
    let here = node.machine();
    // Nothing of a call turned away here ran.
    let refuse = |why: String| Failure::refused(format!("{here} refused {}: {why}", caller.machine));
    if let RemoteCall::AttachmentThumbnail { attachment, .. } | RemoteCall::ReadAttachment { attachment, .. } = call
        && !node.is_me(&attachment.owner().machine)
    {
        return Err(refuse("the attachment names a different holding machine".into()));
    }
    let view = link.view();
    view.admits(&caller.machine, &caller.account, &here, call.purpose()).map_err(refuse)?;
    let agents_here = matches!(
        call,
        RemoteCall::ScanAgents { .. }
            | RemoteCall::AddAgentLogin { .. }
            | RemoteCall::AgentFolder { .. }
            | RemoteCall::SetAgentTitle { .. }
            | RemoteCall::PreviewAgentChange { .. }
            | RemoteCall::ChangeAgentLogin { .. }
    );
    // A managed machine's agents work for no one: nobody scans them or puts
    // the World's tools in them, its owner neither (`world-network.md`, 受管机器).
    if (agents_here || matches!(call, RemoteCall::ClaudeConsent { .. })) && view.me().is_some_and(|me| me.managed) {
        return Err(refuse(format!("{here} is a managed machine, whose agents work for no one")));
    }
    // Which logins this machine lists, and what is put into them, is its
    // owner's alone (`world-agent-logins.md`): shared or administered, the
    // machine is not another account's to scan or change.
    if agents_here && view.member(&here).and_then(|member| member.home.as_ref()) != Some(&caller.account) {
        return Err(refuse(format!(
            "only {here}'s owner scans it or changes its agents, not the account {}",
            caller.account
        )));
    }
    // Which version a machine runs is its owner's to choose, and the
    // network's super administrator's but for a visitor's machine
    // (`world-upgrade.md`, 谁来更新).
    if matches!(call, RemoteCall::UpdateWorld | RemoteCall::Updates { .. }) {
        let owner = view.member(&here).and_then(|member| member.home.as_ref());
        let own = owner == Some(&caller.account);
        let administers = view.is_super(&caller.account) && owner.is_some_and(|owner| !view.is_visitor(owner));
        if !own && !administers {
            return Err(refuse(format!("only {here}'s owner updates it, not the account {}", caller.account)));
        }
    }
    crate::devices::admit(&view, &caller.machine, call)?;
    let hub = link.membership().id.clone();
    if let Some(town) = town_of(call) {
        let allowed = view
            .projects
            .iter()
            .find(|project| project.name == *town)
            .is_some_and(|project| view.in_project(&caller.account, &project.town));
        if !allowed {
            return Err(refuse(format!("the account {} may not use the project {town}", caller.account)));
        }
    }
    let hands = view.rights(&caller.account, &here).hands;
    // A conversation's attachments are seen by the accounts it works for
    // alone (`seen`); a draft's, which no conversation holds yet, by whoever
    // may work here.
    let resident = resident_of(call).or(match call {
        RemoteCall::AttachmentThumbnail { attachment, .. } | RemoteCall::ReadAttachment { attachment, .. }
            if !hands || node.residents.knows(&attachment.owner().resident) =>
        {
            Some(&attachment.owner().resident)
        }
        _ => None,
    });
    if let Some(resident) = resident {
        if !node.residents.knows(resident) {
            return Err(crate::forget::gone(node, resident));
        }
        let serves =
            node.residents.about(resident).is_some_and(|(about, _)| node.serves(&about.acting, &hub, &caller.account));
        if !serves {
            return Err(refuse(format!("{resident} does not work for the account {}", caller.account)));
        }
    }
    // Of another's machine, an account its agents' logins assist uses
    // those logins alone (`world-network.md`, 共享机器与智能体协助).
    let login = |agent: &fastctx_types::AgentName, home: Option<&std::path::Path>| {
        if hands || view.may_use_login(&caller.account, &here, agent, home) {
            return Ok(());
        }
        let login = home.map(|home| home.display().to_string()).unwrap_or_else(|| "default".to_string());
        Err(refuse(format!("the {agent} login {login} on {here} does not assist the account {}", caller.account)))
    };
    match call {
        RemoteCall::Open { agent, home, .. } => login(agent, home.as_deref())?,
        RemoteCall::Ask { started, .. } => login(&started.callee.login.agent, started.callee.login.home.as_deref())?,
        _ => {}
    }
    if let Some(resident) = resident
        && !hands
        && let Some((about, _)) = node.residents.about(resident)
    {
        login(&about.host.agent, about.home.as_deref())?;
    }
    Ok(Asker { machine: caller.machine.clone(), hub, account: caller.account.clone(), hands })
}
