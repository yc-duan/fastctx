//! What this machine's own callers may do to one of its conversations
//! (`world-gui.md`, 侧栏、首页与机器页上列的对话): what another machine's
//! account may, by whom the caller works for (`super::seen`). The user here
//! works for the accounts this computer is logged in to and for the
//! conversations opened here; a session, for the accounts its conversation
//! works for. A conversation that works for none of them is not there to
//! the caller: it is neither listed nor reached, its machine's owner and the
//! network's administrator included.

use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::access::Acting;
use fastctx_types::agents::AgentsReply;
use fastctx_types::attachments::UploadTarget;
use fastctx_types::ipc::{Failure, Peer};
use fastctx_types::logins::MachineLogins;
use fastctx_types::{Call, ResidentId};

use super::Asker;
use super::seen::{self, Looker};
use crate::Node;
use crate::town::caller::{self, Caller};

/// Refuses `call`, from the other end of a connection whose peer is
/// `peer`, when it reaches a conversation of this machine that works for
/// none of the accounts the caller works for.
pub fn admit(node: &Node, peer: &Peer, call: &Call) -> Result<(), Failure> {
    let inheriting = match call {
        Call::ReadAttachment { attachment: fastctx_types::attachments::AttachmentSource::Upload(file), .. }
        | Call::AttachmentThumbnail {
            attachment: fastctx_types::attachments::AttachmentSource::Upload(file), ..
        } if node.is_me(&file.conversation().machine) && !node.residents.knows(&file.conversation().resident) => {
            crate::attachments::lifetime::reader(node, file)?
        }
        _ => None,
    };
    let Some(resident) = inheriting.as_ref().or_else(|| reached(node, call)) else { return Ok(()) };
    // One this machine does not know is another machine's, or gone, which
    // the call's own answer says.
    if !node.residents.knows(resident) {
        return Ok(());
    }
    let caller = match (caller::of(node, peer), call) {
        (Ok(caller), _) => caller,
        // A session's own channel speaks as the resident it names.
        (Err(_), Call::Message { resident: speaker, .. }) => {
            Caller::Resident(fastctx_types::Address { machine: node.machine(), resident: speaker.clone() })
        }
        (Err(_), _) => return Ok(()),
    };
    let acting = crate::around::acting_of(node, &caller);
    if seen::resident(node, Looker::Here(&acting), resident) {
        return Ok(());
    }
    let whom = match caller {
        Caller::Resident(address) => address.resident.to_string(),
        _ => "this computer's logins".to_string(),
    };
    Err(Failure::refused(format!("{resident} works for none of the accounts {whom} works for")))
}

/// This machine's agents as a caller that may not work here sees them: an
/// agent working for an account some login here assists, the machine
/// itself not given to it (`world-network.md`, 共享机器与智能体协助). It sees
/// those logins alone, as that account would asking from another machine.
pub fn agents(node: &Node, acting: &Acting, reply: AgentsReply) -> AgentsReply {
    let mut agents = Vec::new();
    for (link, asker) in askers(node, acting) {
        for facts in seen::agents(&link.view(), &asker, reply.clone()).agents {
            if !agents.contains(&facts) {
                agents.push(facts);
            }
        }
    }
    AgentsReply { agents, ..reply }
}

/// This machine's record of its agents' logins, as a caller that may not
/// work here sees it (`agents`).
pub fn logins(node: &Node, acting: &Acting, logins: MachineLogins) -> MachineLogins {
    let mut seen: Option<MachineLogins> = None;
    for (link, asker) in askers(node, acting) {
        let more = crate::logins::remote::seen(&link.view(), &asker, logins.clone());
        let Some(kept) = &mut seen else {
            seen = Some(more);
            continue;
        };
        for agent in crate::logins::AGENTS {
            let listed = &mut kept.of_mut(agent).expect("an agent that keeps logins").listed;
            for login in &more.of(agent).expect("an agent that keeps logins").listed {
                if !listed.contains(login) {
                    listed.push(login.clone());
                }
            }
        }
    }
    seen.unwrap_or_else(|| crate::logins::remote::seen_by_none(logins))
}

/// The accounts `acting` works for, each as the door would see it asking
/// from another machine of its hub.
fn askers(node: &Node, acting: &Acting) -> Vec<(Arc<Link>, Asker)> {
    let mut askers = Vec::new();
    for link in node.links() {
        let view = link.view();
        let here = view.me.clone();
        for account in node.accounts_on(&link, acting) {
            let hands = view.rights(&account, &here).hands;
            let asker = Asker { machine: here.clone(), hub: link.membership().id.clone(), account, hands };
            askers.push((link.clone(), asker));
        }
    }
    askers
}

/// The conversation of this machine `call` reaches, if it reaches one.
fn reached<'a>(node: &Node, call: &'a Call) -> Option<&'a ResidentId> {
    let here = |machine: Option<&fastctx_types::access::Written>| machine.is_none_or(|machine| node.is_here(machine));
    match call {
        Call::Collaboration { resident, machine }
        | Call::Talk { to: resident, machine, .. }
        | Call::Message { to: resident, machine, .. }
        | Call::Watch { resident, machine, .. }
        | Call::Goal { resident, machine, .. }
        | Call::Composer { resident, machine, .. }
        | Call::Stop { resident, machine }
        | Call::Control { resident, machine, .. }
        | Call::Manage { resident, machine, .. }
        | Call::Offers { resident, machine }
        | Call::ItemDetail { resident, machine, .. }
        | Call::AgentActivity { resident, machine, .. }
            if here(machine.as_ref()) =>
        {
            Some(resident)
        }
        Call::Forget { resident, machine } if node.is_here(machine) => Some(resident),
        Call::Attach { resident }
        | Call::HoldHost { resident }
        | Call::TurnEvidence { resident, .. }
        | Call::CompactBetweenTurns { resident } => Some(resident),
        Call::Upload { target, node: machine, .. } => {
            let machine = match target {
                UploadTarget::Sent { machine, .. } => Some(machine),
                _ => machine.as_ref(),
            };
            match target.conversation_target() {
                UploadTarget::Resident { resident } if here(machine) => Some(resident),
                _ => None,
            }
        }
        Call::AttachmentThumbnail { attachment, .. } if node.is_me(&attachment.thumbnail_owner().machine) => {
            Some(&attachment.thumbnail_owner().resident)
        }
        Call::AttachmentThumbnail { attachment, .. } | Call::ReadAttachment { attachment, .. }
            if node.is_me(&attachment.owner().machine) =>
        {
            Some(&attachment.owner().resident)
        }
        _ => None,
    }
}
