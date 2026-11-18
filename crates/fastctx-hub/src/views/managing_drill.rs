//! One who manages a login with its owner (`usage-assist-limits`
//! 2026-11-19): what the hub lets them change of it, the share of an account
//! it assists already and nothing else, and what of it their machines see,
//! everyone that login assists and no other login's.

use super::scale_drill::member;
use super::*;
use crate::manage::set_share;
use fastctx_net::directory::{Account, Assist, Kind, Level};
use fastctx_types::usage::Usd;
use jiff::Timestamp;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn name(s: &str) -> AccountName {
    s.parse().expect("an account name")
}

fn machine(s: &str) -> MachineName {
    s.parse().expect("a machine name")
}

fn agent() -> fastctx_types::AgentName {
    "claude".parse().expect("an agent")
}

fn other() -> PathBuf {
    PathBuf::from("/logins/other")
}

fn assist(to: &str, home: Option<PathBuf>) -> Assist {
    Assist {
        machine: machine("owner-pc"),
        account: name(to),
        agent: agent(),
        home,
        until: None,
        weekly: None,
        daily: None,
        five_hour: None,
        turns: None,
        share: None,
        manages: None,
    }
}

/// The owner's default login assists ann, who manages it; bob, held to a
/// share until a day far off; and cat, as an earlier release kept one with no
/// limit. Its other login assists dan.
fn network() -> Directory {
    let at = Timestamp::from_second(1_700_000_000).expect("a time");
    let mut all = Directory::default();
    all.accounts.push(Arc::new(Account {
        name: name("root"),
        level: Level::Super,
        locked: false,
        opened: at,
        collab: None,
        history: false,
        profile: None,
        promotion: false,
    }));
    for (i, who) in ["owner", "ann", "bob", "cat", "dan"].into_iter().enumerate() {
        all.accounts.push(Arc::new(Account {
            name: name(who),
            level: Level::Member,
            ..Account::clone(&all.accounts[0])
        }));
        let logins = [name(who)].into_iter().collect();
        all.members.push(Arc::new(member(
            machine(&format!("{who}-pc")),
            Kind::Computer,
            logins,
            Some(name(who)),
            i as u64,
        )));
    }
    all.assists.push(Arc::new(Assist { manages: Some(true), ..assist("ann", None) }));
    all.assists.push(Arc::new(Assist {
        until: Some(Timestamp::from_second(4_000_000_000).expect("a time")),
        weekly: Some(Usd(900_000_000)),
        five_hour: Some(80),
        turns: Some(3),
        share: Some(30),
        manages: Some(false),
        ..assist("bob", None)
    }));
    all.assists.push(Arc::new(assist("cat", None)));
    all.assists.push(Arc::new(Assist {
        weekly: Some(Usd(300_000_000)),
        share: Some(10),
        ..assist("dan", Some(other()))
    }));
    all
}

fn kept(all: &Directory, to: &str, home: Option<&Path>) -> Assist {
    let found = all.assists.iter().find(|kept| kept.account == name(to) && kept.is(&agent(), home));
    Assist::clone(found.expect("an assistance"))
}

/// What `by` is told on giving `to` this share of the default login.
fn set(all: &mut Directory, by: &str, to: &str, share: Option<u8>) -> Option<Result<(), String>> {
    let weekly = share.map(|share| Usd(u64::from(share) * 30_000_000));
    set_share(all, &name(by), (&machine("owner-pc"), &agent(), None), &name(to), true, weekly, share)
}

#[test]
fn one_who_manages_sets_a_share_and_nothing_else() {
    let mut all = network();
    let before = kept(&all, "bob", None);

    // Another's share: only the share changes, in percent and in dollars.
    assert_eq!(set(&mut all, "ann", "bob", Some(45)), Some(Ok(())));
    let after = kept(&all, "bob", None);
    assert_eq!((after.share, after.weekly), (Some(45), Some(Usd(1_350_000_000))));
    assert_eq!(after, Assist { share: after.share, weekly: after.weekly, ..before.clone() });

    // Their own: they manage still, with a share and with none again.
    assert_eq!(set(&mut all, "ann", "ann", Some(20)), Some(Ok(())));
    let own = kept(&all, "ann", None);
    assert_eq!((own.share, own.manages, own.manages()), (Some(20), Some(true), true));
    assert_eq!(set(&mut all, "ann", "ann", None), Some(Ok(())));
    assert_eq!((kept(&all, "ann", None).share, kept(&all, "ann", None).weekly), (None, None));

    // No limit is theirs alone; a share is a percent, with its dollars.
    let unchanged = all.clone();
    assert!(matches!(set(&mut all, "ann", "bob", None), Some(Err(_))));
    assert!(matches!(set(&mut all, "ann", "bob", Some(0)), Some(Err(_))));
    assert!(matches!(set(&mut all, "ann", "bob", Some(101)), Some(Err(_))));
    let login = (&machine("owner-pc"), &agent(), None);
    assert!(matches!(set_share(&mut all, &name("ann"), login, &name("bob"), true, None, Some(40)), Some(Err(_))));
    // No one ended, no one added.
    assert!(matches!(set_share(&mut all, &name("ann"), login, &name("bob"), false, None, None), Some(Err(_))));
    assert!(matches!(set(&mut all, "ann", "dan", Some(40)), Some(Err(_))));
    assert!(matches!(set(&mut all, "ann", "owner", Some(40)), Some(Err(_))));
    // A login they do not manage is no more theirs than anyone's.
    let other = other();
    let elsewhere = (&machine("owner-pc"), &agent(), Some(other.as_path()));
    assert_eq!(set_share(&mut all, &name("ann"), elsewhere, &name("dan"), true, Some(Usd(1)), Some(40)), None);
    // Nor is the login anything to one held to a share of it.
    assert_eq!(set(&mut all, "bob", "ann", Some(40)), None);
    assert_eq!(set(&mut all, "bob", "bob", Some(90)), None);
    assert_eq!(all, unchanged);

    // An assistance of before the word, with no limit, manages; and still
    // does once it takes a share for itself.
    assert!(kept(&all, "cat", None).manages());
    assert_eq!(set(&mut all, "cat", "cat", Some(15)), Some(Ok(())));
    let cat = kept(&all, "cat", None);
    assert_eq!((cat.share, cat.manages), (Some(15), Some(true)));

    // One whose assistance has ended manages nothing.
    let ann = all.assists.iter().position(|kept| kept.account == name("ann")).expect("ann");
    let ended = Timestamp::from_second(1_700_000_000).expect("a time");
    all.assists[ann] = Arc::new(Assist { until: Some(ended), ..Assist::clone(&all.assists[ann]) });
    assert_eq!(set(&mut all, "ann", "bob", Some(50)), None);
}

#[test]
fn one_who_manages_sees_everyone_the_login_assists() {
    let all = network();
    let index = Index::new(&all);
    let x = Indexed::new(&all, &index);
    let seen = |me: &str| -> Vec<(AccountName, Option<PathBuf>)> {
        let me = machine(me);
        let (mine, users) = (x.acts_for(&me), users_in(&x, &me));
        let cut = cut_in(&x, &me, &mine, &users);
        // Whoever is named in an assistance shown is an account shown.
        for assist in &cut.assists {
            assert!(
                cut.accounts.iter().any(|account| account.name == assist.account),
                "{} in {me}'s view",
                assist.account
            );
        }
        let mut seen: Vec<_> = cut.assists.iter().map(|assist| (assist.account.clone(), assist.home.clone())).collect();
        seen.sort();
        seen
    };
    let of =
        |names: &[&str]| -> Vec<(AccountName, Option<PathBuf>)> { names.iter().map(|who| (name(who), None)).collect() };
    assert_eq!(seen("ann-pc"), of(&["ann", "bob", "cat"]));
    assert_eq!(seen("cat-pc"), of(&["ann", "bob", "cat"]));
    assert_eq!(seen("bob-pc"), of(&["bob"]));
    assert_eq!(seen("dan-pc"), vec![(name("dan"), Some(other()))]);
    assert_eq!(seen("owner-pc").len(), 4);
}
