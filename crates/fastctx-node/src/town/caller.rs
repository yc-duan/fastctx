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
    /// An agent's session the World has no resident for.
    Session(AgentName),
}

/// Who is on the other end of a command's connection.
pub fn of(node: &Node, peer: &Peer) -> Result<Caller, Failure> {
    let here = node.machine();
    let resident = |id| Caller::Resident(Address { machine: here.clone(), resident: id });
    match peer {
        Peer::Page { page } => Ok(Caller::Page(page.clone())),
        Peer::Command { resident: Some(address), .. } => Ok(Caller::Resident(address.clone())),
        Peer::Command { resident: None, session: Some(SessionHint::Codex { thread }) } => {
            Ok(node.residents.resident_of_thread(thread).map(resident).unwrap_or_else(|| agent("codex")))
        }
        Peer::Command { resident: None, session: Some(SessionHint::Claude { inbox }) } => {
            Ok(node.residents.resident_of_inbox(inbox).map(resident).unwrap_or_else(|| agent("claude")))
        }
        Peer::Command { resident: None, session: None } => Ok(Caller::User),
        Peer::Session { .. } | Peer::Supervisor { .. } => Err(Failure::new("this is asked of the node by a command")),
    }
}

fn agent(name: &str) -> Caller {
    Caller::Session(AgentName::try_from(name).expect("a valid agent name"))
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
            return Ok(town);
        }
        Err(Failure::new(match self {
            Caller::Resident(address) if !node.is_me(&address.machine) => format!(
                "name the project: this session's project is known on {}, where it runs; `fastctx project list` lists the projects",
                address.machine
            ),
            Caller::Resident(_) => {
                "this session is not in a project yet; name one (`fastctx project list` lists them)".to_string()
            }
            _ => "name the project (`fastctx project list` lists them)".to_string(),
        }))
    }
}
