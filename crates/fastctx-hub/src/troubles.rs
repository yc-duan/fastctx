//! What goes wrong with the network's machines and with the network itself,
//! told to those who should hear of it (`specs/world-engine.md`, 量与告警).
//!
//! A machine's trouble is told to the accounts that hold it when it happens,
//! the network's to its super administrator; who hears of one is fixed then,
//! each account with when it was opened, so a later account of the same name
//! hears nothing of it. One kind of trouble of one machine is told at most
//! once a while. Troubles are kept on disk for a week, at most `AT_MOST` of
//! them, and reach every device of those accounts with their views: a device
//! tells each once.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use fastctx_net::directory::Level;
use fastctx_net::directory_index::Indexed;
use fastctx_net::sign::PublicKey;
use fastctx_types::health::Near;
use fastctx_types::trouble::{Trouble, TroubleKind};
use fastctx_types::{AccountName, MachineName};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::Hub;
use crate::store::rows::{Changes, Put, body};
use crate::store::shelf::Shelf;

/// How long a trouble is kept.
const KEPT: SignedDuration = SignedDuration::from_hours(7 * 24);
/// The most troubles kept in all; the oldest go first.
const AT_MOST: usize = 4096;
/// The most troubles in one view, the newest.
const SHOWN: usize = 64;
/// The rows they are kept in.
const ROWS: &str = "troubles";

/// A machine that connected again more than this many times within
/// `CONNECTED_WITHIN` dropped too often.
const DROPS: usize = 3;
const CONNECTED_WITHIN: Duration = Duration::from_secs(60 * 60);
/// Machines that went silent within `SILENT_WITHIN`, past `OUTAGE_LEAST` and
/// a tenth of the network, lost their line together.
const SILENT_WITHIN: Duration = Duration::from_secs(5 * 60);
const OUTAGE_LEAST: usize = 5;

/// How long one kind of trouble of one machine, or of the network, is not
/// told again.
fn quiet(kind: TroubleKind) -> SignedDuration {
    match kind {
        TroubleKind::Near => SignedDuration::from_hours(6),
        TroubleKind::Violations => SignedDuration::from_hours(24),
        TroubleKind::Silent
        | TroubleKind::Drops
        | TroubleKind::Stuck
        | TroubleKind::Outage
        | TroubleKind::Hub
        | TroubleKind::Other => SignedDuration::from_hours(1),
    }
}

/// A trouble as it is kept.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Kept {
    trouble: Trouble,
    /// The machine's key, for a trouble of a machine: one of the same name
    /// later is another machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<PublicKey>,
    /// Who hears of it: each account, with when it was opened.
    told: Vec<(AccountName, Timestamp)>,
}

/// The troubles kept, oldest first, and the same by each account that hears
/// of them.
#[derive(Default)]
struct Shelved {
    all: Vec<Arc<Kept>>,
    by_account: HashMap<AccountName, Vec<Arc<Kept>>>,
}

impl Shelved {
    fn of(all: Vec<Arc<Kept>>) -> Self {
        let mut by_account: HashMap<AccountName, Vec<Arc<Kept>>> = HashMap::new();
        for kept in &all {
            for (account, _) in &kept.told {
                by_account.entry(account.clone()).or_default().push(kept.clone());
            }
        }
        Self { all, by_account }
    }
}

pub(crate) struct Troubles {
    shelf: Shelf,
    kept: ArcSwap<Shelved>,
    next: AtomicU64,
    /// When each kind was last told of each machine, by its key, or of the
    /// network.
    told: Mutex<HashMap<(Option<PublicKey>, TroubleKind), Timestamp>>,
    /// When each machine connected lately (`DROPS`).
    connected: Mutex<HashMap<PublicKey, VecDeque<Instant>>>,
    /// When machines went silent lately (`OUTAGE_LEAST`).
    silent: Mutex<VecDeque<Instant>>,
}

impl Troubles {
    /// The troubles kept in `shelf`; those older than a week are let go.
    pub(crate) fn open(shelf: Shelf) -> std::io::Result<Self> {
        let now = Timestamp::now();
        let mut all = Vec::new();
        let mut gone = Vec::new();
        for (key, _, kept) in shelf.rows::<Kept>(ROWS)? {
            if kept.trouble.at.checked_add(KEPT).is_ok_and(|until| until > now) {
                all.push(Arc::new(kept));
            } else {
                gone.push(key);
            }
        }
        all.sort_by_key(|kept| kept.trouble.id);
        while all.len() > AT_MOST {
            gone.push(row_key(all.remove(0).trouble.id));
        }
        if !gone.is_empty() {
            let mut changes = Changes::default();
            changes.deleted.extend(gone.into_iter().map(|key| (ROWS, Arc::<str>::from(key))));
            shelf.write(changes)?;
        }
        let next = all.last().map_or(1, |kept| kept.trouble.id + 1);
        let told = all.iter().map(|kept| ((kept.key, kept.trouble.kind), kept.trouble.at)).collect();
        Ok(Self {
            shelf,
            kept: ArcSwap::from_pointee(Shelved::of(all)),
            next: AtomicU64::new(next),
            told: Mutex::new(told),
            connected: Mutex::default(),
            silent: Mutex::default(),
        })
    }

    /// The troubles `me`'s view tells, newest last: those told to an account
    /// logged in on it, as it was opened then.
    pub(crate) fn for_member(&self, directory: &Indexed<'_>, me: &MachineName) -> Vec<Trouble> {
        let Some(member) = directory.member(me) else { return Vec::new() };
        let kept = self.kept.load();
        let mut found: Vec<&Arc<Kept>> = Vec::new();
        for name in &member.logins {
            let Some(account) = directory.account(name).filter(|account| !account.locked) else { continue };
            found.extend(
                kept.by_account
                    .get(name)
                    .into_iter()
                    .flatten()
                    .filter(|kept| kept.told.iter().any(|(told, opened)| told == name && *opened == account.opened)),
            );
        }
        found.sort_by_key(|kept| kept.trouble.id);
        found.dedup_by_key(|kept| kept.trouble.id);
        let skip = found.len().saturating_sub(SHOWN);
        found.into_iter().skip(skip).map(|kept| kept.trouble.clone()).collect()
    }

    /// `name` connected: one that connected again too often within the hour
    /// dropped too often.
    pub(crate) fn connected(&self, hub: &Hub, name: &MachineName, key: PublicKey) {
        let now = Instant::now();
        let times = {
            let mut connected = self.connected.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            connected.retain(|_, times| times.back().is_some_and(|at| now.duration_since(*at) < CONNECTED_WITHIN));
            let times = connected.entry(key).or_default();
            while times.front().is_some_and(|at| now.duration_since(*at) >= CONNECTED_WITHIN) {
                times.pop_front();
            }
            times.push_back(now);
            times.len()
        };
        if times > DROPS + 1 {
            let count = u32::try_from(times - 1).unwrap_or(u32::MAX);
            self.raise(hub, Some((name, key)), TroubleKind::Drops, Detail { count: Some(count), ..Detail::default() });
        }
    }

    /// Nothing was heard from `name` for `seconds`, and its session ended:
    /// told of a machine meant to stay up; many of them together are an
    /// outage of the network.
    pub(crate) fn silent(&self, hub: &Hub, name: &MachineName, key: PublicKey, stays_up: bool, seconds: u64) {
        if stays_up {
            self.raise(
                hub,
                Some((name, key)),
                TroubleKind::Silent,
                Detail { seconds: Some(seconds), ..Detail::default() },
            );
        }
        let now = Instant::now();
        let together = {
            let mut silent = self.silent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            while silent.front().is_some_and(|at| now.duration_since(*at) >= SILENT_WITHIN) {
                silent.pop_front();
            }
            silent.push_back(now);
            silent.len()
        };
        let members = hub.store().book.directory.members.len();
        if together >= OUTAGE_LEAST.max(members / 10) {
            let count = u32::try_from(together).unwrap_or(u32::MAX);
            self.raise(hub, None, TroubleKind::Outage, Detail { count: Some(count), ..Detail::default() });
        }
    }

    /// Tells `kind` of `machine`, or of the network, unless it was told a
    /// while ago already.
    pub(crate) fn raise(
        &self,
        hub: &Hub,
        machine: Option<(&MachineName, PublicKey)>,
        kind: TroubleKind,
        detail: Detail,
    ) {
        let now = Timestamp::now();
        let key = machine.map(|(_, key)| key);
        {
            let mut told = self.told.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let quiet_until = told.get(&(key, kind)).and_then(|at| at.checked_add(quiet(kind)).ok());
            if quiet_until.is_some_and(|until| until > now) {
                return;
            }
            told.insert((key, kind), now);
        }
        let store = hub.store();
        let directory = &store.book.directory;
        let recipients: Vec<AccountName> = match machine {
            Some((name, _)) => {
                let name = directory.canonical(name);
                let owner = directory.member(name).and_then(|member| member.home.clone());
                let mut recipients: Vec<AccountName> = owner.into_iter().collect();
                recipients.extend(
                    directory.shares.iter().filter(|share| share.machine == *name).map(|share| share.account.clone()),
                );
                recipients
            }
            None => directory
                .accounts
                .iter()
                .filter(|account| account.level == Level::Super)
                .map(|account| account.name.clone())
                .collect(),
        };
        let mut told: Vec<(AccountName, Timestamp)> = recipients
            .into_iter()
            .filter_map(|name| directory.account(&name).map(|account| (name, account.opened)))
            .collect();
        told.sort();
        told.dedup();
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let trouble = Trouble {
            id,
            at: now,
            machine: machine.map(|(name, _)| directory.canonical(name).clone()),
            kind,
            count: detail.count,
            seconds: detail.seconds,
            near: detail.near,
        };
        hub.log.line(format_args!(
            "trouble {}: {:?}{}",
            id,
            trouble.kind,
            trouble.machine.as_ref().map_or(String::new(), |machine| format!(" of {machine}"))
        ));
        let kept = Arc::new(Kept { trouble, key, told });
        let mut gone = Vec::new();
        self.kept.rcu(|shelved| {
            let mut all = Vec::with_capacity(shelved.all.len() + 1);
            let mut dropped = Vec::new();
            for old in &shelved.all {
                if old.trouble.at.checked_add(KEPT).is_ok_and(|until| until > now) {
                    all.push(old.clone());
                } else {
                    dropped.push(old.trouble.id);
                }
            }
            all.push(kept.clone());
            let over = all.len().saturating_sub(AT_MOST);
            dropped.extend(all.drain(..over).map(|old| old.trouble.id));
            gone = dropped;
            Shelved::of(all)
        });
        self.keep(hub, &kept, gone);
        self.tell(hub, &kept, &store);
    }

    /// Writes `kept` down, and lets go of the rows of `gone`.
    fn keep(&self, hub: &Hub, kept: &Kept, gone: Vec<u64>) {
        let mut changes = Changes::default();
        match body(kept) {
            Ok(body) => changes.put.push(Put {
                kind: ROWS,
                key: row_key(kept.trouble.id).into(),
                position: i64::try_from(kept.trouble.id).unwrap_or(i64::MAX),
                body,
            }),
            Err(error) => hub.log.line(format_args!("keeping trouble {}: {error}", kept.trouble.id)),
        }
        changes.deleted.extend(gone.into_iter().map(|id| (ROWS, Arc::<str>::from(row_key(id)))));
        let (shelf, log, id) = (self.shelf.clone(), hub.log.clone(), kept.trouble.id);
        fastctx_platform::task::spawn(async move {
            if let Err(error) = shelf.write_async(changes).await {
                log.line(format_args!("keeping trouble {id}: {error}"));
            }
        });
    }

    /// Tells the sessions of the devices logged in to an account that hears
    /// of `kept` what changed in their views.
    fn tell(&self, hub: &Hub, kept: &Kept, store: &crate::store::Snapshot) {
        let directory = &store.book.directory;
        let listening: Vec<MachineName> = directory
            .members
            .iter()
            .filter(|member| member.logins.iter().any(|login| kept.told.iter().any(|(account, _)| account == login)))
            .map(|member| member.name.clone())
            .collect();
        let sessions: Vec<_> = {
            let sessions = hub.sessions();
            listening
                .iter()
                .filter_map(|name| {
                    sessions
                        .get(name)
                        .filter(|session| session.view_deltas)
                        .map(|session| (name.clone(), session.key, session.generation, session.out.clone()))
                })
                .collect()
        };
        if sessions.is_empty() {
            return;
        }
        let Some(hub) = hub.this.get().and_then(std::sync::Weak::upgrade) else { return };
        fastctx_platform::task::spawn_blocking(move || hub.push_views(sessions));
    }
}

/// What a trouble says beside its kind.
#[derive(Default)]
pub(crate) struct Detail {
    pub count: Option<u32>,
    pub seconds: Option<u64>,
    pub near: Option<Near>,
}

fn row_key(id: u64) -> String {
    format!("{id:020}")
}
