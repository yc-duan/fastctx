//! Each member's view of the directory (`fastctx_net::directory::View`):
//! what it needs to act and to check what reaches it, and no more.
//!
//! A member sees itself; the accounts it may act for, the machines they may
//! use and the devices logged in to them; the accounts that may use it, and
//! the members that may act for those, which is whom calls to it come from;
//! the shares, assistance, acting and projects of all these accounts; the hub's
//! super administrator, whom every member is told of, since it may use them
//! all; whether each member it sees is online, since when, through which
//! machine, and how its lines go; and every machine that allows direct
//! connections, whatever its account, since a member whose own line is down
//! reaches the hub through one. A device logged in to the super
//! administrator's account sees the whole directory, what each online
//! machine's direct address may be chosen from, and the moves of the hub
//! (`crate::moving`), as the machine the hub moves to sees its move. Once
//! the hub was moved, every member is told where it reaches the hub now.
//!
//! A visitor's machines are its alone (`world-network.md`, 一个中枢，几个账号):
//! the super administrator's view names them and whether they are online,
//! nothing of how they connect; nobody else's view has them. A member whose
//! release does not read visitors is sent them as members. Each member is
//! also told the tokens its accounts made that still let devices in, and
//! the super administrator's devices those of every account.

use std::collections::{BTreeSet, HashMap};

use fastctx_net::directory::{Directory, Level, Member, Paths, Reach, View};
use fastctx_net::directory_index::{Index, Indexed, ordered};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;

/// Whether a member is connected, as the hub sees it.
#[derive(Clone)]
pub(crate) struct Presence {
    pub since: Option<Timestamp>,
    pub through: Option<String>,
    pub reach: Reach,
    pub paths: Option<Paths>,
    pub strain: Option<fastctx_types::health::Strain>,
}

pub(crate) trait Online {
    fn get(&self, name: &MachineName, key: &fastctx_net::sign::PublicKey) -> Option<Presence>;
    /// Whether one of its logins has the workbench open, as it last told
    /// (`crate::workbench`).
    fn workbench(&self, name: &MachineName) -> Option<bool> {
        let _ = name;
        None
    }
}
impl Online for HashMap<MachineName, Presence> {
    fn get(&self, name: &MachineName, _key: &fastctx_net::sign::PublicKey) -> Option<Presence> {
        HashMap::get(self, name).cloned()
    }
}
impl Online for crate::Hub {
    fn get(&self, name: &MachineName, key: &fastctx_net::sign::PublicKey) -> Option<Presence> {
        self.sessions().get(name).filter(|session| session.key == *key).map(crate::presence_of)
    }
    fn workbench(&self, name: &MachineName) -> Option<bool> {
        crate::workbench::of(self, name)
    }
}

/// `me`'s view of the directory; `trial`, the addresses the hub checks it
/// at for direct connections now, if it does.
pub(crate) fn view_for(
    hub: &crate::Hub,
    store: &crate::store::Snapshot,
    online: &impl Online,
    me: &MachineName,
    trial: Vec<std::net::SocketAddr>,
) -> Result<View, String> {
    view_for_in(hub, store, &store.indexed(), online, me, trial)
}

/// `me`'s view, the directory asked through `x`, its index: one index
/// makes the views of every member told of one change (`Hub::push_views`).
pub(crate) fn view_for_in(
    hub: &crate::Hub,
    store: &crate::store::Snapshot,
    x: &Indexed<'_>,
    online: &impl Online,
    me: &MachineName,
    trial: Vec<std::net::SocketAddr>,
) -> Result<View, String> {
    let _timing = hub.health.view.time();
    let all = &store.book.directory;
    let mine = x.acts_for(me);
    let users = users_in(x, me);
    let whole = mine.iter().any(|account| x.logged_in(me, account) && x.is_super(account));
    let mut directory = if whole { all.clone() } else { cut_in(x, me, &mine, &users) };
    directory.projects = projects_in(x, &mine, &users);
    // An account's own configuration goes whole only to the machines that
    // work for it; another is given what of it runs there, the super
    // administrator's own included.
    for account in &mut directory.accounts {
        if !mine.contains(&account.name) {
            let account = std::sync::Arc::make_mut(account);
            account.collab = None;
            account.history = false;
            account.profile = account.profile.as_ref().map(|profile| profile.located(|machine| x.same(machine, me)));
        }
    }
    for member in &mut directory.members {
        let others = member.name != *me
            && member.home.as_ref().is_some_and(|owner| x.is_visitor(owner) && !mine.contains(owner));
        // Where a machine may be reached is the super administrator's to
        // choose from, and the machine's own to know; a visitor's is its own.
        let reach = (whole && !others) || member.name == *me;
        let own = member.home.as_ref().is_some_and(|owner| mine.contains(owner));
        let member = std::sync::Arc::make_mut(member);
        present(store, online, member, reach, own);
        if others {
            member.direct.clear();
            member.relay.clear();
            member.through = None;
            member.paths = None;
            member.group = None;
        }
    }
    // A release that cannot read a visitor would lose its whole view to one.
    if !store.letters.get(me).is_some_and(|letters| letters.roles) {
        for account in &mut directory.accounts {
            if account.level == fastctx_net::directory::Level::Visitor {
                std::sync::Arc::make_mut(account).level = fastctx_net::directory::Level::Member;
            }
        }
    }
    let logged: BTreeSet<&AccountName> = x.member(me).map(|member| member.logins.iter().collect()).unwrap_or_default();
    let now = Timestamp::now();
    // The codes of the accounts it is logged in to; the super
    // administrator's devices see every account's, as it may revoke any.
    let every =
        logged.iter().any(|name| x.accounts_named(name).iter().any(|&at| all.accounts[at].level == Level::Super));
    let candidates = if every {
        (0..store.book.tokens.len()).collect()
    } else {
        ordered(logged.iter().flat_map(|account| store.tokens.get(*account).into_iter().flatten()).copied())
    };
    let tokens = candidates
        .into_iter()
        .map(|at| &store.book.tokens[at])
        .filter(|record| record.closed(now).is_none())
        .map(|record| fastctx_types::accounts::TokenSeen {
            key: record.key.to_string(),
            account: record.account.clone(),
            label: record.label.clone(),
            kind: record.kind,
            managed: record.managed,
            uses: record.uses,
            used: record.used_count(),
            expires: record.expires,
            way: record.way,
            name: record.name.clone(),
            made_by: Some(record.made_by.clone()),
            made_at: Some(record.made_at),
        })
        .collect();
    // Those that take direct connections forward for any machine; one that
    // relays, for its owner's own.
    let forwarders = ordered(x.direct().iter().chain(mine.iter().flat_map(|account| x.relaying(account))).copied())
        .into_iter()
        .map(|at| &all.members[at])
        .filter(|member| member.name != *me)
        // A visitor's machine is nobody else's to know of, nor to go through.
        .filter(|member| !member.home.as_ref().is_some_and(|owner| x.is_visitor(owner) && !mine.contains(owner)))
        .filter_map(|member| {
            let relays = member.home.as_ref().is_some_and(|owner| mine.contains(owner)) && !member.relay.is_empty();
            let at = if !member.direct.is_empty() {
                member.direct.clone()
            } else if relays {
                member.relay.clone()
            } else {
                return None;
            };
            Some(fastctx_net::dial::Forwarder { name: member.name.clone(), at })
        })
        .collect();
    Ok(View {
        shared_records: true,
        ledger_parts: true,
        named_asks: true,
        project_memory: true,
        collaboration_tree: true,
        history_switch: true,
        call_details: true,
        machine_log: true,
        layers: true,
        hub: store.hub.id.clone(),
        hub_name: store.hub.name.clone(),
        hub_machine: crate::hub_machine::current(hub, store)
            .filter(|machine| whole || mine.iter().any(|account| x.rights(account, machine).hands)),
        machine_notices: store.book.machine_log.notices(x, me).map_err(|e| e.to_string())?,
        troubles: hub.troubles.for_member(x, me),
        me: me.clone(),
        self_registration: store.hub.self_registration,
        direct_links: true,
        // Its machines tell how their own work goes with each heartbeat
        // (`crate::strain`).
        health: true,
        trial,
        forwarders,
        moving: true,
        renames: true,
        afoot: true,
        forgets: true,
        managed: true,
        roles: true,
        groups: true,
        dissolves: true,
        deletes_projects: true,
        project_entry: true,
        deletes_project_conversations: true,
        project_deletions: crate::project_deletions::for_member(store, me),
        lan_relays: true,
        paced_intake: true,
        profiles: true,
        account_records: true,
        conversation_plans: true,
        plan_copies: true,
        skill_files: true,
        record_keys: true,
        goal_control: true,
        scratch_locations: true,
        goal_commands: true,
        subagents: true,
        family_prompts: true,
        versions: true,
        package_progress: true,
        due: true,
        uninstalls: true,
        tokens,
        hub_addresses: crate::moving::addresses_for(store, me),
        hub_moves: crate::moving::requests::told(hub, store, me, whole),
        presence: 0,
        revision: 0,
        directory,
    })
}

/// How `machine` is connected now as `reader`'s view has it, when the view
/// has it at all: the same as `view_for` puts in the whole view.
pub(crate) fn presence_for(
    store: &crate::store::Snapshot,
    x: &Indexed<'_>,
    online: &impl Online,
    reader: &MachineName,
    machine: &MachineName,
) -> Option<fastctx_net::directory::Presence> {
    let mine = x.acts_for(reader);
    let whole = mine.iter().any(|account| x.logged_in(reader, account) && x.is_super(account));
    let member = x.member(machine)?;
    if !whole && !sees_in(x, reader, &mine, &users_in(x, reader), member) {
        return None;
    }
    let others = member.name != *reader
        && member.home.as_ref().is_some_and(|owner| x.is_visitor(owner) && !mine.contains(owner));
    let reach = (whole && !others) || member.name == *reader;
    let own = member.home.as_ref().is_some_and(|owner| mine.contains(owner));
    let mut member = Member::clone(member);
    present(store, online, &mut member, reach, own);
    if others {
        member.through = None;
        member.paths = None;
    }
    Some(fastctx_net::directory::Presence::of(&member))
}

/// The accounts that may use `me` or are logged in on it.
#[allow(dead_code, reason = "the authorization oracle's entry; the hub asks `users_in`")]
fn users_of(all: &Directory, me: &MachineName) -> BTreeSet<AccountName> {
    let index = Index::new(all);
    users_in(&Indexed::new(all, &index), me)
}

/// The accounts that may use `me` or are logged in on it, found among
/// those its record names and the super administrators: no other may.
pub(crate) fn users_in(x: &Indexed<'_>, me: &MachineName) -> BTreeSet<AccountName> {
    let all = x.all;
    let mut named: Vec<&AccountName> = x.supers().iter().map(|&at| &all.accounts[at].name).collect();
    if let Some(member) = x.member(me) {
        named.extend(member.home.iter());
        named.extend(member.logins.iter());
        named.extend(x.shares_by_machine(&member.name).iter().map(|&at| &all.shares[at].account));
        named.extend(x.assists_by_machine(&member.name).iter().map(|&at| &all.assists[at].account));
    }
    named
        .into_iter()
        .filter(|account| !x.accounts_named(account).is_empty())
        .filter(|account| x.rights(account, me).any() || x.logged_in(me, account))
        .cloned()
        .collect()
}

/// Whether `me`, acting for `mine` and used by `users`, sees `member`.
#[allow(dead_code, reason = "the authorization oracle's entry; the hub asks `sees_in`")]
fn sees(
    all: &Directory,
    me: &MachineName,
    mine: &BTreeSet<AccountName>,
    users: &BTreeSet<AccountName>,
    member: &Member,
) -> bool {
    let index = Index::new(all);
    sees_in(&Indexed::new(all, &index), me, mine, users, member)
}

pub(crate) fn sees_in(
    x: &Indexed<'_>,
    me: &MachineName,
    mine: &BTreeSet<AccountName>,
    users: &BTreeSet<AccountName>,
    member: &Member,
) -> bool {
    member.name == *me
        || mine.iter().any(|account| x.rights(account, &member.name).any() || member.logins.contains(account))
        || users.iter().any(|account| x.may_act_for(account, &member.name))
}

#[allow(dead_code, reason = "the authorization oracle's entry; the hub asks `cut_in`")]
fn cut(all: &Directory, me: &MachineName, mine: &BTreeSet<AccountName>) -> Directory {
    let index = Index::new(all);
    let x = Indexed::new(all, &index);
    let users = users_in(&x, me);
    cut_in(&x, me, mine, &users)
}

/// `me`'s cut of the directory, acting for `mine` and used by `users`.
/// What it may see is found from what its accounts are named in, each
/// then judged as a scan of everything would judge it, and kept in the
/// directory's order.
fn cut_in(x: &Indexed<'_>, me: &MachineName, mine: &BTreeSet<AccountName>, users: &BTreeSet<AccountName>) -> Directory {
    let all = x.all;
    let accounts: BTreeSet<AccountName> = mine.union(users).cloned().collect();
    // A super administrator's account may see any machine.
    let every =
        mine.iter().any(|account| x.accounts_named(account).iter().any(|&at| all.accounts[at].level == Level::Super));
    let candidates = if every {
        (0..all.members.len()).collect()
    } else {
        let mut at: Vec<usize> = x.named(me).to_vec();
        for account in mine {
            at.extend(x.owned(account));
            at.extend(x.logged(account));
            for &share in x.shares_by_account(account) {
                at.extend(x.naming(&all.shares[share].machine));
            }
            for &assist in x.assists_by_account(account) {
                at.extend(x.naming(&all.assists[assist].machine));
            }
        }
        for account in users {
            at.extend(x.logged(account));
            for &acting in x.acting_by_account(account) {
                if x.may_act_for(account, &all.acting[acting].machine) {
                    at.extend(x.naming(&all.acting[acting].machine));
                }
            }
        }
        ordered(at)
    };
    let members: Vec<std::sync::Arc<Member>> = candidates
        .into_iter()
        .map(|at| &all.members[at])
        .filter(|member| sees_in(x, me, mine, users, member))
        .cloned()
        .collect();
    let seen: BTreeSet<&MachineName> = members.iter().map(|member| &member.name).collect();
    // What concerns this member: given to the accounts it sees, of itself,
    // or of a machine one of its accounts holds (whole-machine sharing).
    let concerns = |account: &AccountName, machine: &MachineName| {
        accounts.contains(account) || machine == me || mine.iter().any(|account| x.holds(account, machine))
    };
    // The names, now and before, of the machines its accounts may hold.
    let mut held: Vec<&MachineName> = Vec::new();
    for account in mine {
        let shared = x.shares_by_account(account).iter().flat_map(|&share| x.naming(&all.shares[share].machine));
        for at in x.owned(account).iter().copied().chain(shared) {
            let member = &all.members[at];
            held.push(&member.name);
            held.extend(member.was.iter());
        }
    }
    let shares: Vec<_> = ordered(
        accounts
            .iter()
            .flat_map(|account| x.shares_by_account(account))
            .chain(x.shares_by_machine(me))
            .chain(held.iter().flat_map(|name| x.shares_by_machine(name)))
            .copied(),
    )
    .into_iter()
    .map(|at| &all.shares[at])
    .filter(|share| concerns(&share.account, &share.machine))
    .cloned()
    .collect();
    let assists: Vec<_> = ordered(
        accounts
            .iter()
            .flat_map(|account| x.assists_by_account(account))
            .chain(x.assists_by_machine(me))
            .chain(held.iter().flat_map(|name| x.assists_by_machine(name)))
            .copied(),
    )
    .into_iter()
    .map(|at| &all.assists[at])
    .filter(|assist| concerns(&assist.account, &assist.machine))
    .cloned()
    .collect();
    let named: BTreeSet<&AccountName> = accounts
        .iter()
        .chain(shares.iter().map(|share| &share.account))
        .chain(assists.iter().map(|assist| &assist.account))
        .chain(x.super_account().map(|account| &account.name))
        .collect();
    Directory {
        accounts: ordered(named.iter().flat_map(|name| x.accounts_named(name)).copied())
            .into_iter()
            .map(|at| all.accounts[at].clone())
            .collect(),
        shares,
        assists,
        plans: ordered(seen.iter().flat_map(|name| x.plans_by_machine(name)).copied())
            .into_iter()
            .map(|at| all.plans[at].clone())
            .collect(),
        acting: ordered(accounts.iter().flat_map(|account| x.acting_by_account(account)).copied())
            .into_iter()
            .map(|at| &all.acting[at])
            // A use kept from an ended assistance acts for no one, and no
            // member is told of it until the assistance is given again.
            .filter(|acting| seen.contains(&acting.machine) && x.may_act_for(&acting.account, &acting.machine))
            .cloned()
            .collect(),
        projects: Vec::new(),
        members,
    }
}

/// The projects of the accounts `me` acts for or that may work on it: what
/// it checks a call naming a project against. The super administrator's
/// machine gets no others; and of these, its people are shown only their
/// own accounts' (`world-network.md`, 项目也照同一个规矩).
#[allow(dead_code, reason = "the authorization oracle's entry; the hub asks `projects_in`")]
fn projects_for(
    all: &Directory,
    me: &MachineName,
    mine: &BTreeSet<AccountName>,
) -> Vec<std::sync::Arc<fastctx_net::directory::Project>> {
    let index = Index::new(all);
    let x = Indexed::new(all, &index);
    let users = users_in(&x, me);
    projects_in(&x, mine, &users)
}

fn projects_in(
    x: &Indexed<'_>,
    mine: &BTreeSet<AccountName>,
    users: &BTreeSet<AccountName>,
) -> Vec<std::sync::Arc<fastctx_net::directory::Project>> {
    ordered(mine.iter().chain(users).flat_map(|account| x.projects_by_account(account)).copied())
        .into_iter()
        .map(|at| x.all.projects[at].clone())
        .collect()
}

/// `member` as it is connected now; `own`, whether the view is its owner's,
/// who alone is told how its logins stand.
fn present(store: &crate::store::Snapshot, online: &impl Online, member: &mut Member, reach: bool, own: bool) {
    member.settled = store.letters.get(&member.name).and_then(|letters| letters.settled);
    member.workbench = if own { online.workbench(&member.name) } else { None };
    match online.get(&member.name, &member.key) {
        Some(presence) => {
            member.online = true;
            member.since = presence.since;
            member.through = presence.through.clone();
            member.reach = (reach && member.kind.is_computer()).then(|| presence.reach.clone());
            member.paths = presence.paths.clone();
            member.strain = presence.strain;
        }
        None => {
            member.online = false;
            member.since = store.letters.get(&member.name).and_then(|letters| letters.last_seen);
            member.through = None;
            member.reach = None;
            member.paths = None;
            member.strain = None;
        }
    }
}

#[cfg(test)]
mod scale_drill;
