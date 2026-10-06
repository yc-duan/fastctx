//! What another member may ask of this machine (`world-network.md`, 管理权
//! 与范围): every call is checked at this door against the scope of the
//! member that makes it, as this machine works it out from the members
//! table, before anything of it runs. A call is refused with what its
//! scope lacks: this machine, the action, the kind of agent it reaches, or
//! the project it is about.
//!
//! What asks of the whole World (who is here, what happened since, the
//! agents, a resident in detail) is answered with what the asker's scope
//! covers and nothing more, so a member sees only what it may use.

use std::sync::Arc;

use fastctx_net::scope::{Action, Denied, Scope};
use fastctx_net::trust::Member;
use fastctx_types::ipc::Failure;
use fastctx_types::management::Management;
use fastctx_types::remote::RemoteCall;
use fastctx_types::{AgentName, MachineName, ResidentId, TownName};

use crate::Node;

pub mod letters;
pub mod seen;

/// What `call` does here, in the terms of a scope: the action, the agent
/// it reaches, and the project it is about.
struct Asked<'a> {
    action: Action,
    agent: Option<AgentName>,
    town: Option<&'a TownName>,
    /// The resident of this machine it reaches, whose project counts too.
    resident: Option<&'a ResidentId>,
}

fn asked(call: &RemoteCall) -> Asked<'_> {
    let reaches = match call {
        RemoteCall::Talk { to: who, .. }
        | RemoteCall::Composer { resident: who, .. }
        | RemoteCall::Watch { resident: who, .. }
        | RemoteCall::Stop { resident: who }
        | RemoteCall::Control { resident: who, .. }
        | RemoteCall::Manage { resident: who, .. }
        | RemoteCall::Offers { resident: who }
        | RemoteCall::Upload {
            target:
                fastctx_types::attachments::UploadTarget::Resident { resident: who }
                | fastctx_types::attachments::UploadTarget::Draft { resident: who, .. },
            ..
        } => Some(who),
        _ => None,
    };
    let resident = |resident: &ResidentId| Some(resident.agent());
    let (action, agent, town) = match call {
        RemoteCall::Run { town, .. } => (Action::Run, None, town.as_ref()),
        RemoteCall::Write { town, .. } => (Action::Write, None, town.as_ref()),
        RemoteCall::CopySend { .. } | RemoteCall::CopyReceive { .. } => (Action::Copy, None, None),
        // A colleague is a Codex resident.
        RemoteCall::Ask { town, .. } => (Action::AskColleague, Some(codex()), town.as_ref()),
        RemoteCall::Who { town, .. } | RemoteCall::Since { town, .. } => (Action::W, None, town.as_ref()),
        RemoteCall::Agents | RemoteCall::Facts | RemoteCall::Look { .. } | RemoteCall::Find { .. } => {
            (Action::W, None, None)
        }
        RemoteCall::Talk { to, .. } => (Action::Talk, resident(to), None),
        RemoteCall::Watch { resident: who, .. }
        | RemoteCall::Composer { resident: who, .. }
        | RemoteCall::Stop { resident: who }
        | RemoteCall::Control { resident: who, .. }
        | RemoteCall::Manage { resident: who, .. }
        | RemoteCall::Offers { resident: who } => (Action::Talk, resident(who), None),
        RemoteCall::Open { agent, town, .. } => (Action::Talk, Some(agent.clone()), town.as_ref()),
        RemoteCall::Upload { target, .. } => match target {
            fastctx_types::attachments::UploadTarget::Resident { resident: who }
            | fastctx_types::attachments::UploadTarget::Draft { resident: who, .. } => {
                (Action::Talk, resident(who), None)
            }
            fastctx_types::attachments::UploadTarget::New { agent, town } => {
                (Action::Talk, Some(agent.clone()), town.as_ref())
            }
        },
        RemoteCall::FindFiles { .. } => (Action::Talk, None, None),
        RemoteCall::ReadFile { .. } | RemoteCall::Canon { .. } => (Action::Read, None, None),
        RemoteCall::ClaudeConsent { .. } => (Action::Consent, Some(claude()), None),
        RemoteCall::PageAction { source, .. } => (Action::Page, None, Some(&source.town)),
        // Checked by what it asks, in `devices::relay`.
        RemoteCall::Relay { .. } => (Action::W, None, None),
    };
    Asked { action, agent, town, resident: reaches }
}

fn codex() -> AgentName {
    AgentName::try_from("codex").expect("a valid agent name")
}

fn claude() -> AgentName {
    AgentName::try_from("claude").expect("a valid agent name")
}

/// The member `from` as this machine trusts it.
pub fn member(node: &Node, from: &MachineName) -> Result<Member, Failure> {
    node.link()
        .and_then(|link| link.members().0.get(from).cloned())
        .ok_or_else(|| Failure::new(format!("{from} is not a member this machine trusts")))
}

/// Refuses a call `from` may not make.
pub fn admit(node: &Arc<Node>, from: &MachineName, call: &RemoteCall) -> Result<Member, Failure> {
    let member = member(node, from)?;
    crate::devices::admit(&member, call)?;
    if matches!(call, RemoteCall::Relay { .. }) {
        return Ok(member);
    }
    let asked = asked(call);
    let here = node.machine();
    let refuse = |denied: Denied| Failure::new(format!("{} refused {from}: {denied}", here));
    // Consent to Claude Code's bypass mode is the user's alone, and only a
    // member that manages the World speaks for the user on that.
    if asked.action == Action::Consent && !member.manage && member.kind.is_machine() {
        return Err(Failure::new(format!(
            "{here} refused {from}: only the user, from a device or a machine with management rights, gives that consent"
        )));
    }
    member.scope.allows(&here, asked.action, asked.agent.as_ref()).map_err(refuse)?;
    if let Some(town) = asked.town
        && !in_scope(node, &member.scope, town)
    {
        return Err(refuse(Denied::Project));
    }
    if let RemoteCall::Manage { action: Management::Move { town: Some(town) }, .. } = call
        && !in_scope(node, &member.scope, town)
    {
        return Err(refuse(Denied::Project));
    }
    if let Some(resident) = asked.resident
        && let Some(Some(town)) = node.residents.town_of(resident)
        && !in_scope(node, &member.scope, &town)
    {
        return Err(refuse(Denied::Project));
    }
    Ok(member)
}

/// Whether `scope` covers the project `town`.
pub fn in_scope(node: &Node, scope: &Scope, town: &TownName) -> bool {
    if scope.projects.is_all() {
        return true;
    }
    crate::ledger::names::town_id(node, town).is_some_and(|id| scope.projects.contains(&id))
}

/// Whether `scope` covers a resident `id` of this machine, in `town`.
pub fn sees(node: &Node, scope: &Scope, resident: &ResidentId, town: Option<&TownName>) -> bool {
    scope.agents.contains(&resident.agent()) && town.is_none_or(|town| in_scope(node, scope, town))
}

/// Whether this machine's own scope covers `machine`: what it asks around
/// the World, it asks only of those, so its residents and its user see only
/// what they may use.
pub fn reaches(node: &Node, table: &fastctx_net::trust::Table, machine: &MachineName) -> bool {
    table.get(&node.machine()).is_none_or(|me| me.scope.machines.contains(machine))
}
