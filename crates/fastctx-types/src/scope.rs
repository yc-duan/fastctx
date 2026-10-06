//! What a member of a World may do: its scope (`world-network.md`, 管理权与
//! 范围). A scope names the machines a member may use, the agents it may
//! reach on them by program, the projects it may use, the actions it may
//! take there, and whether it may invite members of its own. The grants
//! that set scopes, and the checking of them, are the network's
//! (`fastctx_net::scope`, `fastctx_net::trust`).

use std::collections::BTreeSet;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{AgentName, MachineName};

/// A town as the hub and the wire know it: a name without meaning, so
/// nothing on the way learns what a project is called.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TownId(String);

impl TownId {
    /// The World's own ledger, which every member reads.
    pub fn world() -> Self {
        Self("t-0000000000000000".to_string())
    }

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
        if ok { Ok(Self(text)) } else { Err(format!("{text:?} is not a town id")) }
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

/// Everything, or only these.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Set<T: Ord> {
    All,
    Only(BTreeSet<T>),
}

impl<T: Ord + Clone> Set<T> {
    pub fn contains(&self, item: &T) -> bool {
        match self {
            Set::All => true,
            Set::Only(items) => items.contains(item),
        }
    }

    pub fn intersect(&self, other: &Self) -> Self {
        match (self, other) {
            (Set::All, other) | (other, Set::All) => other.clone(),
            (Set::Only(a), Set::Only(b)) => Set::Only(a.intersection(b).cloned().collect()),
        }
    }

    /// Whether everything in `self` is in `other`.
    pub fn within(&self, other: &Self) -> bool {
        match (self, other) {
            (_, Set::All) => true,
            (Set::All, Set::Only(_)) => false,
            (Set::Only(a), Set::Only(b)) => a.is_subset(b),
        }
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Set::All)
    }
}

impl<T: Ord + fmt::Debug> fmt::Debug for Set<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Set::All => f.write_str("all"),
            Set::Only(items) => f.debug_set().entries(items).finish(),
        }
    }
}

impl<T: Ord + Serialize> Serialize for Set<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Set::All => serializer.serialize_str("all"),
            Set::Only(items) => items.serialize(serializer),
        }
    }
}

impl<'de, T: Ord + DeserializeOwned> Deserialize<'de> for Set<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either<T: Ord> {
            Word(String),
            Items(BTreeSet<T>),
        }
        match Either::<T>::deserialize(deserializer)? {
            Either::Word(word) if word == "all" => Ok(Set::All),
            Either::Word(word) => Err(serde::de::Error::custom(format!("{word:?} is not \"all\" or a list"))),
            Either::Items(items) => Ok(Set::Only(items)),
        }
    }
}

/// What a member does on another machine, named after the hand a resident
/// does it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Run commands there.
    Run,
    /// Write files there: `edit`, `write` and `apply_patch`.
    Write,
    /// Copy files to or from there.
    Copy,
    /// Ask a colleague to work there.
    AskColleague,
    /// Ask who is there and what each is doing, and what happened since.
    W,
    /// Talk with an agent there and watch it, and manage its conversation.
    Talk,
    /// Read files there.
    Read,
    /// Give the user's consent to Claude Code's bypass-permissions mode there.
    Consent,
    /// Do what a page asks there, once the user agreed to it.
    Page,
}

impl Action {
    pub const ALL: [Action; 9] = [
        Action::Run,
        Action::Write,
        Action::Copy,
        Action::AskColleague,
        Action::W,
        Action::Talk,
        Action::Read,
        Action::Consent,
        Action::Page,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Action::Run => "run",
            Action::Write => "write",
            Action::Copy => "copy",
            Action::AskColleague => "ask_colleague",
            Action::W => "w",
            Action::Talk => "talk",
            Action::Read => "read",
            Action::Consent => "consent",
            Action::Page => "page",
        }
    }
}

/// What one member may use and do. Between machines a project is named by
/// its id (`TownId`); a person and the interface name it by its short name,
/// and the node puts one for the other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(serialize = "P: Ord + Serialize", deserialize = "P: Ord + DeserializeOwned"))]
pub struct Scope<P: Ord = TownId> {
    pub machines: Set<MachineName>,
    /// The agents it may reach on those machines, by program.
    pub agents: Set<AgentName>,
    pub projects: Set<P>,
    pub actions: Set<Action>,
    /// Whether it may invite members of its own, within its own scope.
    #[serde(default)]
    pub invite: bool,
}

/// A scope as a person names it: projects by their short names.
pub type NamedScope = Scope<crate::TownName>;

impl<P: Ord + Clone> Scope<P> {
    /// The same scope, with each project named by `name`; `None` when one
    /// has no name there.
    pub fn named<Q: Ord>(&self, mut name: impl FnMut(&P) -> Option<Q>) -> Option<Scope<Q>> {
        let projects = match &self.projects {
            Set::All => Set::All,
            Set::Only(projects) => Set::Only(projects.iter().map(&mut name).collect::<Option<BTreeSet<Q>>>()?),
        };
        Some(Scope {
            machines: self.machines.clone(),
            agents: self.agents.clone(),
            projects,
            actions: self.actions.clone(),
            invite: self.invite,
        })
    }
}

impl Scope {
    /// Everything, inviting included: the scope of a member nobody limited.
    pub fn all() -> Self {
        Self { machines: Set::All, agents: Set::All, projects: Set::All, actions: Set::All, invite: true }
    }

    /// What both allow.
    pub fn intersect(&self, other: &Scope) -> Scope {
        Scope {
            machines: self.machines.intersect(&other.machines),
            agents: self.agents.intersect(&other.agents),
            projects: self.projects.intersect(&other.projects),
            actions: self.actions.intersect(&other.actions),
            invite: self.invite && other.invite,
        }
    }

    /// Whether `self` gives nothing `other` does not.
    pub fn within(&self, other: &Scope) -> bool {
        self.machines.within(&other.machines)
            && self.agents.within(&other.agents)
            && self.projects.within(&other.projects)
            && self.actions.within(&other.actions)
            && (!self.invite || other.invite)
    }

    /// Whether it may take `action` on `machine`, and when the action
    /// reaches an agent, on an agent of the program `agent`.
    pub fn allows(&self, machine: &MachineName, action: Action, agent: Option<&AgentName>) -> Result<(), Denied> {
        if !self.machines.contains(machine) {
            return Err(Denied::Machine(machine.clone()));
        }
        if !self.actions.contains(&action) {
            return Err(Denied::Action(action));
        }
        if let Some(agent) = agent
            && !self.agents.contains(agent)
        {
            return Err(Denied::Agent(agent.clone()));
        }
        Ok(())
    }
}

/// What a scope does not cover.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Denied {
    #[error("its scope does not include the machine {0}")]
    Machine(MachineName),
    #[error("its scope does not include `{}`", .0.name())]
    Action(Action),
    #[error("its scope does not include the agents of {0}")]
    Agent(AgentName),
    #[error("its scope does not include that project")]
    Project,
}
