//! A `Directory` indexed once and asked many times (`world-concurrency.md`,
//! 热路径上不做与总数成正比的事): the hub's views of thousands of members,
//! each found by key rather than by a scan of everything. What is decided is
//! the one policy of `authorization.rs`, asked through `Lookup`; only how it
//! finds what it asks about differs. Every list keeps the positions of what
//! it holds, so what is gathered from it comes out in the directory's own
//! order, as a scan would give it.

use std::collections::{HashMap, HashSet};

use fastctx_types::access::TownId;
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;

use crate::authorization::Lookup;
use crate::directory::{Account, Assist, Directory, Level, Member, Project};

/// Where each thing of a directory is, by what it is asked by.
#[derive(Debug, Default)]
pub struct Index {
    /// Accounts by name, every position a name has, the first first.
    accounts: HashMap<AccountName, Vec<usize>>,
    /// Members by the name they have now, and by the names they had.
    named: HashMap<MachineName, Vec<usize>>,
    formerly: HashMap<MachineName, Vec<usize>>,
    projects: HashMap<TownId, usize>,
    /// Members by the account whose machines they are, and by the accounts
    /// logged in on them.
    owned: HashMap<AccountName, Vec<usize>>,
    logged: HashMap<AccountName, Vec<usize>>,
    /// Shares, assistance and acting by account and by machine as written.
    shares_by_account: HashMap<AccountName, Vec<usize>>,
    shares_by_machine: HashMap<MachineName, Vec<usize>>,
    assists_by_account: HashMap<AccountName, Vec<usize>>,
    assists_by_machine: HashMap<MachineName, Vec<usize>>,
    acting_by_account: HashMap<AccountName, Vec<usize>>,
    acting_by_machine: HashMap<MachineName, Vec<usize>>,
    plans_by_machine: HashMap<MachineName, Vec<usize>>,
    /// Projects by their owner and by every account given them.
    projects_by_account: HashMap<AccountName, Vec<usize>>,
    /// Accounts of the super administrator's level, locked or not.
    supers: Vec<usize>,
    /// Machines that take direct connections, and those that relay by owner.
    direct: Vec<usize>,
    relaying: HashMap<AccountName, Vec<usize>>,
}

impl Index {
    pub fn new(all: &Directory) -> Self {
        let mut index = Self::default();
        for (at, account) in all.accounts.iter().enumerate() {
            index.accounts.entry(account.name.clone()).or_default().push(at);
            if account.level == Level::Super {
                index.supers.push(at);
            }
        }
        for (at, member) in all.members.iter().enumerate() {
            index.named.entry(member.name.clone()).or_default().push(at);
            for was in &member.was {
                index.formerly.entry(was.clone()).or_default().push(at);
            }
            if let Some(owner) = &member.home {
                index.owned.entry(owner.clone()).or_default().push(at);
                if !member.relay.is_empty() {
                    index.relaying.entry(owner.clone()).or_default().push(at);
                }
                if !member.direct.is_empty() {
                    index.direct.push(at);
                }
            }
            for login in &member.logins {
                index.logged.entry(login.clone()).or_default().push(at);
            }
        }
        for (at, project) in all.projects.iter().enumerate() {
            index.projects.entry(project.town.clone()).or_insert(at);
            index.projects_by_account.entry(project.owner.clone()).or_default().push(at);
            for account in &project.granted {
                if *account != project.owner {
                    index.projects_by_account.entry(account.clone()).or_default().push(at);
                }
            }
        }
        for (at, share) in all.shares.iter().enumerate() {
            index.shares_by_account.entry(share.account.clone()).or_default().push(at);
            index.shares_by_machine.entry(share.machine.clone()).or_default().push(at);
        }
        for (at, assist) in all.assists.iter().enumerate() {
            index.assists_by_account.entry(assist.account.clone()).or_default().push(at);
            index.assists_by_machine.entry(assist.machine.clone()).or_default().push(at);
        }
        for (at, acting) in all.acting.iter().enumerate() {
            index.acting_by_account.entry(acting.account.clone()).or_default().push(at);
            index.acting_by_machine.entry(acting.machine.clone()).or_default().push(at);
        }
        for (at, plan) in all.plans.iter().enumerate() {
            index.plans_by_machine.entry(plan.machine.clone()).or_default().push(at);
        }
        index
    }
}

/// A directory with its index, asked as the policy asks a directory.
#[derive(Clone, Copy)]
pub struct Indexed<'a> {
    pub all: &'a Directory,
    index: &'a Index,
}

fn positions<'i, K: std::hash::Hash + Eq + ?Sized, Q: std::borrow::Borrow<K> + std::hash::Hash + Eq>(
    map: &'i HashMap<Q, Vec<usize>>,
    key: &K,
) -> &'i [usize] {
    map.get(key).map_or(&[], Vec::as_slice)
}

impl<'a> Indexed<'a> {
    pub fn new(all: &'a Directory, index: &'a Index) -> Self {
        Self { all, index }
    }

    /// Positions of the members whose name now is `name`.
    pub fn named(&self, name: &MachineName) -> &'a [usize] {
        positions(&self.index.named, name)
    }

    /// Positions of the machines whose home is `account`.
    pub fn owned(&self, account: &AccountName) -> &'a [usize] {
        positions(&self.index.owned, account)
    }

    /// Positions of the members `account` is logged in on.
    pub fn logged(&self, account: &AccountName) -> &'a [usize] {
        positions(&self.index.logged, account)
    }

    pub fn shares_by_account(&self, account: &AccountName) -> &'a [usize] {
        positions(&self.index.shares_by_account, account)
    }

    pub fn shares_by_machine(&self, machine: &MachineName) -> &'a [usize] {
        positions(&self.index.shares_by_machine, machine)
    }

    pub fn assists_by_account(&self, account: &AccountName) -> &'a [usize] {
        positions(&self.index.assists_by_account, account)
    }

    pub fn assists_by_machine(&self, machine: &MachineName) -> &'a [usize] {
        positions(&self.index.assists_by_machine, machine)
    }

    pub fn acting_by_account(&self, account: &AccountName) -> &'a [usize] {
        positions(&self.index.acting_by_account, account)
    }

    pub fn plans_by_machine(&self, machine: &MachineName) -> &'a [usize] {
        positions(&self.index.plans_by_machine, machine)
    }

    pub fn projects_by_account(&self, account: &AccountName) -> &'a [usize] {
        positions(&self.index.projects_by_account, account)
    }

    /// Positions of the accounts named `name`.
    pub fn accounts_named(&self, name: &AccountName) -> &'a [usize] {
        positions(&self.index.accounts, name)
    }

    /// Positions of the accounts of the super administrator's level.
    pub fn supers(&self) -> &'a [usize] {
        &self.index.supers
    }

    /// Positions of the machines that take direct connections.
    pub fn direct(&self) -> &'a [usize] {
        &self.index.direct
    }

    /// Positions of `account`'s machines that relay.
    pub fn relaying(&self, account: &AccountName) -> &'a [usize] {
        positions(&self.index.relaying, account)
    }

    /// The position of the member `name` names, as `Directory::member` finds it.
    pub fn position(&self, name: &MachineName) -> Option<usize> {
        self.named(name).first().or_else(|| positions(&self.index.formerly, name).first()).copied()
    }

    /// The positions of the members `name` names now or named before.
    pub fn naming(&self, name: &MachineName) -> impl Iterator<Item = usize> + 'a {
        self.named(name).iter().chain(positions(&self.index.formerly, name)).copied()
    }

    // The same questions `Directory` answers, by key.

    pub fn member(&self, name: &MachineName) -> Option<&'a Member> {
        self.position(name).map(|at| &self.all.members[at])
    }

    pub fn account(&self, name: &AccountName) -> Option<&'a Account> {
        self.accounts_named(name).first().map(|&at| &self.all.accounts[at])
    }

    pub fn canonical<'n>(&self, name: &'n MachineName) -> &'n MachineName
    where
        'a: 'n,
    {
        self.member(name).map_or(name, |member| &member.name)
    }

    pub fn same(&self, one: &MachineName, other: &MachineName) -> bool {
        self.canonical(one) == self.canonical(other)
    }

    pub fn is_super(&self, account: &AccountName) -> bool {
        self.account(account).is_some_and(Account::is_super)
    }

    pub fn is_visitor(&self, account: &AccountName) -> bool {
        self.account(account).is_some_and(|found| found.level == Level::Visitor)
    }

    pub fn super_account(&self) -> Option<&'a Account> {
        self.supers().first().map(|&at| &self.all.accounts[at])
    }

    pub fn logged_in(&self, member: &MachineName, account: &AccountName) -> bool {
        self.member(member).is_some_and(|member| member.logins.contains(account))
    }

    pub fn authorize(
        &self,
        account: &AccountName,
        action: crate::authorization::Action<'_>,
    ) -> Result<(), fastctx_types::authorization::Denial> {
        crate::authorization::authorize(self, account, action)
    }

    pub fn rights(&self, account: &AccountName, machine: &MachineName) -> fastctx_types::access::Rights {
        crate::authorization::rights(self, account, machine)
    }

    pub fn may_act_for(&self, account: &AccountName, member: &MachineName) -> bool {
        self.authorize(account, crate::authorization::Action::ActFor(member)).is_ok()
    }

    pub fn holds(&self, account: &AccountName, machine: &MachineName) -> bool {
        self.authorize(account, crate::authorization::Action::Hold(machine)).is_ok()
    }

    /// The accounts `member` may act for, as `Directory::acts_for` gives them.
    pub fn acts_for(&self, member: &MachineName) -> std::collections::BTreeSet<AccountName> {
        let member = self.canonical(member);
        let mut accounts: std::collections::BTreeSet<AccountName> =
            self.member(member).map(|member| member.logins.clone()).unwrap_or_default();
        accounts.extend(
            positions(&self.index.acting_by_machine, member).iter().map(|&at| self.all.acting[at].account.clone()),
        );
        accounts.retain(|account| self.may_act_for(account, member));
        accounts
    }
}

impl Lookup for Indexed<'_> {
    fn find_account(&self, name: &AccountName) -> Option<&Account> {
        self.account(name)
    }

    fn find_member(&self, name: &MachineName) -> Option<&Member> {
        self.member(name)
    }

    fn find_project(&self, town: &TownId) -> Option<&Project> {
        self.index.projects.get(town).map(|&at| &self.all.projects[at])
    }

    fn is_shared(&self, account: &AccountName, machine: &MachineName) -> bool {
        self.shares_by_account(account).iter().any(|&at| self.all.shares[at].machine == *machine)
    }

    fn assists(
        &self,
        account: &AccountName,
        machine: &MachineName,
        now: Timestamp,
        which: &dyn Fn(&Assist) -> bool,
    ) -> bool {
        self.assists_by_account(account)
            .iter()
            .map(|&at| &self.all.assists[at])
            .any(|assist| assist.machine == *machine && assist.live(now) && which(assist))
    }

    fn acts(&self, account: &AccountName, machine: &MachineName) -> bool {
        self.acting_by_account(account).iter().any(|&at| self.all.acting[at].machine == *machine)
    }
}

/// Positions gathered from several lists, each once, in the directory's order.
pub fn ordered(positions: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut seen = HashSet::new();
    let mut all: Vec<usize> = positions.into_iter().filter(|at| seen.insert(*at)).collect();
    all.sort_unstable();
    all
}
