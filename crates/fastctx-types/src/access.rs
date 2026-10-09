//! Who may use what (`world-network.md`, 一台机器给几个账号用): an account
//! uses a machine in two ways, working on it and using the agents logged in
//! on it; a project is an account's, and may be granted to others.

use serde::{Deserialize, Serialize};

/// A project as the hub and the wire know it, chosen at random when the
/// project is opened. Its short name is the town's (`TownName`); the id is
/// what the hub keeps its ledger under.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TownId(String);

impl TownId {
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(format!("t-{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TownId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let ok = text.len() == 18
            && text.starts_with("t-")
            && text[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if ok { Ok(Self(text)) } else { Err(format!("{text:?} is not a project id")) }
    }
}

impl From<TownId> for String {
    fn from(id: TownId) -> String {
        id.0
    }
}

impl std::fmt::Display for TownId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for TownId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

/// What an account may do on a machine: the two switches of a grant. The
/// account a machine belongs to has both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rights {
    /// Run commands on it, read and write its files, move files to and from
    /// it, stop what runs there; and see what other accounts run there.
    pub hands: bool,
    /// Open sessions of the agents logged in on it and talk to them.
    pub agents: bool,
}

impl Rights {
    pub const ALL: Rights = Rights { hands: true, agents: true };
    pub const NONE: Rights = Rights { hands: false, agents: false };

    pub fn allows(&self, use_: Use) -> bool {
        match use_ {
            Use::Hands => self.hands,
            Use::Agents => self.agents,
        }
    }

    pub fn any(&self) -> bool {
        self.hands || self.agents
    }

    pub fn union(self, other: Rights) -> Rights {
        Rights { hands: self.hands || other.hands, agents: self.agents || other.agents }
    }
}

/// One of the two ways to use a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Use {
    Hands,
    Agents,
}

impl std::fmt::Display for Use {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Use::Hands => "work on it",
            Use::Agents => "use its agents",
        })
    }
}

/// What a call or a letter from one machine asks of the machine it goes to,
/// for an account: the standing it needs there. The sender names it and the
/// hub checks it; the receiver works it out from what it is asked and checks
/// that, so a sender naming less than it asks for gains nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// Work there: run commands, read and write files, move files, stop jobs.
    Hands,
    /// Use the agents there: open sessions and talk to them.
    Agents,
    /// Ask what is there; the answer holds what the account may see.
    Look,
    /// Words for a resident there that works for the account.
    Words,
    /// News of what happened on the sending machine, for a resident there
    /// that works for the account and reached out to the sender: a job that
    /// ended, a colleague's answer. It is not the sender acting for anyone.
    Echo,
}

impl Purpose {
    /// Whether the sender acts for the account in saying it, and so must be
    /// a machine the account trusts (`world-network.md`, 什么算出手).
    pub fn acts(self) -> bool {
        self != Purpose::Echo
    }
}
