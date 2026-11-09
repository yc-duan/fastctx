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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use fastctx_net::directory::Level;
use fastctx_net::directory_index::Indexed;
use fastctx_net::sign::PublicKey;
use fastctx_platform::doing::Rank;
use fastctx_types::health::{Migrating, Near};
use fastctx_types::trouble::{Trouble, TroubleKind};
use fastctx_types::update::Kind;
use fastctx_types::{AccountName, MachineName};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::Hub;
use crate::locks::Mutex;
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
/// The most connections of one machine remembered.
const CONNECTIONS_KEPT: usize = 256;
/// Machines meant to stay up that went silent within `SILENT_WITHIN`, past
/// `OUTAGE_LEAST` and a tenth of all such machines, lost their line together.
const SILENT_WITHIN: Duration = Duration::from_secs(5 * 60);
const OUTAGE_LEAST: usize = 5;
/// How often what no longer counts is let go of, and how many machines are
/// meant to stay up is counted again.
const RECOUNT: Duration = Duration::from_secs(60);

/// How long one kind of trouble of one machine, or of the network, is not
/// told again.
fn quiet(kind: TroubleKind) -> SignedDuration {
    match kind {
        TroubleKind::Near => SignedDuration::from_hours(6),
        TroubleKind::Violations | TroubleKind::Damaged | TroubleKind::Migration => SignedDuration::from_hours(24),
        TroubleKind::Silent
        | TroubleKind::Drops
        | TroubleKind::Stuck
        | TroubleKind::Outage
        | TroubleKind::Hub
        | TroubleKind::Rollout
        | TroubleKind::Other => SignedDuration::from_hours(1),
    }
}

/// The longest `quiet`.
const QUIET_LONGEST: SignedDuration = SignedDuration::from_hours(24);

/// Whether a machine of `kind`, as it last reported, holds its line while it
/// is up, so that the line coming and going is told: all but a phone, whose
/// line comes and goes with it.
fn steady(kind: Option<Kind>) -> bool {
    kind != Some(Kind::Phone)
}

/// Whether a machine of `kind` is meant to stay up, so that its silence is
/// told: one with no application, such as a server. A computer someone sits
/// at, a managed one and a phone sleep, shut down or lose power at will, all
/// of which the hub hears as silence.
fn stays_up(kind: Option<Kind>) -> bool {
    kind == Some(Kind::Headless)
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
    recent: Mutex<Recent>,
}

/// What the rules remember between one event and the next.
#[derive(Default)]
struct Recent {
    /// When each kind was last told of each machine, by its key, or of the
    /// network.
    told: HashMap<(Option<PublicKey>, TroubleKind), Timestamp>,
    /// When each steady machine connected lately (`DROPS`).
    connected: HashMap<PublicKey, VecDeque<Instant>>,
    /// When machines meant to stay up went silent lately (`OUTAGE_LEAST`).
    silent: VecDeque<Instant>,
    /// How many machines are meant to stay up, as counted then.
    staying: Option<(Instant, usize)>,
    /// When `told` and `connected` last let go of what no longer counts.
    pruned: Option<Instant>,
    /// How the move of each machine's earlier records last went, by its key,
    /// as kept in `MIGRATIONS`.
    migrations: HashMap<PublicKey, Migrated>,
}

/// The rows each machine's last migration summary is kept in.
const MIGRATIONS: &str = "migrations";

/// How the move of a machine's earlier records last went, and since when it
/// stood so: kept for each machine that had anything set aside or failing,
/// so that a count gone back to nothing is on record too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Migrated {
    machine: MachineName,
    remaining: u64,
    failed: u32,
    damaged: u32,
    since: Timestamp,
}

impl Recent {
    /// Lets go of the troubles past their longest quiet and of machines not
    /// connected within the hour, at most once every `RECOUNT`: the cost
    /// does not grow with the network on every event.
    fn prune(&mut self, now: Instant, at: Timestamp) {
        if self.pruned.is_some_and(|pruned| now.duration_since(pruned) < RECOUNT) {
            return;
        }
        self.pruned = Some(now);
        self.told.retain(|_, told| told.checked_add(QUIET_LONGEST).is_ok_and(|until| until > at));
        self.connected.retain(|_, times| times.back().is_some_and(|last| now.duration_since(*last) < CONNECTED_WITHIN));
    }
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
        // One over is not quiet: should it come again, it is told.
        let told = all
            .iter()
            .filter(|kept| kept.trouble.resolved.is_none())
            .map(|kept| ((kept.key, kept.trouble.kind), kept.trouble.at))
            .collect();
        let mut migrations = HashMap::new();
        for (key, _, migrated) in shelf.rows::<Migrated>(MIGRATIONS)? {
            if let Ok(key) = key.parse::<PublicKey>() {
                migrations.insert(key, migrated);
            }
        }
        Ok(Self {
            shelf,
            kept: ArcSwap::from_pointee(Shelved::of(all)),
            next: AtomicU64::new(next),
            recent: Mutex::new("hub troubles", Rank::HUB_TROUBLES, Recent { told, migrations, ..Recent::default() }),
        })
    }

    /// Keeps how the move of `name`'s earlier records goes, as it told it,
    /// where that changed; and once nothing it set aside is left, marks what
    /// was told of them as over: looked into as it changes, and `first` in a
    /// connection, for what was told before the hub kept it.
    pub(crate) fn migrating(&self, hub: &Hub, name: &MachineName, key: PublicKey, migrating: &Migrating, first: bool) {
        let now = Timestamp::now();
        let changed = {
            let Ok(mut recent) = self.recent.lock();
            let before = recent.migrations.get(&key);
            let same = before.is_some_and(|before| {
                (before.remaining, before.failed, before.damaged)
                    == (migrating.remaining, migrating.failed, migrating.damaged)
            });
            let nothing = migrating.failed == 0 && migrating.damaged == 0;
            if same || (before.is_none() && nothing) {
                None
            } else {
                let migrated = Migrated {
                    machine: name.clone(),
                    remaining: migrating.remaining,
                    failed: migrating.failed,
                    damaged: migrating.damaged,
                    since: now,
                };
                recent.migrations.insert(key, migrated.clone());
                Some(migrated)
            }
        };
        let looked = first || changed.is_some();
        if let Some(migrated) = changed {
            let mut changes = Changes::default();
            match body(&migrated) {
                Ok(body) => changes.put.push(Put { kind: MIGRATIONS, key: key.to_string().into(), position: 0, body }),
                Err(error) => hub.log.line(format_args!("keeping {name}'s migration: {error}")),
            }
            let (shelf, log, name) = (self.shelf.clone(), hub.log.clone(), name.clone());
            fastctx_platform::task::spawn(async move {
                if let Err(error) = shelf.write_async(changes).await {
                    log.line(format_args!("keeping {name}'s migration: {error}"));
                }
            });
        }
        if migrating.damaged == 0 && looked {
            // The files it had set aside, told with how many there were;
            // a damaged store, told without a count, is not over by it.
            self.resolve(hub, key, TroubleKind::Damaged, |trouble| trouble.count.is_some());
        }
    }

    /// The machines whose earlier records last stood set aside or failing,
    /// with since when.
    pub(crate) fn migrations(&self) -> Vec<serde_json::Value> {
        let Ok(recent) = self.recent.lock();
        let mut found: Vec<&Migrated> =
            recent.migrations.values().filter(|migrated| migrated.failed > 0 || migrated.damaged > 0).collect();
        found.sort_by_key(|migrated| std::cmp::Reverse(migrated.damaged));
        found
            .into_iter()
            .take(SHOWN)
            .map(|migrated| {
                serde_json::json!({
                    "member": migrated.machine.to_string(),
                    "set_aside": migrated.damaged,
                    "failed": migrated.failed,
                    "remaining": migrated.remaining,
                    "since": migrated.since,
                })
            })
            .collect()
    }

    /// Marks what `kind` told of the machine of `key`, those `of` picks, as
    /// over, and tells those who heard of them; should it come again, it is
    /// told at once.
    fn resolve(&self, hub: &Hub, key: PublicKey, kind: TroubleKind, of: impl Fn(&Trouble) -> bool) {
        let now = Timestamp::now();
        let mut over = Vec::new();
        self.kept.rcu(|shelved| {
            over.clear();
            let all = shelved
                .all
                .iter()
                .map(|kept| {
                    if kept.key == Some(key)
                        && kept.trouble.kind == kind
                        && kept.trouble.resolved.is_none()
                        && of(&kept.trouble)
                    {
                        let mut ended = (**kept).clone();
                        ended.trouble.resolved = Some(now);
                        let ended = Arc::new(ended);
                        over.push(ended.clone());
                        ended
                    } else {
                        kept.clone()
                    }
                })
                .collect();
            Shelved::of(all)
        });
        if over.is_empty() {
            return;
        }
        {
            let Ok(mut recent) = self.recent.lock();
            recent.told.remove(&(Some(key), kind));
        }
        let store = hub.store();
        for kept in &over {
            hub.log.line(format_args!("trouble {} is over", kept.trouble.id));
            self.keep(hub, kept, Vec::new());
            self.tell(hub, kept, &store);
        }
    }

    pub(crate) fn measurements(&self) -> serde_json::Value {
        self.recent.measurements()
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
        // Rules broken, kept from an earlier release, are no trouble to tell.
        found.retain(|kept| kept.trouble.kind != TroubleKind::Violations);
        found.sort_by_key(|kept| kept.trouble.id);
        found.dedup_by_key(|kept| kept.trouble.id);
        let skip = found.len().saturating_sub(SHOWN);
        found.into_iter().skip(skip).map(|kept| kept.trouble.clone()).collect()
    }

    /// The troubles of `kinds` told of a machine since `since`: the machine's
    /// key, the kind and when.
    pub(crate) fn of_kinds_since(
        &self,
        kinds: &[TroubleKind],
        since: Timestamp,
    ) -> Vec<(PublicKey, TroubleKind, Timestamp)> {
        self.kept
            .load()
            .all
            .iter()
            .filter(|kept| kept.trouble.at >= since && kinds.contains(&kept.trouble.kind))
            .filter_map(|kept| kept.key.map(|key| (key, kept.trouble.kind, kept.trouble.at)))
            .collect()
    }

    /// `name`, last reported as `kind`, connected: a steady machine that
    /// connected again too often within the hour dropped too often.
    pub(crate) fn connected(&self, hub: &Hub, name: &MachineName, key: PublicKey, kind: Option<Kind>) {
        if !steady(kind) {
            return;
        }
        let now = Instant::now();
        let times = {
            let Ok(mut recent) = self.recent.lock();
            recent.prune(now, Timestamp::now());
            let times = recent.connected.entry(key).or_default();
            while times.front().is_some_and(|at| now.duration_since(*at) >= CONNECTED_WITHIN)
                || times.len() >= CONNECTIONS_KEPT
            {
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

    /// Nothing was heard from `name`, last reported as `kind`, for
    /// `seconds`, and its session ended: told of a machine meant to stay up;
    /// many of them together are an outage of the network.
    pub(crate) fn silent(&self, hub: &Hub, name: &MachineName, key: PublicKey, kind: Option<Kind>, seconds: u64) {
        if !stays_up(kind) {
            return;
        }
        self.raise(hub, Some((name, key)), TroubleKind::Silent, Detail { seconds: Some(seconds), ..Detail::default() });
        let now = Instant::now();
        let (together, counted) = {
            let Ok(mut recent) = self.recent.lock();
            while recent.silent.front().is_some_and(|at| now.duration_since(*at) >= SILENT_WITHIN) {
                recent.silent.pop_front();
            }
            recent.silent.push_back(now);
            let counted =
                recent.staying.filter(|(at, _)| now.duration_since(*at) < RECOUNT).map(|(_, staying)| staying);
            (recent.silent.len(), counted)
        };
        if together < OUTAGE_LEAST {
            return;
        }
        let staying = counted.unwrap_or_else(|| {
            let staying = crate::updates::reported_of_kind(hub, Kind::Headless);
            let Ok(mut recent) = self.recent.lock();
            recent.staying = Some((now, staying));
            staying
        });
        if together >= staying / 10 {
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
            let Ok(mut recent) = self.recent.lock();
            recent.prune(Instant::now(), now);
            let quiet_until = recent.told.get(&(key, kind)).and_then(|at| at.checked_add(quiet(kind)).ok());
            if quiet_until.is_some_and(|until| until > now) {
                return;
            }
            recent.told.insert((key, kind), now);
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
            version: detail.version,
            resolved: None,
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
    pub version: Option<String>,
}

fn row_key(id: u64) -> String {
    format!("{id:020}")
}
