//! Who runs a command, as far as the World can tell (`world.md`, 身份照实):
//! a resident the World's `run` started it for; an agent's session whose
//! own shell started it, as its host says in the command's environment; or,
//! with neither, the user at a terminal.

use fastctx_types::ipc::{Failure, Peer, SessionHint};
use fastctx_types::project::Writer;
use fastctx_types::{Address, AgentName, TownName};

use crate::Node;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    Page(Box<fastctx_types::pages::PageSource>),
    User,
    /// A resident of this machine or another.
    Resident(Address),
    /// An unregistered host, retained for legacy attribution but never admitted as an account.
    #[expect(
        dead_code,
        reason = "Legacy attribution remains representable; unknown callers are refused before construction."
    )]
    Session(AgentName),
}

/// Who is on the other end of a command's connection.
pub fn of(node: &Node, peer: &Peer) -> Result<Caller, Failure> {
    let here = node.machine();
    let resident = |id| Caller::Resident(Address { machine: here.clone(), resident: id });
    match peer {
        Peer::Page { page, job } => {
            crate::pages::identity::job(node, page, job.as_ref())?;
            crate::pages::identity::acting(node, page)?;
            Ok(Caller::Page(page.clone()))
        }
        Peer::Command { resident: Some(address), .. } => {
            if node.is_me(&address.machine) {
                node.residents.active(&address.resident)?;
            } else if crate::jobs::running_for(&node.world, &node.names(), address).is_empty() {
                return Err(unrecognized("the command belongs to no active remote job"));
            }
            Ok(Caller::Resident(address.clone()))
        }
        Peer::Command { resident: None, session: Some(SessionHint::Codex { thread }) } => node
            .residents
            .resident_of_thread(thread)
            .map(resident)
            .ok_or_else(|| unrecognized("the Codex thread is not registered")),
        Peer::Command { resident: None, session: Some(SessionHint::Claude { inbox }) } => node
            .residents
            .resident_of_inbox(inbox)
            .map(resident)
            .ok_or_else(|| unrecognized("the Claude session is not registered")),
        Peer::Command { resident: None, session: None } => Ok(Caller::User),
        Peer::Session { .. } | Peer::Supervisor { .. } => Err(Failure::new("this is asked of the node by a command")),
    }
}

fn unrecognized(message: &str) -> Failure {
    fastctx_types::authorization::Denial::new(fastctx_types::authorization::DenialReason::NoSubject, message).into()
}

impl Caller {
    pub fn writer(&self) -> Writer {
        match self {
            Caller::Page(page) => Writer::Page { page: page.clone() },
            Caller::User => Writer::User,
            Caller::Resident(address) => Writer::Resident { address: address.clone() },
            Caller::Session(agent) => Writer::Session { agent: agent.clone() },
        }
    }

    /// The town the caller lives in, when it is a resident of this machine
    /// that lives in one.
    pub fn town(&self, node: &Node) -> Option<TownName> {
        match self {
            Caller::Page(page) => Some(page.town.clone()),
            Caller::Resident(address) if node.is_me(&address.machine) => {
                node.residents.town_of(&address.resident).flatten()
            }
            _ => None,
        }
    }

    /// `given`, or else the town the caller lives in.
    pub fn town_or(&self, node: &Node, given: Option<TownName>) -> Result<TownName, Failure> {
        if let Some(town) = given.or_else(|| self.town(node)) {
            super::access::check(node, &crate::around::acting_of(node, self), &town)?;
            return Ok(town);
        }
        Err(Failure::new(match self {
            // The machine the session lives on is not named: for an agent
            // assisting another account it is its owner's, which that
            // account is not told of.
            Caller::Resident(address) if !node.is_me(&address.machine) => {
                "name the project: this machine does not know which one this session is in; `fastctx project list` lists the projects".to_string()
            }
            Caller::Resident(_) => {
                "this session is not in a project yet; name one (`fastctx project list` lists them)".to_string()
            }
            _ => "name the project (`fastctx project list` lists them)".to_string(),
        }))
    }
}
