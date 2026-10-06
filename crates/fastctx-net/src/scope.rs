//! What each member may do: its scope, and the grants that set it.
//!
//! A few machines the user trusts most hold management rights; the founder
//! is born with them. Only their grants count: a grant names a member by
//! name and key and says what it may use (which machines, which agents on
//! them, which projects), which of the World's actions it may take there,
//! whether it may invite others, and whether it manages too. A member that
//! has never been granted anything keeps the scope its invitation gave it,
//! or, for the members of a World older than scopes, everything.
//!
//! Every machine works out every member's scope for itself, from the signed
//! records the hub hands out (`crate::trust`), and refuses at its own door
//! whatever a caller's scope does not cover.

use std::collections::BTreeSet;
use std::fmt;

use fastctx_types::{AgentName, MachineName};
use jiff::Timestamp;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ledger::TownId;
use crate::sign::PublicKey;
use crate::trust::WorldId;

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

/// What one member may use and do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub machines: Set<MachineName>,
    /// The agents it may reach on those machines, by program.
    pub agents: Set<AgentName>,
    pub projects: Set<TownId>,
    pub actions: Set<Action>,
    /// Whether it may invite members of its own, within its own scope.
    #[serde(default)]
    pub invite: bool,
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

/// A management machine's word on what `member` may do, signed by it.
/// Each grant for a member carries the next version; a machine takes the
/// latest one it can trust, in the order the grants were made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub world: WorldId,
    pub member: MachineName,
    /// The member's key, so a grant never carries over to another machine
    /// that took the name later.
    pub key: PublicKey,
    pub scope: Scope,
    /// Whether the member holds management rights from now on.
    pub manage: bool,
    pub version: u64,
    pub by: MachineName,
    pub at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_read_and_combine_as_written() {
        let only: Set<MachineName> = serde_json::from_str(r#"["desktop","laptop"]"#).unwrap();
        let all: Set<MachineName> = serde_json::from_str(r#""all""#).unwrap();
        assert!(all.is_all());
        assert_eq!(serde_json::to_string(&only).unwrap(), r#"["desktop","laptop"]"#);
        let one: Set<MachineName> = serde_json::from_str(r#"["laptop"]"#).unwrap();
        assert!(one.within(&only) && only.within(&all) && !all.within(&only));
        assert_eq!(only.intersect(&all), only);
        assert_eq!(only.intersect(&one), one);
        assert!(serde_json::from_str::<Set<MachineName>>(r#""some""#).is_err());
    }
}
