//! Who is on a hub and what each may do (`world-network.md`).
//!
//! The hub is where this is decided: accounts, the computers and phones
//! logged in to them, the machines registered under them, the grants and
//! trust between accounts and machines, and whose each project is. Every
//! member is given its own view of it (`View`), cut to what concerns it,
//! and checks what reaches it against that view before acting: a second
//! look that guards against the hub's mistakes, not against the hub.
//!
//! The judgments are here, on `Directory`, so the hub, which holds all of
//! it, and every member, which holds its cut, make them the same way.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use fastctx_types::access::{Purpose, Rights, TownId, Use};
use fastctx_types::{AccountName, HubName, MachineName, TownName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ids::HubId;
use crate::sign::PublicKey;

/// What a member is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A computer: a node runs on it, and it may be registered as a machine.
    #[default]
    Computer,
    /// A phone: it carries its user's words and looks; nothing runs on it.
    Phone,
    /// A web AI's connection through the hub's endpoint.
    Web,
}

impl Kind {
    pub fn is_computer(self) -> bool {
        self == Kind::Computer
    }
}

/// A relay a machine may connect through: a port forwarded to the hub, so
/// the hub's own certificate is what a machine checks there too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relay {
    /// What the user called it, such as the machine it runs on.
    pub label: String,
    pub addresses: Vec<SocketAddr>,
}

/// A computer, phone or web AI on the hub, under a name no other member
/// on the hub has.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub name: MachineName,
    pub key: PublicKey,
    pub kind: Kind,
    /// The accounts logged in on it.
    pub logins: BTreeSet<AccountName>,
    /// The account it is registered as a machine under; that account is
    /// among its logins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<AccountName>,
    /// The relays it connects through, in order; the hub comes after them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<Relay>,
    pub added: Timestamp,
    /// Whether it is connected now, as the hub saw when it made the view.
    #[serde(default)]
    pub online: bool,
    /// When it connected, when online; when it was last connected, when not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Timestamp>,
    /// The relay it is connected through, when online through one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<String>,
}

impl Member {
    pub fn is_machine(&self) -> bool {
        self.home.is_some()
    }
}

/// What an account may do on a machine that is not its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub machine: MachineName,
    pub account: AccountName,
    pub rights: Rights,
}

/// An account letting a machine act for it: what runs there for it may
/// reach its other machines.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Trust {
    pub account: AccountName,
    pub machine: MachineName,
}

/// A project: the account it was opened in, and the accounts it was
/// granted to, which may read and write its records too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub town: TownId,
    pub name: TownName,
    pub owner: AccountName,
    #[serde(default)]
    pub granted: BTreeSet<AccountName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub name: AccountName,
    pub admin: bool,
    /// Whether computers logging in to it may register as its machines.
    pub may_register: bool,
    pub locked: bool,
    pub opened: Timestamp,
}

/// Accounts, members and what each may do: all of it on the hub, a cut of
/// it on each member.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directory {
    pub accounts: Vec<Account>,
    pub members: Vec<Member>,
    pub grants: Vec<Grant>,
    pub trust: Vec<Trust>,
    pub projects: Vec<Project>,
}

impl Directory {
    pub fn member(&self, name: &MachineName) -> Option<&Member> {
        self.members.iter().find(|member| member.name == *name)
    }

    pub fn by_key(&self, key: &PublicKey) -> Option<&Member> {
        self.members.iter().find(|member| member.key == *key)
    }

    /// The registered machines.
    pub fn machines(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|member| member.is_machine())
    }

    pub fn account(&self, name: &AccountName) -> Option<&Account> {
        self.accounts.iter().find(|account| account.name == *name)
    }

    /// Whether `account` is logged in on `member`.
    pub fn logged_in(&self, member: &MachineName, account: &AccountName) -> bool {
        self.member(member).is_some_and(|member| member.logins.contains(account))
    }

    /// What `account` may do on `machine`: everything on its own machines,
    /// what its grants say on another's, nothing on what is no machine.
    pub fn rights(&self, account: &AccountName, machine: &MachineName) -> Rights {
        let Some(member) = self.member(machine) else { return Rights::NONE };
        if member.home.as_ref() == Some(account) {
            return Rights::ALL;
        }
        if !member.is_machine() {
            return Rights::NONE;
        }
        self.grants
            .iter()
            .filter(|grant| grant.machine == *machine && grant.account == *account)
            .fold(Rights::NONE, |rights, grant| rights.union(grant.rights))
    }

    /// Whether `member` may act for `account`: the account is logged in on
    /// it, or the account trusts it.
    pub fn trusts(&self, account: &AccountName, member: &MachineName) -> bool {
        self.logged_in(member, account)
            || self.trust.iter().any(|trust| trust.account == *account && trust.machine == *member)
    }

    /// The accounts `member` may act for.
    pub fn acts_for(&self, member: &MachineName) -> BTreeSet<AccountName> {
        let mut accounts: BTreeSet<AccountName> =
            self.member(member).map(|member| member.logins.clone()).unwrap_or_default();
        accounts.extend(self.trust.iter().filter(|trust| trust.machine == *member).map(|trust| trust.account.clone()));
        accounts
    }

    /// Whether a resident of `account` may live on `member`, or be told
    /// things there: the account is logged in on it or may use its agents.
    pub fn hosts(&self, account: &AccountName, member: &MachineName) -> bool {
        self.logged_in(member, account) || self.rights(account, member).agents
    }

    /// Whether `member`, acting for `account`, may `use_` `machine`; when
    /// not, why, in words for whoever is refused.
    pub fn may(
        &self,
        member: &MachineName,
        account: &AccountName,
        machine: &MachineName,
        use_: Use,
    ) -> Result<(), String> {
        let purpose = match use_ {
            Use::Hands => Purpose::Hands,
            Use::Agents => Purpose::Agents,
        };
        self.admits(member, account, machine, purpose)
    }

    /// Whether `from` may say something for `purpose` to `to`, for
    /// `account`; when not, why, in words for whoever is refused.
    pub fn admits(
        &self,
        from: &MachineName,
        account: &AccountName,
        to: &MachineName,
        purpose: Purpose,
    ) -> Result<(), String> {
        match self.account(account) {
            None => return Err(format!("there is no account {account}")),
            Some(found) if found.locked => return Err(format!("the account {account} is locked")),
            Some(_) => {}
        }
        if purpose.acts() {
            if !self.trusts(account, from) {
                return Err(format!("{from} may not act for the account {account}"));
            }
        } else if !(self.logged_in(from, account) || self.rights(account, from).any()) {
            return Err(format!("{from} is not a machine the account {account} uses"));
        }
        let rights = self.rights(account, to);
        let allowed = match purpose {
            Purpose::Hands => rights.hands,
            Purpose::Agents => rights.agents,
            Purpose::Look => rights.any(),
            Purpose::Words | Purpose::Echo => self.hosts(account, to),
        };
        if allowed {
            return Ok(());
        }
        Err(match self.member(to) {
            None => format!("there is no machine named {to}"),
            Some(member)
                if !member.is_machine() && matches!(purpose, Purpose::Hands | Purpose::Agents | Purpose::Look) =>
            {
                format!("{to} is logged in to this hub but not registered as a machine, so no account can use it")
            }
            Some(_) => match purpose {
                Purpose::Hands => format!("the account {account} may not work on {to}"),
                Purpose::Agents => format!("the account {account} may not use the agents on {to}"),
                Purpose::Look => format!("the account {account} may not use {to}"),
                Purpose::Words | Purpose::Echo => format!("no resident of the account {account} can be on {to}"),
            },
        })
    }

    /// The machines `account` may use, each with what it may do there.
    pub fn usable(&self, account: &AccountName) -> Vec<(&Member, Rights)> {
        self.machines()
            .map(|machine| (machine, self.rights(account, &machine.name)))
            .filter(|(_, rights)| rights.any())
            .collect()
    }

    /// The first of `accounts` for which `from` may say something for
    /// `purpose` to `to`; `None` when none may.
    pub fn account_for<'a>(
        &self,
        accounts: impl IntoIterator<Item = &'a AccountName>,
        from: &MachineName,
        to: &MachineName,
        purpose: Purpose,
    ) -> Option<AccountName> {
        accounts.into_iter().find(|account| self.admits(from, account, to, purpose).is_ok()).cloned()
    }

    pub fn project(&self, town: &TownId) -> Option<&Project> {
        self.projects.iter().find(|project| project.town == *town)
    }

    /// The project named `name` that `account` may use.
    pub fn project_named(&self, account: &AccountName, name: &TownName) -> Option<&Project> {
        self.projects.iter().find(|project| project.name == *name && self.in_project_of(account, project))
    }

    /// Whether `account` may read and write the records of `town`.
    pub fn in_project(&self, account: &AccountName, town: &TownId) -> bool {
        self.project(town).is_some_and(|project| self.in_project_of(account, project))
    }

    fn in_project_of(&self, account: &AccountName, project: &Project) -> bool {
        project.owner == *account || project.granted.contains(account)
    }

    /// Whether `member`, acting for any account it may, may read and write
    /// the records of `town`.
    pub fn member_in_project(&self, member: &MachineName, town: &TownId) -> bool {
        self.acts_for(member).iter().any(|account| self.in_project(account, town))
    }
}

/// One member's view of its hub's directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub hub: HubId,
    pub hub_name: HubName,
    /// The member this view is for.
    pub me: MachineName,
    /// Whether accounts may be opened by anyone who reaches the hub.
    #[serde(default)]
    pub self_registration: bool,
    #[serde(flatten)]
    pub directory: Directory,
}

impl View {
    /// An empty view, before the hub has given one.
    pub fn empty(hub: HubId, hub_name: HubName, me: MachineName) -> Self {
        Self { hub, hub_name, me, self_registration: false, directory: Directory::default() }
    }

    pub fn me(&self) -> Option<&Member> {
        self.directory.member(&self.me)
    }

    /// The accounts this member may act for.
    pub fn mine(&self) -> BTreeSet<AccountName> {
        self.directory.acts_for(&self.me)
    }
}

impl std::ops::Deref for View {
    type Target = Directory;

    fn deref(&self) -> &Directory {
        &self.directory
    }
}
