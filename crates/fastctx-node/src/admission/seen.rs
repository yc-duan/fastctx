//! What an account sees of this machine's answers to the World-wide asks
//! (`world-network.md`, 一台机器给几个账号用): an account that may work on
//! this machine sees everything on it, each resident with whom it works
//! for; one that may only use its agents sees only the residents that work
//! for it, with their jobs and what happened to them.

use fastctx_types::agents::AgentsReply;
use fastctx_types::run::JobOwner;
use fastctx_types::since::{SinceKind, SinceReply};
use fastctx_types::town::{Looked, WhoReply};
use fastctx_types::{Address, ResidentId};

use super::Asker;
use crate::Node;

/// Whether `asker` sees the resident `id` of this machine.
pub fn resident(node: &Node, asker: &Asker, id: &ResidentId) -> bool {
    match node.residents.about(id) {
        Some((about, _)) => asker.hands || node.serves(&about.acting, &asker.hub, &asker.account),
        None => false,
    }
}

fn address(node: &Node, asker: &Asker, address: &Address) -> bool {
    address.machine != node.machine() || resident(node, asker, &address.resident)
}

fn owner(node: &Node, asker: &Asker, owner: &JobOwner) -> bool {
    match owner {
        JobOwner::Resident(owner) => address(node, asker, owner),
        // A page's jobs belong to its project, which the call's project
        // check covers.
        JobOwner::Page { .. } => asker.hands,
    }
}

pub fn who(node: &Node, asker: &Asker, mut reply: WhoReply) -> WhoReply {
    if asker.hands {
        return reply;
    }
    reply.residents.retain(|now| resident(node, asker, &now.about.id));
    reply.jobs.retain(|job| owner(node, asker, &job.owner));
    reply.history = reply.history.map(|history| since(node, asker, history));
    reply
}

pub fn since(node: &Node, asker: &Asker, mut reply: SinceReply) -> SinceReply {
    if asker.hands {
        return reply;
    }
    let seen = |resident: &Address| address(node, asker, resident);
    reply.events.retain(|event| match &event.what {
        SinceKind::FileAction { resident, .. }
        | SinceKind::ResidentBorn { resident, .. }
        | SinceKind::TurnEnded { resident, .. }
        | SinceKind::ResidentMoved { resident, .. }
        | SinceKind::ClaudeRegistered { resident, .. }
        | SinceKind::SubmissionUncertain { resident, .. } => seen(resident),
        SinceKind::JobEnded { owner: job_owner, .. } => owner(node, asker, job_owner),
        SinceKind::ColleagueAsked { asker: who, colleague, .. } => seen(who) && seen(colleague),
        SinceKind::AnswerDelivery { colleague, to, .. } => seen(colleague) && seen(to),
        SinceKind::JournalEntry { .. }
        | SinceKind::JournalDescription { .. }
        | SinceKind::ProjectVersion { .. }
        | SinceKind::MachineConnected
        | SinceKind::MachineOffline => true,
    });
    reply.undated.retain(|fact| seen(&fact.resident));
    reply
}

/// Which agents are logged in here is the machine's, not a resident's, and
/// whoever may use the machine sees it.
pub fn agents(reply: AgentsReply) -> AgentsReply {
    reply
}

pub fn looked(node: &Node, asker: &Asker, looked: Option<Looked>) -> Option<Looked> {
    let seen = match &looked {
        None => true,
        Some(Looked::Resident(detail)) => address(
            node,
            asker,
            &Address { machine: detail.resident.about.machine.clone(), resident: detail.resident.about.id.clone() },
        ),
        Some(Looked::Job { record }) => owner(node, asker, &record.owner),
    };
    if seen { looked } else { None }
}

pub fn job(node: &Node, asker: &Asker, job: &fastctx_types::town::RunningJob) -> bool {
    owner(node, asker, &job.owner)
}
