//! The standing drill of the hub at the scale it is built for
//! (`world-concurrency.md`, 只给有关的，给变化的部分): a network of 5,000
//! accounts and 10,000 machines, where each member's view comes from the
//! directory's index as the full scan made it, member for member and in
//! order; is made in well under a millisecond; and one change, a machine
//! shared with one more account, changes the views of the few members it
//! touches, each by a few hundred bytes, the super administrator's whole
//! view of every machine among them.

use super::*;
use fastctx_net::directory::{Account, ActsFor, Assist, Kind, Project, Share};
use std::time::{Duration, Instant};

const ACCOUNTS: usize = 5000;
const MACHINES: usize = 10_000;
/// Sessions of members reading their views when the change comes.
const SESSIONS: usize = 1000;
/// Views checked against the full scan, which takes long on purpose.
const SCANNED: usize = 3;

/// How long a member's view may take on average, with room for an
/// unoptimized build on a busy machine: the hub's own build takes about
/// ten microseconds.
const VIEW_AT_MOST: Duration = Duration::from_millis(5);

fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

fn account(i: usize) -> AccountName {
    format!("a{i}").parse().expect("an account name")
}

fn machine(i: usize) -> MachineName {
    format!("m{i}").parse().expect("a machine name")
}

fn member(name: MachineName, kind: Kind, logins: BTreeSet<AccountName>, home: Option<AccountName>, n: u64) -> Member {
    let mut key = [0u8; 32];
    key[..8].copy_from_slice(&(n + 1).to_le_bytes());
    Member {
        name,
        was: Vec::new(),
        key: fastctx_net::sign::PublicKey::from_bytes(key),
        kind,
        logins,
        home,
        managed: false,
        code: None,
        group: None,
        direct: Vec::new(),
        relay: Vec::new(),
        added: jiff::Timestamp::from_second(1_700_000_000).expect("a time"),
        online: false,
        since: None,
        through: None,
        reach: None,
        paths: None,
    }
}

/// A network of every kind of tie at once: owners and their machines,
/// phones, shares, assistances, machines acting for accounts and projects
/// granted to several; the first account its super administrator.
fn network() -> Directory {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let at = jiff::Timestamp::from_second(1_700_000_000).expect("a time");
    let mut all = Directory::default();
    for i in 0..ACCOUNTS {
        all.accounts.push(Account {
            name: account(i),
            level: if i == 0 {
                Level::Super
            } else if i % 50 == 0 {
                Level::Visitor
            } else {
                Level::Member
            },
            locked: i > 0 && i % 97 == 0,
            opened: at,
            collab: None,
            profile: None,
            promotion: false,
        });
    }
    for i in 0..MACHINES {
        let owner = account(i % ACCOUNTS);
        let mut one = member(machine(i), Kind::Computer, [owner.clone()].into_iter().collect(), Some(owner), i as u64);
        if i % 100 == 0 {
            one.direct = vec!["203.0.113.1:7444".parse().expect("an address")];
        }
        all.members.push(one);
    }
    for i in 0..1000 {
        let logins = [account(rng(&mut seed) as usize % ACCOUNTS)].into_iter().collect();
        let name = format!("phone{i}").parse().expect("a machine name");
        all.members.push(member(name, Kind::Phone, logins, None, (MACHINES + i) as u64));
    }
    for _ in 0..3000 {
        all.shares.push(Share {
            machine: machine(rng(&mut seed) as usize % MACHINES),
            account: account(rng(&mut seed) as usize % ACCOUNTS),
            jobs: None,
        });
    }
    for _ in 0..2000 {
        all.assists.push(Assist {
            machine: machine(rng(&mut seed) as usize % MACHINES),
            account: account(rng(&mut seed) as usize % ACCOUNTS),
            agent: "codex".parse().expect("an agent"),
            home: None,
            until: None,
            weekly: None,
            daily: None,
            five_hour: None,
            turns: None,
        });
    }
    for _ in 0..1000 {
        all.acting.push(ActsFor {
            machine: machine(rng(&mut seed) as usize % MACHINES),
            account: account(rng(&mut seed) as usize % ACCOUNTS),
            since: at,
        });
    }
    for i in 0..2000 {
        let owner = account(rng(&mut seed) as usize % ACCOUNTS);
        let granted = (0..3).map(|_| account(rng(&mut seed) as usize % ACCOUNTS)).collect();
        all.projects.push(Project {
            town: fastctx_types::access::TownId::from_bytes((i as u64 + 1).to_le_bytes()),
            name: format!("p{i}").parse().expect("a project name"),
            owner,
            granted,
        });
    }
    all
}

/// The members a view holds as the full scan finds them.
fn scanned(all: &Directory, me: &MachineName, mine: &BTreeSet<AccountName>) -> Vec<Member> {
    let users: BTreeSet<AccountName> = all
        .accounts
        .iter()
        .filter(|account| all.rights(&account.name, me).any() || all.logged_in(me, &account.name))
        .map(|account| account.name.clone())
        .collect();
    all.members
        .iter()
        .filter(|member| {
            member.name == *me
                || mine.iter().any(|account| all.rights(account, &member.name).any() || member.logins.contains(account))
                || users.iter().any(|account| all.may_act_for(account, &member.name))
        })
        .cloned()
        .collect()
}

/// One member's view as the hub gives it.
fn view_of(x: &Indexed<'_>, me: &MachineName) -> View {
    let mine = x.acts_for(me);
    let users = users_in(x, me);
    let whole = mine.iter().any(|account| x.logged_in(me, account) && x.is_super(account));
    let mut view =
        View::empty(fastctx_net::ids::HubId::from_bytes([7; 8]), "home".parse().expect("a name"), me.clone());
    view.directory = if whole { x.all.clone() } else { cut_in(x, me, &mine, &users) };
    view.directory.projects = projects_in(x, &mine, &users);
    view
}

#[test]
fn views_at_scale() {
    let mut all = network();
    let index = Index::new(&all);
    let x = Indexed::new(&all, &index);
    let mut seed = 42u64;
    // The index finds what the scan found.
    for _ in 0..SCANNED {
        let me = machine(rng(&mut seed) as usize % MACHINES);
        let mine = x.acts_for(&me);
        let users = users_in(&x, &me);
        assert_eq!(scanned(&all, &me, &mine), cut_in(&x, &me, &mine, &users).members, "the view of {me}");
    }
    let sessions: Vec<MachineName> =
        std::iter::once(machine(0)).chain((0..SESSIONS).map(|_| machine(rng(&mut seed) as usize % MACHINES))).collect();
    let began = Instant::now();
    let before: Vec<View> = sessions.iter().map(|me| view_of(&x, me)).collect();
    let each = began.elapsed() / sessions.len() as u32;
    assert!(each < VIEW_AT_MOST, "a member's view took {each:?} on average");
    assert_eq!(before[0].directory.members.len(), all.members.len(), "the super administrator sees every machine");
    drop(index);

    // One more share: only the views it touches change, and by little.
    all.shares.push(Share { machine: machine(4242), account: account(17), jobs: None });
    let index = Index::new(&all);
    let x = Indexed::new(&all, &index);
    let mut told = 0;
    for (at, (old, me)) in before.iter().zip(&sessions).enumerate() {
        let new = view_of(&x, me);
        let Some(delta) = fastctx_net::view_delta::between(old, &new, 1, 2).expect("a change of a view") else {
            continue;
        };
        told += 1;
        let bytes = serde_json::to_vec(&delta).expect("a change serializes").len();
        let most = if at == 0 { 1024 } else { 64 * 1024 };
        assert!(bytes < most, "the change of {me}'s view is {bytes} bytes");
        let (put, _) = fastctx_net::view_delta::apply(old, &delta).expect("a change applies");
        assert_eq!(put.directory, new.directory, "the change makes {me}'s view what it is now");
    }
    assert!((1..=50).contains(&told), "{told} of {} views changed for one share", sessions.len());
}
