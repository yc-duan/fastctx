//! The account policy shared by the hub, nodes and device clients.
//!
//! The policy is written once, over `Lookup`: a whole directory answers its
//! questions by a scan (`Directory`), one indexed for many questions by key
//! (`crate::directory_index::Indexed`). The decisions are the same either way.

use std::path::Path;

use fastctx_types::access::{Purpose, Rights, TownId};
use fastctx_types::authorization::{Denial, DenialReason};
use fastctx_types::{AccountName, AgentName, MachineName};
use jiff::Timestamp;

use crate::directory::{Account, Assist, Directory, Level, Member, Project};

/// An account's action. `Call` composes source delegation with the target action.
#[derive(Clone, Copy, Debug)]
pub enum Action<'a> {
    Work(&'a MachineName),
    /// `agent == None` asks whether any login is available, for discovery and legacy calls.
    Login {
        machine: &'a MachineName,
        agent: Option<&'a AgentName>,
        home: Option<&'a Path>,
    },
    Project(&'a TownId),
    Look(&'a MachineName),
    Manage(&'a MachineName),
    /// Ownership or a whole-machine share, without the administrator's implicit reach.
    Hold(&'a MachineName),
    ActFor(&'a MachineName),
    Call {
        from: &'a MachineName,
        to: &'a MachineName,
        purpose: Purpose,
    },
}

/// What the policy asks of a directory.
pub trait Lookup {
    /// The account named `name`.
    fn find_account(&self, name: &AccountName) -> Option<&Account>;
    /// The member `name` names, by its name now or one it had.
    fn find_member(&self, name: &MachineName) -> Option<&Member>;
    fn find_project(&self, town: &TownId) -> Option<&Project>;
    /// Whether `machine`, by the name it has now, is shared with `account`.
    fn is_shared(&self, account: &AccountName, machine: &MachineName) -> bool;
    /// Whether an assistance for `account` on `machine`, by the name it has
    /// now, still assists at `now` and answers `which`.
    fn assists(
        &self,
        account: &AccountName,
        machine: &MachineName,
        now: Timestamp,
        which: &dyn Fn(&Assist) -> bool,
    ) -> bool;
    /// Whether `machine`, by the name it has now, acts for `account`
    /// because the account used an agent there.
    fn acts(&self, account: &AccountName, machine: &MachineName) -> bool;
}

impl Lookup for Directory {
    fn find_account(&self, name: &AccountName) -> Option<&Account> {
        self.account(name)
    }

    fn find_member(&self, name: &MachineName) -> Option<&Member> {
        self.member(name)
    }

    fn find_project(&self, town: &TownId) -> Option<&Project> {
        self.project(town)
    }

    fn is_shared(&self, account: &AccountName, machine: &MachineName) -> bool {
        self.shares.iter().any(|share| share.account == *account && share.machine == *machine)
    }

    fn assists(
        &self,
        account: &AccountName,
        machine: &MachineName,
        now: Timestamp,
        which: &dyn Fn(&Assist) -> bool,
    ) -> bool {
        self.assists
            .iter()
            .any(|assist| assist.account == *account && assist.machine == *machine && assist.live(now) && which(assist))
    }

    fn acts(&self, account: &AccountName, machine: &MachineName) -> bool {
        self.acting.iter().any(|acting| acting.account == *account && acting.machine == *machine)
    }
}

impl Directory {
    /// Effective machine rights for calls from this source, without combining different accounts.
    pub fn call_rights(&self, from: &MachineName, account: &AccountName, to: &MachineName) -> Rights {
        Rights {
            hands: self.authorize(account, Action::Call { from, to, purpose: Purpose::Hands }).is_ok(),
            agents: self.authorize(account, Action::Call { from, to, purpose: Purpose::Agents }).is_ok(),
        }
    }

    /// Decides an action using one account and this directory's current policy.
    /// Missing and locked accounts are refused before considering ownership or delegation.
    pub fn authorize(&self, account: &AccountName, action: Action<'_>) -> Result<(), Denial> {
        authorize(self, account, action)
    }

    /// Stored grants without the account's temporary lock, for retaining delegation across a lock.
    pub(crate) fn machine_rights(&self, account: &AccountName, machine: &MachineName) -> Rights {
        machine_rights(self, account, machine)
    }
}

/// Decides an action using one account and the policy, as `lookup` finds
/// what it asks about.
pub fn authorize<L: Lookup + ?Sized>(lookup: &L, account: &AccountName, action: Action<'_>) -> Result<(), Denial> {
    use DenialReason as Why;
    match lookup.find_account(account) {
        None => return Err(Denial::new(Why::AccountMissing, format!("there is no account {account}"))),
        Some(found) if found.locked => {
            return Err(Denial::new(Why::AccountLocked, format!("the account {account} is locked")));
        }
        Some(_) => {}
    }
    match action {
        Action::Project(town) => {
            let project = lookup
                .find_project(town)
                .ok_or_else(|| Denial::new(Why::ProjectMissing, format!("there is no project {town}")))?;
            if project.owner == *account || project.granted.contains(account) {
                Ok(())
            } else {
                Err(Denial::new(
                    Why::ProjectNotShared,
                    format!("the account {account} may not use the project {}", project.name),
                ))
            }
        }
        Action::ActFor(machine) => {
            let member = lookup.find_member(machine).ok_or_else(|| missing(machine))?;
            let allowed = !member.managed
                && (member.logins.contains(account)
                    || (lookup.acts(account, &member.name) && machine_rights(lookup, account, &member.name).agents));
            if allowed {
                Ok(())
            } else {
                Err(Denial::new(Why::CannotAct, format!("{machine} may not act for the account {account}")))
            }
        }
        Action::Call { from, to, purpose } => {
            if purpose.acts() {
                authorize(lookup, account, Action::ActFor(from))?;
            } else if !(logged_in(lookup, from, account) || machine_rights(lookup, account, from).any()) {
                return Err(Denial::new(Why::CannotAct, format!("{from} is not a machine the account {account} uses")));
            }
            match purpose {
                Purpose::Hands => authorize(lookup, account, Action::Work(to)),
                Purpose::Agents => authorize(lookup, account, Action::Login { machine: to, agent: None, home: None }),
                Purpose::Look => authorize(lookup, account, Action::Look(to)),
                Purpose::Words | Purpose::Echo => {
                    if logged_in(lookup, to, account) || machine_rights(lookup, account, to).agents {
                        Ok(())
                    } else {
                        Err(Denial::new(
                            Why::LoginNotAssisting,
                            format!("no resident of the account {account} can be on {to}"),
                        ))
                    }
                }
            }
        }
        Action::Hold(machine) => {
            let member = registered(lookup, machine)?;
            if member.home.as_ref().is_some_and(|owner| {
                owner == account || (!is_visitor(lookup, owner) && lookup.is_shared(account, &member.name))
            }) {
                Ok(())
            } else {
                Err(Denial::new(Why::MachineNotOwned, fastctx_types::sharing::HOLDER_REQUIRED))
            }
        }
        Action::Work(machine) | Action::Manage(machine) | Action::Look(machine) => {
            registered(lookup, machine)?;
            let rights = machine_rights(lookup, account, machine);
            let work = matches!(action, Action::Work(_) | Action::Manage(_));
            if if work { rights.hands } else { rights.any() } {
                return Ok(());
            }
            let message = if work && rights.agents {
                format!(
                    "an agent on {machine} assists the account {account}, which is not given the machine, so it may not work on {machine}"
                )
            } else if work {
                format!("the account {account} may not work on {machine}")
            } else {
                format!("the account {account} may not use {machine}")
            };
            Err(Denial::new(Why::MachineNotShared, message))
        }
        Action::Login { machine, agent, home } => {
            let member = registered(lookup, machine)?;
            if member.managed {
                return Err(Denial::new(
                    Why::ManagedMachine,
                    format!("{machine} is a managed machine, whose agents work for no one"),
                ));
            }
            let rights = machine_rights(lookup, account, machine);
            let allowed = rights.hands
                || (rights.agents
                    && agent.is_none_or(|agent| {
                        lookup.assists(account, &member.name, Timestamp::now(), &|assist| assist.is(agent, home))
                    }));
            if allowed {
                Ok(())
            } else {
                let message = match agent {
                    Some(agent) => format!(
                        "the account {account} may not use the {agent} login {} on {machine}",
                        home.map_or_else(|| "(default)".into(), |home| home.display().to_string())
                    ),
                    None => format!("the account {account} may not use the agents on {machine}"),
                };
                Err(Denial::new(Why::LoginNotAssisting, message))
            }
        }
    }
}

/// What `account` may do on `machine`, as `Directory::rights` says it.
pub fn rights<L: Lookup + ?Sized>(lookup: &L, account: &AccountName, machine: &MachineName) -> Rights {
    Rights {
        hands: authorize(lookup, account, Action::Work(machine)).is_ok(),
        agents: authorize(lookup, account, Action::Login { machine, agent: None, home: None }).is_ok(),
    }
}

/// Stored grants without the account's temporary lock, for retaining delegation across a lock.
pub(crate) fn machine_rights<L: Lookup + ?Sized>(lookup: &L, account: &AccountName, machine: &MachineName) -> Rights {
    let Some(member) = lookup.find_member(machine) else { return Rights::NONE };
    let Some(owner) = member.home.as_ref() else { return Rights::NONE };
    let visitors = is_visitor(lookup, owner);
    let whole =
        owner == account || (!visitors && (is_super(lookup, account) || lookup.is_shared(account, &member.name)));
    if member.managed {
        return Rights { hands: whole, agents: false };
    }
    if whole {
        return Rights::ALL;
    }
    Rights { hands: false, agents: !visitors && lookup.assists(account, &member.name, Timestamp::now(), &|_| true) }
}

fn registered<'l, L: Lookup + ?Sized>(lookup: &'l L, name: &MachineName) -> Result<&'l Member, Denial> {
    let member = lookup.find_member(name).ok_or_else(|| missing(name))?;
    if member.is_machine() {
        Ok(member)
    } else {
        Err(Denial::new(
            DenialReason::NotMachine,
            format!("{name} is logged in to this hub but not registered as a machine, so no account can use it"),
        ))
    }
}

fn is_super<L: Lookup + ?Sized>(lookup: &L, account: &AccountName) -> bool {
    lookup.find_account(account).is_some_and(Account::is_super)
}

fn is_visitor<L: Lookup + ?Sized>(lookup: &L, account: &AccountName) -> bool {
    lookup.find_account(account).is_some_and(|found| found.level == Level::Visitor)
}

fn logged_in<L: Lookup + ?Sized>(lookup: &L, member: &MachineName, account: &AccountName) -> bool {
    lookup.find_member(member).is_some_and(|member| member.logins.contains(account))
}

fn missing(machine: &MachineName) -> Denial {
    Denial::new(DenialReason::MachineMissing, format!("there is no machine named {machine}"))
}
