//! Who may use what (`world-network.md`, 共享机器与智能体协助): an account
//! works on its own machines and on those shared with it, with every agent
//! logged in there; on another's machine it may use only the agents' logins
//! assisting it. A project is an account's, and may be shared with others.

use serde::{Deserialize, Serialize};

use crate::{AccountName, HubName, MachineName, NameError};

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

/// Which hub: chosen when the hub is made, and named in everything said
/// through it, so nothing meant for one hub is taken by another. Written
/// `w-` and sixteen hex digits, as the network's third version wrote the
/// World it became.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HubId(String);

impl HubId {
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(format!("w-{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
    }

    /// The eight bytes the id spells.
    pub fn bytes(&self) -> [u8; 8] {
        let mut bytes = [0_u8; 8];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&self.0[2 + index * 2..4 + index * 2], 16).expect("checked when made");
        }
        bytes
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HubId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let ok = text.len() == 18
            && text.starts_with("w-")
            && text[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if ok { Ok(Self(text)) } else { Err(format!("{text:?} is not a hub id")) }
    }
}

impl From<HubId> for String {
    fn from(id: HubId) -> String {
        id.0
    }
}

impl std::str::FromStr for HubId {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        Self::try_from(text.to_string())
    }
}

impl std::fmt::Display for HubId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for HubId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

/// What an account may do on a machine. Its own machine, one shared with it
/// and, for the hub's super administrator, every machine give both; a login
/// assisting it there gives `agents` alone, for that login only, which the
/// directory names (`world-network.md`, 共享机器与智能体协助).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rights {
    /// Run commands on it, read and write its files, move files to and from
    /// it, stop what runs there; and see what other accounts run there.
    /// Working on a machine comes with every agent logged in on it.
    pub hands: bool,
    /// Open sessions of agents logged in on it and talk to them: all of
    /// them with `hands`, else only the logins assisting the account.
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
    /// a machine that may act for the account (`world-network.md`,
    /// 别人机器上的智能体什么时候能访问你的机器).
    pub fn acts(self) -> bool {
        self != Purpose::Echo
    }
}

/// An account of one hub.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HubAccount {
    pub hub: HubId,
    pub account: AccountName,
}

/// Whom a resident or a job works for (`world-network.md`, 一个居民替谁干活).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Acting {
    /// Every account logged in on the computer it is on, as they are at
    /// each step: a session the user opened there, and what it runs.
    #[default]
    Here,
    /// These accounts: a session an account opened from elsewhere, a job
    /// another machine sent, a colleague.
    Accounts { accounts: Vec<HubAccount> },
}

impl Acting {
    pub fn one(hub: HubId, account: AccountName) -> Self {
        Acting::Accounts { accounts: vec![HubAccount { hub, account }] }
    }
}

/// A machine as it is written (`world-network.md`): its name, or `name.hub`
/// where two hubs a computer is on each have a different computer of that
/// name. A computer has one name on every hub it is on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Written {
    pub name: MachineName,
    pub hub: Option<HubName>,
}

impl Written {
    /// `name` or `name.hub`, read after NFKC, so a full-width `．` separates
    /// as `.` does.
    pub fn parse(text: &str) -> Result<Self, NameError> {
        let text = crate::names::nfkc(text);
        let (name, hub) = match text.split_once('.') {
            Some((name, hub)) => (name, Some(hub)),
            None => (&*text, None),
        };
        Ok(Self { name: MachineName::try_from(name)?, hub: hub.map(HubName::try_from).transpose()? })
    }
}

impl From<&MachineName> for Written {
    fn from(name: &MachineName) -> Self {
        Self { name: name.clone(), hub: None }
    }
}

impl From<MachineName> for Written {
    fn from(name: MachineName) -> Self {
        Self { name, hub: None }
    }
}

impl TryFrom<String> for Written {
    type Error = NameError;

    fn try_from(text: String) -> Result<Self, NameError> {
        Self::parse(&text)
    }
}

impl std::str::FromStr for Written {
    type Err = NameError;

    fn from_str(text: &str) -> Result<Self, NameError> {
        Self::parse(text)
    }
}

impl From<Written> for String {
    fn from(written: Written) -> String {
        written.to_string()
    }
}

impl std::fmt::Display for Written {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.hub {
            Some(hub) => write!(f, "{}.{hub}", self.name),
            None => self.name.fmt(f),
        }
    }
}
