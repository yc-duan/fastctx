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
#[derive(Clone)]
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
        RemoteCall::GoalCommand { actor, request, .. } => Some(request.resident(actor)),
        RemoteCall::Plan { resident: who, .. }
        | RemoteCall::Goal { resident: who, .. }
        | RemoteCall::ConversationDebt { resident: who, .. }
        | RemoteCall::Collaboration { resident: who, .. }
        | RemoteCall::EnterProject { resident: who, .. }
        | RemoteCall::Talk { to: who, .. }
        | RemoteCall::Composer { resident: who, .. }
        | RemoteCall::Watch { resident: who, .. }
        | RemoteCall::Stop { resident: who }
        | RemoteCall::Control { resident: who, .. }
        | RemoteCall::Manage { resident: who, .. }
        | RemoteCall::Offers { resident: who }
        | RemoteCall::ItemDetail { resident: who, .. }
        | RemoteCall::AgentActivity { resident: who, .. } => Some(who),
        RemoteCall::CommitAttachment { attachment, .. } => Some(&attachment.conversation().resident),
        RemoteCall::AttachmentPlaces { target, .. }
        | RemoteCall::Upload { target, .. }
        | RemoteCall::ReserveAttachment { target, .. } => match target.conversation_target() {
            fastctx_types::attachments::UploadTarget::Resident { resident } => Some(resident),
            _ => None,
        },
        _ => None,
    }
}

/// The project `call` is about, if it names one.
pub(crate) fn town_of(call: &RemoteCall) -> Option<&TownName> {
    match call {
        RemoteCall::EnterProject { town, .. } | RemoteCall::MemoryFolders { town } => Some(town),
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
        RemoteCall::AttachmentPlaces { target, .. }
        | RemoteCall::Upload { target, .. }
        | RemoteCall::ReserveAttachment { target, .. } => match target.conversation_target() {
            fastctx_types::attachments::UploadTarget::New { town, .. }
            | fastctx_types::attachments::UploadTarget::Draft { town, .. } => town.as_ref(),
            _ => None,
        },
        RemoteCall::Manage { action: Management::Move { town: Some(town) }, .. } => Some(town),
        _ => None,
    }
}

/// Refuses a call `caller` may not make here; otherwise, who asked.
pub fn admit(node: &Arc<Node>, link: &Arc<Link>, caller: &Caller, call: &RemoteCall) -> Result<Asker, Failure> {
    let here = node.machine();
    // Nothing of a call turned away here ran.
    let refuse = |why: String| Failure::refused(format!("{here} refused {}: {why}", caller.machine));
    let reached = match call {
        RemoteCall::AttachmentThumbnail { attachment, .. } => Some(attachment.thumbnail_owner()),
        RemoteCall::ReadAttachment { attachment, .. } => Some(attachment.owner()),
        _ => None,
    };
    if reached.is_some_and(|owner| !node.is_me(&owner.machine)) {
        return Err(refuse("the attachment names a different holding machine".into()));
    }
    let view = crate::authorization::current_view(link)?;
    if let RemoteCall::ConversationDebt { requester, .. } | RemoteCall::WaitingJobs { owner: requester } = call
        && !view.same(&requester.machine, &caller.machine)
    {
        return Err(refuse("only a requester's own machine may query its outstanding deliveries".into()));
    }
    if let RemoteCall::GoalCommand { actor, .. } = call
        && !view.same(&actor.machine, &caller.machine)
        && !view.rights(&caller.account, &caller.machine).hands
    {
        return Err(refuse("a forwarded goal command requires work permission on its sending machine".into()));
    }
    if let RemoteCall::AttachmentReferences { attachments, .. } = call
        && attachments.iter().any(|file| !view.same(&file.owner.machine, &caller.machine))
    {
        return Err(refuse("only the original holder may query its attachment references".into()));
    }
    view.authorize(
        &caller.account,
        fastctx_net::authorization::Action::Call { from: &caller.machine, to: &here, purpose: call.purpose() },
    )
    .map_err(|denial| {
        let mut failure = Failure::from(denial);
        failure.message = format!("{here} refused {}: {}", caller.machine, failure.message);
        failure
    })?;
    let agents_here = matches!(
        call,
        RemoteCall::ScanAgents { .. }
            | RemoteCall::AddAgentLogin { .. }
            | RemoteCall::AgentFolder { .. }
            | RemoteCall::SetAgentTitle { .. }
            | RemoteCall::PreviewAgentChange { .. }
            | RemoteCall::ChangeAgentLogin { .. }
            | RemoteCall::ClaudeConsent { .. }
            | RemoteCall::Homes { .. }
    );
    // A managed machine's agents work for no one: nobody scans them or puts
    // the World's tools in them, its owner neither (`world-network.md`, 受管机器).
    if agents_here && view.me().is_some_and(|me| me.managed) {
        return Err(refuse(format!("{here} is a managed machine, whose agents work for no one")));
    }
    // Login management belongs to holders (`world-network.md`, 共享机器与智能体协助).
    if agents_here {
        view.authorize(&caller.account, fastctx_net::authorization::Action::Hold(&here)).map_err(Failure::from)?;
    }
    // Updates use general machine management authority (`world-upgrade.md`, 谁来更新).
    if matches!(call, RemoteCall::UpdateWorld | RemoteCall::Updates { .. }) && !view.may_update(&caller.account, &here)
    {
        return Err(refuse(fastctx_types::sharing::MANAGER_REQUIRED.into()));
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
    if matches!(call, RemoteCall::EnterProject { .. }) && !view.rights(&caller.account, &caller.machine).hands {
        return Err(refuse("the requesting job no longer has work permission on its machine".into()));
    }
    let hands = view.rights(&caller.account, &here).hands;
    // A conversation's attachments are seen by the accounts it works for
    // alone (`seen`); a draft's, which no conversation holds yet, by whoever
    // may work here.
    let inheriting = match call {
        RemoteCall::ReadAttachment {
            attachment: fastctx_types::attachments::AttachmentSource::Upload(file), ..
        }
        | RemoteCall::AttachmentThumbnail {
            attachment: fastctx_types::attachments::AttachmentSource::Upload(file),
            ..
        } if node.is_me(&file.conversation().machine) && !node.residents.knows(&file.conversation().resident) => {
            crate::attachments::lifetime::reader(node, file)?
        }
        _ => None,
    };
    let resident = inheriting.as_ref().or_else(|| resident_of(call)).or_else(|| {
        reached.filter(|owner| !hands || node.residents.knows(&owner.resident)).map(|owner| &owner.resident)
    });
    if let Some(resident) = resident {
        if !node.residents.knows(resident) && matches!(call, RemoteCall::CommitAttachment { .. }) {
            return Ok(Asker { machine: caller.machine.clone(), hub, account: caller.account.clone(), hands });
        }
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
        if hands || crate::homes::may_use_login(node, &view, &caller.account, &here, agent, home)? {
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
