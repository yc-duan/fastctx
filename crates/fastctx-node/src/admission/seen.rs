//! What a member's scope sees of this machine's answers to the World-wide
//! asks: the residents of the agents it may reach, in the projects it may
//! use, with their jobs and what happened to them.

use fastctx_net::scope::Scope;
use fastctx_types::agents::AgentsReply;
use fastctx_types::run::JobOwner;
use fastctx_types::since::{SinceKind, SinceReply, SinceTown};
use fastctx_types::town::{Looked, WhoReply};
use fastctx_types::{Address, TownName};

use crate::Node;

/// Whether `scope` sees the resident at `address` in `town`.
fn resident(node: &Node, scope: &Scope, address: &Address, town: Option<&TownName>) -> bool {
    scope.agents.contains(&address.resident.agent()) && town.is_none_or(|town| super::in_scope(node, scope, town))
}

fn owner(node: &Node, scope: &Scope, owner: &JobOwner, town: Option<&TownName>) -> bool {
    match owner {
        JobOwner::Resident(address) => resident(node, scope, address, town),
        JobOwner::Page { page } => super::in_scope(node, scope, &page.town),
    }
}

fn everything(scope: &Scope) -> bool {
    scope.agents.is_all() && scope.projects.is_all()
}

pub fn who(node: &Node, scope: &Scope, mut reply: WhoReply) -> WhoReply {
    if everything(scope) {
        return reply;
    }
    let machine = reply.machine.clone();
    reply.residents.retain(|now| {
        let address = Address { machine: machine.clone(), resident: now.about.id.clone() };
        resident(node, scope, &address, now.town.as_ref())
    });
    reply.jobs.retain(|job| {
        let town = match &job.owner {
            JobOwner::Resident(address) => node.residents.town_of(&address.resident).flatten(),
            JobOwner::Page { page } => Some(page.town.clone()),
        };
        owner(node, scope, &job.owner, town.as_ref())
    });
    reply
}

pub fn since(node: &Node, scope: &Scope, mut reply: SinceReply) -> SinceReply {
    if everything(scope) {
        return reply;
    }
    let town_seen = |town: &SinceTown| match town {
        SinceTown::Known { town } => town.as_ref().is_none_or(|town| super::in_scope(node, scope, town)),
        SinceTown::Unrecorded { recorded_in } => {
            recorded_in.iter().flatten().all(|town| super::in_scope(node, scope, town))
        }
    };
    let agent = |address: &Address| scope.agents.contains(&address.resident.agent());
    reply.events.retain(|event| {
        town_seen(&event.town)
            && match &event.what {
                SinceKind::ResidentBorn { resident, .. }
                | SinceKind::TurnEnded { resident, .. }
                | SinceKind::ResidentMoved { resident, .. }
                | SinceKind::ClaudeRegistered { resident, .. }
                | SinceKind::SubmissionUncertain { resident, .. } => agent(resident),
                SinceKind::JobEnded { owner: JobOwner::Resident(address), .. } => agent(address),
                SinceKind::JobEnded { owner: JobOwner::Page { page }, .. } => super::in_scope(node, scope, &page.town),
                SinceKind::ColleagueAsked { asker, colleague, .. } => agent(asker) && agent(colleague),
                SinceKind::AnswerDelivery { colleague, to, .. } => agent(colleague) && agent(to),
                SinceKind::JournalEntry { .. }
                | SinceKind::JournalDescription { .. }
                | SinceKind::ProjectVersion { .. }
                | SinceKind::MachineConnected
                | SinceKind::MachineOffline => true,
            }
    });
    reply.undated.retain(|fact| town_seen(&fact.town) && agent(&fact.resident));
    reply
}

pub fn agents(scope: &Scope, mut reply: AgentsReply) -> AgentsReply {
    reply.agents.retain(|facts| scope.agents.contains(&facts.agent));
    reply
}

pub fn looked(node: &Node, scope: &Scope, looked: Option<Looked>) -> Option<Looked> {
    let seen = match &looked {
        None => true,
        Some(Looked::Resident(detail)) => {
            let address =
                Address { machine: detail.resident.about.machine.clone(), resident: detail.resident.about.id.clone() };
            resident(node, scope, &address, detail.resident.town.as_ref())
        }
        Some(Looked::Job { record }) => owner(node, scope, &record.owner, record.town.as_ref()),
    };
    if seen { looked } else { None }
}
