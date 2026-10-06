//! A member's scope as given on the command line: each limit names what it
//! allows, and a limit not given allows everything of its kind.

use clap::Args;
use fastctx_types::scope::{Action, NamedScope, Set};
use fastctx_types::{AgentName, MachineName, TownName};

#[derive(Args, Default)]
pub struct ScopeArgs {
    /// Only these machines, comma-separated.
    #[arg(long, value_delimiter = ',')]
    machines: Vec<MachineName>,
    /// Only the agents of these programs, such as codex or claude.
    #[arg(long, value_delimiter = ',')]
    agents: Vec<AgentName>,
    /// Only these projects, by their short names.
    #[arg(long, value_delimiter = ',')]
    projects: Vec<TownName>,
    /// Only these actions: run, write, copy, ask_colleague, w, talk, read,
    /// consent, page.
    #[arg(long, value_delimiter = ',', value_parser = action)]
    actions: Vec<Action>,
    /// May invite members of its own, within its own scope.
    #[arg(long)]
    invite: bool,
}

fn action(text: &str) -> Result<Action, String> {
    Action::ALL.into_iter().find(|action| action.name() == text).ok_or_else(|| format!("{text:?} is not an action"))
}

impl ScopeArgs {
    /// The scope, or `None` when nothing was limited and inviting not given.
    pub fn scope(self) -> Option<NamedScope> {
        let limited = !(self.machines.is_empty()
            && self.agents.is_empty()
            && self.projects.is_empty()
            && self.actions.is_empty());
        if !limited && !self.invite {
            return None;
        }
        fn set<T: Ord>(items: Vec<T>) -> Set<T> {
            if items.is_empty() { Set::All } else { Set::Only(items.into_iter().collect()) }
        }
        Some(NamedScope {
            machines: set(self.machines),
            agents: set(self.agents),
            projects: set(self.projects),
            actions: set(self.actions),
            invite: self.invite,
        })
    }

    /// The scope, everything when nothing was limited.
    pub fn scope_or_all(self) -> NamedScope {
        let invite = self.invite;
        self.scope().unwrap_or(NamedScope {
            machines: Set::All,
            agents: Set::All,
            projects: Set::All,
            actions: Set::All,
            invite,
        })
    }
}
