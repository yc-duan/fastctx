//! What another machine may ask of this one (`world-network.md`, 一台机器
//! 给几个账号用): every call is checked at this door before anything of it
//! runs, a second look after the hub's.
//!
//! The door works out for itself what a call asks of this machine
//! (`RemoteCall::purpose`) and checks that against the view: that the caller may act
//! for the account it names, and that the account may do that here. A call
//! about one resident goes on only when that resident works for the
//! account; a call about a project, only when the account may use the
//! project. What asks of the whole machine (who is here, what happened
//! since) is answered with what the account may see (`seen`).

use std::sync::Arc;

use fastctx_net::link::{Caller, Link};
use fastctx_types::access::HubId;
use fastctx_types::ipc::Failure;
use fastctx_types::management::Management;
use fastctx_types::remote::RemoteCall;
use fastctx_types::{AccountName, MachineName, ResidentId, TownName};

use crate::Node;

pub mod letters;
pub mod seen;

/// Who asked, and what that lets it see here.
pub struct Asker {
    pub machine: MachineName,
    pub hub: HubId,
    pub account: AccountName,
    /// Whether the account may work on this machine, and so sees all of it.
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
        | RemoteCall::Colleagues { resident: who, .. }
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
        RemoteCall::On { origin, .. } | RemoteCall::Follow { origin, .. } | RemoteCall::StopOn { origin, .. } => {
            origin.town.as_ref()
        }
        RemoteCall::PageAction { source, .. } => Some(&source.town),
        RemoteCall::Upload { target: fastctx_types::attachments::UploadTarget::New { town, .. }, .. } => town.as_ref(),
        RemoteCall::Manage { action: Management::Move { town: Some(town) }, .. } => Some(town),
        _ => None,
    }
}

/// Refuses a call `caller` may not make here; otherwise, who asked.
pub fn admit(node: &Arc<Node>, link: &Arc<Link>, caller: &Caller, call: &RemoteCall) -> Result<Asker, Failure> {
    let here = node.machine();
    // Nothing of a call turned away here ran.
    let refuse = |why: String| Failure::not_run(format!("{here} refused {}: {why}", caller.machine));
    let view = link.view();
    view.admits(&caller.machine, &caller.account, &here, call.purpose()).map_err(refuse)?;
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
    if let Some(resident) = resident_of(call) {
        let serves = node
            .residents
            .about(resident)
            .is_some_and(|(about, _)| node.serves(&about.acting, &hub, &caller.account));
        if !serves {
            return Err(refuse(format!("{resident} does not work for the account {}", caller.account)));
        }
    }
    // Consent to Claude Code's bypass mode is for the Claude logged in on
    // this machine, and so its owner's alone.
    if matches!(call, RemoteCall::ClaudeConsent { .. }) && view.me().and_then(|me| me.home.as_ref()) != Some(&caller.account)
    {
        return Err(refuse(format!(
            "only the account this machine is registered under gives that consent, not {}",
            caller.account
        )));
    }
    let hands = view.rights(&caller.account, &here).hands;
    Ok(Asker { machine: caller.machine.clone(), hub, account: caller.account.clone(), hands })
}
