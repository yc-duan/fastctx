//! The World's hands as MCP tools (`specs/world-run.md`, `world-write.md`,
//! `world-colleagues.md`): the surface an enrolled machine publishes, the hands behind it, and
//! the words models read about them.

pub(crate) mod colleague;
pub(crate) mod run;
pub(crate) mod surface;
pub(crate) mod text;
pub(crate) mod write;

use crate::town::remote::Peer;

/// Whom a hand acts for on this machine.
pub(crate) enum Starter {
    /// A resident living on this machine.
    Here(String),
    /// A resident living on another machine, in the town it named. What it does here is
    /// recorded here under its name; its own machine keeps its actions.
    Guest { peer: Peer, town: Option<String> },
}

impl Starter {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Here(id) => id,
            Self::Guest { peer, .. } => &peer.id,
        }
    }
}
