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

use fastctx_net::directory::{Directory, Member, Paths, Reach, View};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;

use crate::store::Store;

/// Whether a member is connected, as the hub sees it.
pub(crate) struct Presence {
    pub since: Option<Timestamp>,
    pub through: Option<String>,
    pub reach: Reach,
    pub paths: Option<Paths>,
}

/// `me`'s view of the directory; `trial`, the addresses the hub checks it
/// at for direct connections now, if it does.
pub(crate) fn view_for(
    hub: &crate::Hub,
    store: &Store,
    online: &HashMap<MachineName, Presence>,
    me: &MachineName,
    trial: Vec<std::net::SocketAddr>,
) -> View {
    let all = &store.book.directory;
    let mine = all.acts_for(me);
    let whole = mine.iter().any(|account| all.logged_in(me, account) && all.is_super(account));
    let mut directory = if whole { all.clone() } else { cut(all, me, &mine) };
    directory.projects = projects_for(all, me, &mine);
    // An account's own configuration goes whole only to the machines that
    // work for it; another is given what of it runs there, the super
    // administrator's own included.
    for account in &mut directory.accounts {
        if !mine.contains(&account.name) {
            account.collab = None;
            account.profile = account.profile.as_ref().map(|profile| profile.located(|machine| all.same(machine, me)));
        }
    }
    for member in &mut directory.members {
        let others = member.name != *me
            && member.home.as_ref().is_some_and(|owner| all.is_visitor(owner) && !mine.contains(owner));
        // Where a machine may be reached is the super administrator's to
        // choose from, and the machine's own to know; a visitor's is its own.
        let reach = (whole && !others) || member.name == *me;
        present(store, online, member, reach);
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
                account.level = fastctx_net::directory::Level::Member;
            }
        }
    }
    let logged: BTreeSet<&AccountName> =
        all.member(me).map(|member| member.logins.iter().collect()).unwrap_or_default();
    let now = Timestamp::now();
    // The codes of the accounts it is logged in to; the super
    // administrator's devices see every account's, as it may revoke any.
    let every = all
        .accounts
        .iter()
        .any(|account| logged.contains(&account.name) && account.level == fastctx_net::directory::Level::Super);
    let tokens = store
        .book
        .tokens
        .iter()
        .filter(|record| (every || logged.contains(&record.account)) && record.closed(now).is_none())
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
    let forwarders = all
        .machines()
        .filter(|member| member.name != *me)
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
    View {
        ledger_parts: true,
        project_memory: true,
        collaboration_tree: true,
        call_details: true,
        machine_log: true,
        hub: store.hub.id.clone(),
        hub_name: store.hub.name.clone(),
        hub_machine: crate::hub_machine::current(hub, store)
            .filter(|machine| whole || mine.iter().any(|account| all.rights(account, machine).hands)),
        machine_notices: store.book.machine_log.notices(all, me),
        me: me.clone(),
        self_registration: store.hub.self_registration,
        direct_links: true,
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
        goal_control: true,
        scratch_locations: true,
        goal_commands: true,
        subagents: true,
        versions: true,
        package_progress: true,
        due: true,
        uninstalls: true,
        tokens,
        hub_addresses: crate::moving::addresses_for(store, me),
        hub_moves: crate::moving::requests::told(hub, store, me, whole),
        presence: 0,
        directory,
    }
}

/// How `machine` is connected now as `reader`'s view has it, when the view
/// has it at all: the same as `view_for` puts in the whole view.
pub(crate) fn presence_for(
    store: &Store,
    online: &HashMap<MachineName, Presence>,
    reader: &MachineName,
    machine: &MachineName,
) -> Option<fastctx_net::directory::Presence> {
    let all = &store.book.directory;
    let mine = all.acts_for(reader);
    let whole = mine.iter().any(|account| all.logged_in(reader, account) && all.is_super(account));
    let member = all.member(machine)?;
    if !whole && !sees(all, reader, &mine, &users_of(all, reader), member) {
        return None;
    }
    let others = member.name != *reader
        && member.home.as_ref().is_some_and(|owner| all.is_visitor(owner) && !mine.contains(owner));
    let reach = (whole && !others) || member.name == *reader;
    let mut member = member.clone();
    present(store, online, &mut member, reach);
    if others {
        member.through = None;
        member.paths = None;
    }
    Some(fastctx_net::directory::Presence::of(&member))
}

/// The accounts that may use `me` or are logged in on it.
fn users_of(all: &Directory, me: &MachineName) -> BTreeSet<AccountName> {
    all.accounts
        .iter()
        .filter(|account| all.rights(&account.name, me).any() || all.logged_in(me, &account.name))
        .map(|account| account.name.clone())
        .collect()
}

/// Whether `me`, acting for `mine` and used by `users`, sees `member`.
fn sees(
    all: &Directory,
    me: &MachineName,
    mine: &BTreeSet<AccountName>,
    users: &BTreeSet<AccountName>,
    member: &Member,
) -> bool {
    member.name == *me
        || mine.iter().any(|account| all.rights(account, &member.name).any() || member.logins.contains(account))
        || users.iter().any(|account| all.may_act_for(account, &member.name))
}

fn cut(all: &Directory, me: &MachineName, mine: &BTreeSet<AccountName>) -> Directory {
    let users = users_of(all, me);
    let accounts: BTreeSet<AccountName> = mine.union(&users).cloned().collect();
    let members: Vec<Member> =
        all.members.iter().filter(|member| sees(all, me, mine, &users, member)).cloned().collect();
    let seen: BTreeSet<&MachineName> = members.iter().map(|member| &member.name).collect();
    // What concerns this member: given to the accounts it sees, of itself,
    // or of a machine one of its accounts holds (whole-machine sharing).
    let concerns = |account: &AccountName, machine: &MachineName| {
        accounts.contains(account) || machine == me || mine.iter().any(|account| all.holds(account, machine))
    };
    let shares: Vec<_> = all.shares.iter().filter(|share| concerns(&share.account, &share.machine)).cloned().collect();
    let assists: Vec<_> =
        all.assists.iter().filter(|assist| concerns(&assist.account, &assist.machine)).cloned().collect();
    let named: BTreeSet<&AccountName> = accounts
        .iter()
        .chain(shares.iter().map(|share| &share.account))
        .chain(assists.iter().map(|assist| &assist.account))
        .chain(all.super_account().map(|account| &account.name))
        .collect();
    Directory {
        accounts: all.accounts.iter().filter(|account| named.contains(&account.name)).cloned().collect(),
        shares,
        assists,
        plans: all.plans.iter().filter(|plan| seen.contains(&plan.machine)).cloned().collect(),
        acting: all
            .acting
            .iter()
            .filter(|acting| accounts.contains(&acting.account) && seen.contains(&acting.machine))
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
fn projects_for(
    all: &Directory,
    me: &MachineName,
    mine: &BTreeSet<AccountName>,
) -> Vec<fastctx_net::directory::Project> {
    let users = users_of(all, me);
    let concerned = |account: &AccountName| mine.contains(account) || users.contains(account);
    all.projects
        .iter()
        .filter(|project| concerned(&project.owner) || project.granted.iter().any(concerned))
        .cloned()
        .collect()
}

fn present(store: &Store, online: &HashMap<MachineName, Presence>, member: &mut Member, reach: bool) {
    match online.get(&member.name) {
        Some(presence) => {
            member.online = true;
            member.since = presence.since;
            member.through = presence.through.clone();
            member.reach = (reach && member.kind.is_computer()).then(|| presence.reach.clone());
            member.paths = presence.paths.clone();
        }
        None => {
            member.online = false;
            member.since = store.letters.get(&member.name).and_then(|letters| letters.last_seen);
            member.through = None;
            member.reach = None;
            member.paths = None;
        }
    }
}
