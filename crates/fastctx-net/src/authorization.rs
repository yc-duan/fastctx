//! The account policy shared by the hub, nodes and device clients.

use std::path::Path;

use fastctx_types::access::{Purpose, Rights, TownId};
use fastctx_types::authorization::{Denial, DenialReason};
use fastctx_types::{AccountName, AgentName, MachineName};

use crate::directory::{Directory, Member};

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
        use DenialReason as Why;
        match self.account(account) {
            None => return Err(Denial::new(Why::AccountMissing, format!("there is no account {account}"))),
            Some(found) if found.locked => {
                return Err(Denial::new(Why::AccountLocked, format!("the account {account} is locked")));
            }
            Some(_) => {}
        }
        match action {
            Action::Project(town) => {
                let project = self
                    .project(town)
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
                let member = self.member(machine).ok_or_else(|| missing(machine))?;
                let allowed = !member.managed
                    && (member.logins.contains(account)
                        || (self
                            .acting
                            .iter()
                            .any(|acting| acting.account == *account && acting.machine == member.name)
                            && self.machine_rights(account, &member.name).agents));
                if allowed {
                    Ok(())
                } else {
                    Err(Denial::new(Why::CannotAct, format!("{machine} may not act for the account {account}")))
                }
            }
            Action::Call { from, to, purpose } => {
                if purpose.acts() {
                    self.authorize(account, Action::ActFor(from))?;
                } else if !(self.logged_in(from, account) || self.machine_rights(account, from).any()) {
                    return Err(Denial::new(
                        Why::CannotAct,
                        format!("{from} is not a machine the account {account} uses"),
                    ));
                }
                match purpose {
                    Purpose::Hands => self.authorize(account, Action::Work(to)),
                    Purpose::Agents => self.authorize(account, Action::Login { machine: to, agent: None, home: None }),
                    Purpose::Look => self.authorize(account, Action::Look(to)),
                    Purpose::Words | Purpose::Echo => {
                        if self.logged_in(to, account) || self.machine_rights(account, to).agents {
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
                let member = self.machine(machine)?;
                if member.home.as_ref().is_some_and(|owner| {
                    owner == account || (!self.is_visitor(owner) && self.shared(account, &member.name))
                }) {
                    Ok(())
                } else {
                    Err(Denial::new(Why::MachineNotOwned, fastctx_types::sharing::HOLDER_REQUIRED))
                }
            }
            Action::Work(machine) | Action::Manage(machine) | Action::Look(machine) => {
                self.machine(machine)?;
                let rights = self.machine_rights(account, machine);
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
                let member = self.machine(machine)?;
                if member.managed {
                    return Err(Denial::new(
                        Why::ManagedMachine,
                        format!("{machine} is a managed machine, whose agents work for no one"),
                    ));
                }
                let rights = self.machine_rights(account, machine);
                let allowed = rights.hands
                    || (rights.agents
                        && agent
                            .is_none_or(|agent| self.assisting(account, machine).any(|assist| assist.is(agent, home))));
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

    fn machine(&self, name: &MachineName) -> Result<&Member, Denial> {
        let member = self.member(name).ok_or_else(|| missing(name))?;
        if member.is_machine() {
            Ok(member)
        } else {
            Err(Denial::new(
                DenialReason::NotMachine,
                format!("{name} is logged in to this hub but not registered as a machine, so no account can use it"),
            ))
        }
    }

    /// Stored grants without the account's temporary lock, for retaining delegation across a lock.
    pub(crate) fn machine_rights(&self, account: &AccountName, machine: &MachineName) -> Rights {
        let Some(member) = self.member(machine) else { return Rights::NONE };
        let Some(owner) = member.home.as_ref() else { return Rights::NONE };
        let visitors = self.is_visitor(owner);
        let whole = owner == account || (!visitors && (self.is_super(account) || self.shared(account, &member.name)));
        if member.managed {
            return Rights { hands: whole, agents: false };
        }
        if whole {
            return Rights::ALL;
        }
        Rights { hands: false, agents: !visitors && self.assisting(account, &member.name).next().is_some() }
    }
}

fn missing(machine: &MachineName) -> Denial {
    Denial::new(DenialReason::MachineMissing, format!("there is no machine named {machine}"))
}
