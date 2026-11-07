//! Recipient discovery follows reverse indices, then applies the shared policy.

use super::{Indexed, ordered};
use crate::authorization::{Action, Lookup};
use fastctx_types::{AccountName, MachineName, access::TownId};
use std::collections::BTreeSet;

impl Indexed<'_> {
    /// Accounts that own or hold the named machine under the current policy.
    pub fn holders(&self, machine: &MachineName) -> BTreeSet<AccountName> {
        let Some(member) = self.member(machine) else { return BTreeSet::new() };
        member
            .home
            .iter()
            .cloned()
            .chain(self.shares_by_machine(&member.name).iter().map(|&at| self.all.shares[at].account.clone()))
            .filter(|account| self.holds(account, machine))
            .collect()
    }

    /// Positions of members that may currently act for an account.
    pub fn acting_members(&self, account: &AccountName) -> Vec<usize> {
        ordered(
            self.logged(account).iter().copied().chain(
                self.acting_by_account(account).iter().flat_map(|&at| self.naming(&self.all.acting[at].machine)),
            ),
        )
        .into_iter()
        .filter(|&at| self.may_act_for(account, &self.all.members[at].name))
        .collect()
    }

    /// Logged-in readers of an owned record, including its birth-machine holders.
    pub fn record_readers(&self, openers: &[AccountName], shared_on: Option<&MachineName>) -> Vec<usize> {
        let mut accounts: BTreeSet<_> = openers.iter().cloned().collect();
        if let Some(machine) = shared_on {
            accounts.extend(self.holders(machine));
        }
        ordered(
            accounts
                .iter()
                .filter(|account| crate::record_ownership::allows(self, account, openers, shared_on))
                .flat_map(|account| self.logged(account).iter().copied()),
        )
    }

    /// Members entitled to project notifications, including assisting hosts.
    pub fn project_readers(&self, town: &TownId) -> Vec<usize> {
        let Some(project) = self.find_project(town) else { return Vec::new() };
        let mut accounts = project.granted.clone();
        accounts.insert(project.owner.clone());
        if let Some(machine) = &project.shared_on {
            accounts.extend(self.holders(machine));
        }
        ordered(
            accounts
                .iter()
                .filter(|account| self.authorize(account, Action::Project(town)).is_ok())
                .flat_map(|account| self.acting_members(account)),
        )
    }

    /// Whether a member acts for any account allowed to read the project.
    pub fn member_in_project(&self, member: &MachineName, town: &TownId) -> bool {
        self.acts_for(member).iter().any(|account| self.authorize(account, Action::Project(town)).is_ok())
    }
}
