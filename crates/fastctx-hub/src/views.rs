//! Each member's view of the directory (`fastctx_net::directory::View`):
//! what it needs to act and to check what reaches it, and no more.
//!
//! A member sees itself; the accounts it may act for, the machines they may
//! use and the devices logged in to them; the accounts that may use it, and
//! the members that may act for those, which is whom calls to it come from;
//! the grants, trust and projects of all these accounts; and whether each
//! member it sees is online, since when, and through which relay. A device
//! logged in to an administrator's account sees the whole directory.

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
    let admin = mine.iter().any(|account| {
        all.logged_in(me, account) && all.account(account).is_some_and(|account| account.admin && !account.locked)
    });
    let mut directory = if admin { all.clone() } else { cut(all, me, &mine) };
    for member in &mut directory.members {
        present(store, online, member);
    }
    View {
        hub: store.hub.world.clone(),
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
                || users.iter().any(|account| all.trusts(account, &member.name))
        })
        .cloned()
        .collect();
    let seen: BTreeSet<&MachineName> = members.iter().map(|member| &member.name).collect();
    let grants: Vec<_> = all
        .grants
        .iter()
        .filter(|grant| {
            accounts.contains(&grant.account)
                || grant.machine == *me
                || all
                    .member(&grant.machine)
                    .and_then(|machine| machine.home.as_ref())
                    .is_some_and(|home| mine.contains(home))
        })
        .cloned()
        .collect();
    let named: BTreeSet<&AccountName> = accounts.iter().chain(grants.iter().map(|grant| &grant.account)).collect();
    Directory {
        accounts: all.accounts.iter().filter(|account| named.contains(&account.name)).cloned().collect(),
        grants,
        trust: all
            .trust
            .iter()
            .filter(|trust| accounts.contains(&trust.account) && seen.contains(&trust.machine))
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
