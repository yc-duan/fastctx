//! Each member's view of the directory (`fastctx_net::directory::View`):
//! what it needs to act and to check what reaches it, and no more.
//!
//! A member sees itself; the accounts it may act for, the machines they may
//! use and the devices logged in to them; the accounts that may use it, and
//! the members that may act for those, which is whom calls to it come from;
//! the shares, loans, acting and projects of all these accounts; the hub's
//! super administrator, whom every member is told of, since it may use them
//! all; and whether each member it sees is online, since when, and through
//! which relay. A device logged in to the super administrator's account sees
//! the whole directory.

use std::collections::{BTreeSet, HashMap};

use fastctx_net::directory::{Directory, Member, View};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;

use crate::store::Store;

/// Whether a member is connected, as the hub sees it.
pub(crate) struct Presence {
    pub since: Option<Timestamp>,
    pub through: Option<String>,
}

pub(crate) fn view_for(store: &Store, online: &HashMap<MachineName, Presence>, me: &MachineName) -> View {
    let all = &store.book.directory;
    let mine = all.acts_for(me);
    let whole = mine.iter().any(|account| all.logged_in(me, account) && all.is_super(account));
    let mut directory = if whole { all.clone() } else { cut(all, me, &mine) };
    for member in &mut directory.members {
        present(store, online, member);
    }
    View {
        machine_log: true,
        hub: store.hub.id.clone(),
        hub_name: store.hub.name.clone(),
        me: me.clone(),
        self_registration: store.hub.self_registration,
        directory,
    }
}

fn cut(all: &Directory, me: &MachineName, mine: &BTreeSet<AccountName>) -> Directory {
    let users: BTreeSet<AccountName> = all
        .accounts
        .iter()
        .filter(|account| all.rights(&account.name, me).any() || all.logged_in(me, &account.name))
        .map(|account| account.name.clone())
        .collect();
    let accounts: BTreeSet<AccountName> = mine.union(&users).cloned().collect();
    let members: Vec<Member> = all
        .members
        .iter()
        .filter(|member| {
            member.name == *me
                || mine.iter().any(|account| all.rights(account, &member.name).any() || member.logins.contains(account))
                || users.iter().any(|account| all.may_act_for(account, &member.name))
        })
        .cloned()
        .collect();
    let seen: BTreeSet<&MachineName> = members.iter().map(|member| &member.name).collect();
    // What concerns this member: given to the accounts it sees, of itself,
    // or of a machine one of its accounts owns.
    let concerns = |account: &AccountName, machine: &MachineName| {
        accounts.contains(account)
            || machine == me
            || all.member(machine).and_then(|machine| machine.home.as_ref()).is_some_and(|home| mine.contains(home))
    };
    let shares: Vec<_> = all.shares.iter().filter(|share| concerns(&share.account, &share.machine)).cloned().collect();
    let lends: Vec<_> = all.lends.iter().filter(|lend| concerns(&lend.account, &lend.machine)).cloned().collect();
    let named: BTreeSet<&AccountName> = accounts
        .iter()
        .chain(shares.iter().map(|share| &share.account))
        .chain(lends.iter().map(|lend| &lend.account))
        .chain(all.super_account().map(|account| &account.name))
        .collect();
    Directory {
        accounts: all.accounts.iter().filter(|account| named.contains(&account.name)).cloned().collect(),
        shares,
        lends,
        acting: all
            .acting
            .iter()
            .filter(|acting| accounts.contains(&acting.account) && seen.contains(&acting.machine))
            .cloned()
            .collect(),
        projects: all
            .projects
            .iter()
            .filter(|project| {
                accounts.contains(&project.owner) || project.granted.iter().any(|account| accounts.contains(account))
            })
            .cloned()
            .collect(),
        members,
    }
}

fn present(store: &Store, online: &HashMap<MachineName, Presence>, member: &mut Member) {
    match online.get(&member.name) {
        Some(presence) => {
            member.online = true;
            member.since = presence.since;
            member.through = presence.through.clone();
        }
        None => {
            member.online = false;
            member.since = store.letters.get(&member.name).and_then(|letters| letters.last_seen);
            member.through = None;
        }
    }
}
