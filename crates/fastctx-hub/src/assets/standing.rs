//! Where machines stand in syncing with this hub (design §28.9): each
//! machine's latest report, kept; a point a minute of how far behind each
//! stood in each owner library, kept a day and then folded to a point an
//! hour for thirty days; what each account's machines amount to, told in
//! their views; and an account's sync status page, reckoned when asked.
//! Nothing here opens a library to sample: each library's latest change is
//! kept as it moves, and read again only once it did.

use super::Assets;
use super::owners::Owner;
use super::scopes::{self, Scope};
use fastctx_net::directory::Member;
use fastctx_net::directory_index::Indexed;
use fastctx_storage::rusqlite::params;
use fastctx_types::assets::Reply;
use fastctx_types::sync_status::{
    AccountSync, DeviceSync, HubTotals, LagPoint, OwnerStanding, Part, SyncReport, SyncStatus, UNPULLED_NAMED,
};
use fastctx_types::{AccountName, MachineName};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The tables in the hub's control database.
pub(crate) const SCHEMA: &str = "
-- Each machine's latest report of where it stands, when it came, and when
-- the machine was last seen online.
CREATE TABLE IF NOT EXISTS sync_reports (
    machine TEXT PRIMARY KEY, device BLOB NOT NULL, report BLOB NOT NULL, at INTEGER NOT NULL, seen INTEGER
) WITHOUT ROWID;
-- How far behind each machine stood in each owner library: a point a
-- minute (grain 0) for a day, then a point an hour (grain 1) for thirty days.
CREATE TABLE IF NOT EXISTS sync_lag (
    machine TEXT NOT NULL, owner TEXT NOT NULL, grain INTEGER NOT NULL, at INTEGER NOT NULL,
    behind INTEGER NOT NULL, online INTEGER NOT NULL,
    PRIMARY KEY (machine, grain, at, owner)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS sync_lag_by_age ON sync_lag (grain, at);
";

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
/// How long the hour points are kept.
const KEPT: i64 = 30 * DAY;
/// The step of a day's line: what a page draws, not every minute.
const DAY_STEP: i64 = 5 * MINUTE;
/// The steps of the lines of an account's machines asked all at once when
/// it has more than `MANY` of them: a row's short line, a day's and longer.
const COARSE_DAY_STEP: i64 = 30 * MINUTE;
const COARSE_STEP: i64 = 4 * HOUR;
const MANY: usize = 24;
/// How far behind a machine may stand and still count as caught up: what a
/// minute's sampling cannot tell from catching up as it goes.
const CAUGHT_UP: u64 = 60_000;
/// How long a reckoned status stands before it is reckoned again.
const STATUS_STANDS: Duration = Duration::from_secs(60);
/// The owner libraries a report names at most.
const OWNERS_AT_MOST: usize = 4096;
/// How often the machines are sampled.
const EVERY: Duration = Duration::from_secs(60);
/// The points written in one write.
const POINTS_AT_ONCE: usize = 4096;
/// The statuses kept at most.
const STATUSES_AT_MOST: usize = 64;

/// A status reckoned: for the account as it was opened, over a span.
type StatusKey = (AccountName, i64, u32, Option<MachineName>);
/// A report as kept: the machine's name, its device key, the report, when
/// it came and when the machine was last seen.
type ReportRow = (String, Vec<u8>, Vec<u8>, i64, Option<i64>);

/// What the hub keeps in memory beside the tables.
#[derive(Default)]
pub(crate) struct Standing {
    /// Each owner library's latest change, as last read.
    tops: Mutex<HashMap<String, Top>>,
    /// What each account's machines amount to, as last sampled.
    summaries: Mutex<BTreeMap<AccountName, AccountSync>>,
    /// Each account's status reckoned lately, by the account as it was
    /// opened and the span (AO07); none kept once it stands no more (AO03).
    statuses: Mutex<HashMap<StatusKey, (Instant, SyncStatus)>>,
    /// What each owner library holds in all, at the change it was counted.
    totals: Mutex<HashMap<String, (u64, fastctx_content::v2::Totals)>>,
}

/// An owner library's latest change, when it was made, and the library's
/// generation; `moves` counts the moves since, so a read that crossed one
/// is read again.
#[derive(Clone)]
struct Top {
    change: u64,
    at: i64,
    /// The latest change to a stream that is no running log, and when: a
    /// machine is behind, or caught up, by it (design §28.42).
    steady: u64,
    steady_at: i64,
    generation: String,
    moves: u64,
    read_at: u64,
}

impl Standing {
    /// A stream of the library `owner` moved: its latest change is read
    /// again when next asked.
    pub(crate) fn moved(&self, owner: &str) {
        // One not read yet is marked moved all the same: a read under way
        // as it moved is read again (AO02).
        let mut tops = self.tops.lock().expect("tops");
        let top = tops.entry(owner.to_string()).or_insert_with(|| Top {
            change: 0,
            at: 0,
            steady: 0,
            steady_at: 0,
            generation: String::new(),
            moves: 0,
            read_at: u64::MAX,
        });
        top.moves += 1;
    }

    /// What the accounts logged in on `me` amount to.
    pub(crate) fn summaries_for(&self, directory: &Indexed<'_>, me: &MachineName) -> Vec<AccountSync> {
        let Some(member) = directory.member(me) else { return Vec::new() };
        let summaries = self.summaries.lock().expect("summaries");
        member.logins.iter().filter_map(|account| summaries.get(account).cloned()).collect()
    }
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// Keeps `report`, `machine`'s latest.
pub(crate) fn keep(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    device: &[u8; 32],
    mut report: SyncReport,
) -> Result<Reply, String> {
    if report.owners.len() > OWNERS_AT_MOST {
        return Err("a report names too many owner libraries".into());
    }
    report.unpulled_scopes.truncate(UNPULLED_NAMED);
    let body = serde_json::to_vec(&report).map_err(|error| error.to_string())?;
    let (name, at, device) = (directory.canonical(machine).to_string(), now(), *device);
    // The names the machine had go: its report is under its name now (AO05).
    let was: Vec<String> = directory.member(machine).map(|member| member.was.iter().map(ToString::to_string).collect()).unwrap_or_default();
    assets
        .control
        .write_now(256 + body.len(), move |c| {
            // The name now another device's, what the old one's curve said
            // goes with its report (AP11).
            c.prepare_cached(
                "DELETE FROM sync_lag WHERE machine = ?1 AND EXISTS
                 (SELECT 1 FROM sync_reports WHERE machine = ?1 AND device IS NOT NULL AND device != ?2)",
            )?
            .execute(params![name, &device[..]])?;
            c.prepare_cached(
                "INSERT INTO sync_reports (machine, device, report, at, seen) VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT (machine) DO UPDATE SET device = excluded.device, report = excluded.report, at = excluded.at, seen = excluded.at",
            )?
            .execute(params![name, &device[..], body, at])?;
            for old in &was {
                c.prepare_cached("DELETE FROM sync_reports WHERE machine = ?1")?.execute([old])?;
            }
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(Reply::Reported)
}

/// A machine's latest report, when it came, and when the machine was last
/// seen online.
struct Kept {
    report: SyncReport,
    at: i64,
    seen: Option<i64>,
}

/// Every machine's latest report, under its name now, of the machine its
/// name is now — the same key (AO06) — the latest of those kept under its
/// names (AO05); and the names whose reports are of no machine now.
fn kept(assets: &Assets, directory: &Indexed<'_>) -> io::Result<(BTreeMap<MachineName, Kept>, Vec<String>)> {
    let rows: Vec<ReportRow> = assets
        .control
        .read_now(|c| {
            let mut statement = c.prepare_cached("SELECT machine, device, report, at, seen FROM sync_reports")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .map_err(io::Error::other)?;
    let mut kept: BTreeMap<MachineName, Kept> = BTreeMap::new();
    let mut gone = Vec::new();
    for (machine, device, report, at, seen) in rows {
        let (Ok(name), Ok(report)) = (MachineName::try_from(machine.clone()), serde_json::from_slice::<SyncReport>(&report)) else {
            gone.push(machine);
            continue;
        };
        let Some(member) = directory.member(&name).filter(|member| member.key.as_bytes()[..] == device[..]) else {
            gone.push(machine);
            continue;
        };
        let now = member.name.clone();
        if kept.get(&now).is_some_and(|other| other.at >= at) {
            continue;
        }
        kept.insert(now, Kept { report, at, seen });
    }
    Ok((kept, gone))
}

/// `owner`'s latest change; none when it has no library or no change yet.
fn top(assets: &Assets, owner: &str) -> Option<Top> {
    let moves = {
        let mut tops = assets.standing.tops.lock().expect("tops");
        match tops.get(owner) {
            Some(top) if top.moves == top.read_at => return Some(top.clone()),
            Some(top) => top.moves,
            // Marked before it is read: a move while it is read leaves it
            // to be read again (AO02).
            None => {
                tops.insert(
                    owner.to_string(),
                    Top { change: 0, at: 0, steady: 0, steady_at: 0, generation: String::new(), moves: 0, read_at: u64::MAX },
                );
                0
            }
        }
    };
    let owned = Owner::from_key(owner)?;
    if !assets.has_library(&owned) {
        return None;
    }
    let library = assets.library(&owned).ok()?;
    let (change, at) = library.latest_change().ok()??;
    // A library of running logs alone holds nothing a machine is behind on.
    let (steady, steady_at) = library.latest_steady_change().ok()?.unwrap_or((0, 0));
    let generation = library.generation().ok()?;
    let mut tops = assets.standing.tops.lock().expect("tops");
    // A move while it was read leaves it to be read again.
    let now_moves = tops.get(owner).map_or(moves, |top| top.moves);
    let top = Top { change, at, steady, steady_at, generation, moves: now_moves, read_at: moves };
    tops.insert(owner.to_string(), top.clone());
    Some(top)
}

/// How far behind its library's latest change `standing` is: the running
/// logs aside, which grow every few seconds while they run, so a machine
/// away is never at their latest (design §28.42).
fn behind(top: Option<&Top>, standing: &OwnerStanding) -> u64 {
    let Some(top) = top else { return 0 };
    // Of another generation, it is as far behind as can be told; never
    // caught up for it (`Reckoned::exact`).
    if standing.generation != top.generation {
        return (top.at - standing.at).max(CAUGHT_UP as i64) as u64;
    }
    if standing.change >= top.steady {
        return 0;
    }
    (top.steady_at - standing.at).max(0) as u64
}

/// The key of `account`'s own library.
fn own_key(directory: &Indexed<'_>, account: &AccountName) -> Option<String> {
    let opened = directory.all.account(account)?.opened;
    Some(Owner::Account(account.to_string(), opened.as_millisecond()).key())
}

/// Whether where a machine stands in `owner` counts on the page: every
/// library the machine follows does. It follows only those it holds scopes
/// of, by the accounts logged in on it — another account's shared with
/// this one among them (H10) — and the page is of the machine as a whole.
fn counts(owner: &str) -> bool {
    Owner::from_key(owner).is_some()
}

/// Where a machine stands as reckoned: how far behind it is, whether it
/// stands in every library of the account that holds anything, and whether
/// it stands at the very latest change of each library it follows, in its
/// generation.
struct Reckoned {
    behind: u64,
    whole: bool,
    exact: bool,
}

/// How `kept` stands in what `account` reads.
fn reckon(assets: &Assets, directory: &Indexed<'_>, account: &AccountName, kept: &Kept) -> Reckoned {
    let mut worst = 0;
    let mut exact = true;
    for standing in &kept.report.owners {
        if counts(&standing.owner) {
            let top = top(assets, &standing.owner);
            worst = worst.max(behind(top.as_ref(), standing));
            exact &= top.is_none_or(|top| top.generation == standing.generation && standing.change >= top.steady);
        }
    }
    // The account's own library and those of its bodies, where they hold
    // anything, are stood in by each of its machines: one it never stood in
    // is not caught up, whatever its report says of the others (H10).
    let whole = libraries_of(assets, directory, account)
        .into_iter()
        .all(|owner| kept.report.owners.iter().any(|standing| standing.owner == owner) || top(assets, &owner).is_none());
    Reckoned { behind: worst, whole, exact }
}

/// How long a report of a machine online stands for where it is: it reports
/// at least every five minutes.
const FRESH: i64 = 6 * MINUTE;

/// Whether a machine is caught up: in every library it is to stand in, with
/// nothing first pulled or missing, at the very latest change of each — or,
/// online and reporting as it goes, within a minute of it, in the same
/// generation. Offline, only the very latest counts: a change made just
/// after where it stood is one it does not have, however soon after (I03).
fn synced(kept: &Kept, reckoned: &Reckoned, online: bool, now: i64) -> bool {
    let settled = reckoned.whole && kept.report.unpulled == 0 && kept.report.missing_objects == 0;
    let going = online && now - kept.at <= FRESH && reckoned.behind < CAUGHT_UP;
    settled && (reckoned.exact || going)
}

/// The members `account` is logged in on, managed machines aside.
fn machines_of<'a>(directory: &Indexed<'a>, account: &AccountName) -> Vec<&'a Member> {
    directory.all.members.iter().filter(|member| !member.managed && member.logins.contains(account)).map(|member| &**member).collect()
}

/// Samples every machine that reported, a minute at a time.
pub(crate) async fn every_minute(hub: std::sync::Weak<crate::Hub>) {
    loop {
        tokio::time::sleep(EVERY).await;
        let Some(running) = hub.upgrade() else { return };
        let log = running.log.clone();
        if let Err(error) = running.blocking(|hub| sample(&hub)).await {
            log.line(format_args!("sampling where machines stand in syncing: {error}"));
        }
    }
}

/// A minute's point for each machine that reported, in each owner library
/// it named; what is a day old folded into hours, and what is thirty days
/// old dropped; and what each account's machines amount to, told to them
/// when it changed.
fn sample(hub: &crate::Hub) -> io::Result<()> {
    let assets = &hub.assets;
    let now = now();
    let minute = now - now % MINUTE;
    let online: BTreeSet<MachineName> = hub.sessions().keys().cloned().collect();
    let store = hub.store();
    let directory = store.indexed();
    let (kept, gone) = kept(assets, &directory)?;
    let seen: Vec<String> = kept.keys().filter(|machine| online.contains(*machine)).map(ToString::to_string).collect();
    let mut points = Vec::new();
    for (machine, kept) in kept.iter().filter(|(machine, _)| directory.member(machine).is_some()) {
        let on = online.contains(machine);
        for standing in &kept.report.owners {
            let behind = behind(top(assets, &standing.owner).as_ref(), standing);
            points.push((machine.to_string(), standing.owner.clone(), behind as i64, i64::from(on)));
        }
    }
    let mut summaries = BTreeMap::new();
    for account in &directory.all.accounts {
        let (mut devices, mut caught_up, mut worst) = (0, 0, 0);
        // Every machine of the account counts, one that never reported among
        // those not caught up (I05).
        for member in machines_of(&directory, &account.name) {
            devices += 1;
            let Some(kept) = kept.get(&member.name) else { continue };
            let reckoned = reckon(assets, &directory, &account.name, kept);
            worst = worst.max(reckoned.behind);
            caught_up += u32::from(synced(kept, &reckoned, online.contains(&member.name), now));
        }
        if devices > 0 {
            summaries.insert(
                account.name.clone(),
                AccountSync { account: account.name.clone(), devices, synced: caught_up, behind: worst },
            );
        }
    }
    // The points go a batch at a time: however many machines and libraries,
    // no one write is past what the writer takes (AO04).
    for batch in points.chunks(POINTS_AT_ONCE) {
        let batch = batch.to_vec();
        assets
            .control
            .write_now(512 + batch.len() * 128, move |c| {
                let mut point = c.prepare_cached(
                    "INSERT INTO sync_lag (machine, owner, grain, at, behind, online) VALUES (?1, ?2, 0, ?3, ?4, ?5)
                     ON CONFLICT (machine, grain, at, owner) DO UPDATE SET
                     behind = MAX(behind, excluded.behind), online = MAX(online, excluded.online)",
                )?;
                for (machine, owner, behind, on) in &batch {
                    point.execute(params![machine, owner, minute, behind, on])?;
                }
                Ok(())
            })
            .map_err(io::Error::other)?;
    }
    assets
        .control
        .write_now(512 + (seen.len() + gone.len()) * 64, move |c| {
            let mut saw = c.prepare_cached("UPDATE sync_reports SET seen = ?2 WHERE machine = ?1")?;
            for machine in &seen {
                saw.execute(params![machine, now])?;
            }
            for machine in &gone {
                c.prepare_cached("DELETE FROM sync_reports WHERE machine = ?1")?.execute([machine])?;
                c.prepare_cached("DELETE FROM sync_lag WHERE machine = ?1")?.execute([machine])?;
            }
            c.prepare_cached(
                "INSERT INTO sync_lag (machine, owner, grain, at, behind, online)
                 SELECT machine, owner, 1, at - at % 3600000, MAX(behind), MAX(online) FROM sync_lag
                 WHERE grain = 0 AND at < ?1 GROUP BY machine, owner, at - at % 3600000
                 ON CONFLICT (machine, grain, at, owner) DO UPDATE SET
                 behind = MAX(behind, excluded.behind), online = MAX(online, excluded.online)",
            )?
            .execute([now - DAY])?;
            c.prepare_cached("DELETE FROM sync_lag WHERE grain = 0 AND at < ?1")?.execute([now - DAY])?;
            c.prepare_cached("DELETE FROM sync_lag WHERE grain = 1 AND at < ?1")?.execute([now - KEPT])?;
            Ok(())
        })
        .map_err(io::Error::other)?;
    // The accounts whose machines amount to something else now are told.
    let told: BTreeSet<MachineName> = {
        let mut before = assets.standing.summaries.lock().expect("summaries");
        let changed: Vec<&AccountName> = summaries
            .keys()
            .chain(before.keys())
            .filter(|account| summaries.get(*account) != before.get(*account))
            .collect();
        let told = changed
            .into_iter()
            .flat_map(|account| machines_of(&directory, account))
            .map(|member| member.name.clone())
            .collect();
        *before = summaries;
        told
    };
    drop(store);
    if !told.is_empty() {
        hub.views_changed_for(&told);
    }
    Ok(())
}

/// The part of the records a stream of `class` is.
fn part(class: u8) -> Part {
    match class {
        2 => Part::Conversations,
        3 => Part::Output,
        _ => Part::Other,
    }
}

/// Whether `machine` may hold `stream` of the scope `key`.
fn may_hold(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    key: &str,
    stream: &str,
    memo: &mut HashMap<String, Option<Scope>>,
) -> bool {
    let scope = memo.entry(key.to_string()).or_insert_with(|| scopes::scope(assets, key).ok().flatten());
    scope.as_ref().is_some_and(|scope| scopes::readable(directory, machine, scope) && scopes::holds(directory, machine, scope, stream))
}

/// What each owner library has after where a device stands, reckoned once
/// for every device that stands there: by the owner and the change it
/// stands at in the library's present generation, none when all is to come.
type LeftOf = HashMap<(String, Option<u64>), Option<Vec<fastctx_content::v2::StreamBytes>>>;

/// What `machine` has yet to take of what `account` reads, by part, and
/// whether that is only about so.
fn left(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    account: &AccountName,
    kept: &Kept,
    left_of: &mut LeftOf,
) -> (Vec<(Part, u64)>, bool) {
    let mut parts: BTreeMap<Part, u64> = BTreeMap::new();
    let mut scopes_seen: HashMap<String, Option<Scope>> = HashMap::new();
    let unpulled: BTreeSet<&str> = kept.report.unpulled_scopes.iter().map(String::as_str).collect();
    let mut about = kept.report.unpulled as usize > kept.report.unpulled_scopes.len();
    let mut owners: Vec<(String, String, u64, u64)> = kept
        .report
        .owners
        .iter()
        .filter(|standing| counts(&standing.owner))
        .map(|standing| (standing.owner.clone(), standing.generation.clone(), standing.change, standing.taken))
        .collect();
    // Its own library not stood in yet: all of it is to come.
    if let Some(own) = own_key(directory, account)
        && !owners.iter().any(|(owner, ..)| *owner == own)
    {
        owners.push((own, String::new(), 0, 0));
    }
    for (owner, generation, change, taken) in owners {
        let Some(owned) = Owner::from_key(&owner) else { continue };
        if !assets.has_library(&owned) {
            continue;
        }
        let Ok(library) = assets.library(&owned) else {
            about = true;
            continue;
        };
        // Where it stands in the library's present generation; standing at
        // its start, or in another generation, none: all of it is to come.
        let after = library.generation().is_ok_and(|now| now == generation).then_some(change).filter(|at| *at > 0);
        let rows = left_of.entry((owner.clone(), after)).or_insert_with(|| match after {
            // Caught up: nothing of the library is after where it stands, and
            // its frames are not read to say so. Read whole for every device
            // at every look, they filled the hub's disk (2026-11-19).
            Some(after) if library.latest_change().is_ok_and(|top| top.is_none_or(|(top, _)| after >= top)) => {
                Some(Vec::new())
            }
            Some(after) => library.bytes_after(after).ok(),
            // All of it to come: each stream as far as it has come, about.
            None => library.bytes_of_streams().ok(),
        });
        let Some(rows) = rows.clone() else {
            about = true;
            continue;
        };
        about |= after.is_none() && !rows.is_empty();
        let mut here: BTreeMap<Part, u64> = BTreeMap::new();
        for row in rows {
            if unpulled.contains(row.scope.as_str())
                || !may_hold(assets, directory, machine, &row.scope, &row.stream, &mut scopes_seen)
            {
                continue;
            }
            *here.entry(part(row.class)).or_default() += row.bytes;
        }
        // What it took since it last stood anew is no longer to come; how it
        // falls among the parts is only about so.
        let total: u64 = here.values().sum();
        let still = total.saturating_sub(taken);
        about |= taken > 0 && still > 0;
        for (part, bytes) in here {
            let share = if total == 0 { 0 } else { (u128::from(bytes) * u128::from(still) / u128::from(total)) as u64 };
            *parts.entry(part).or_default() += share;
        }
    }
    // Scopes not pulled whole yet: every stream of them it may hold.
    for key in unpulled {
        let Some(scope) = scopes::scope(assets, key).ok().flatten() else { continue };
        if !counts(&scope.owner.key()) {
            continue;
        }
        let Ok(rows) = scopes::library(assets, &scope).and_then(|library| {
            library.bytes_of_scopes(vec![key.to_string()]).map_err(|error| error.to_string())
        }) else {
            about = true;
            continue;
        };
        for row in rows.into_iter().filter(|row| scopes::holds(directory, machine, &scope, &row.stream)) {
            *parts.entry(part(row.class)).or_default() += row.bytes;
        }
        about = true;
    }
    (parts.into_iter().filter(|(_, bytes)| *bytes > 0).collect(), about)
}

/// The owner libraries `account` reads in all: its own, and those of the
/// bodies it is a member of.
fn libraries_of(assets: &Assets, directory: &Indexed<'_>, account: &AccountName) -> Vec<String> {
    let mut owners: Vec<String> = own_key(directory, account).into_iter().collect();
    let bodies: Vec<String> = assets
        .control
        .read_now(|c| {
            let mut statement = c.prepare_cached("SELECT id FROM asset_bodies")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap_or_default();
    for body in bodies {
        // Members are named with when their account was opened (I02).
        if assets.bodies.members(directory.all, &body).is_ok_and(|members| super::bodies::among(directory.all, &members, account)) {
            owners.push(Owner::Body(body).key());
        }
    }
    owners
}

/// What the hub holds of what `account` reads, counted again only where a
/// library changed since.
fn totals(assets: &Assets, directory: &Indexed<'_>, account: &AccountName) -> HubTotals {
    let mut sum = HubTotals::default();
    for owner in libraries_of(assets, directory, account) {
        let Some(top) = top(assets, &owner) else { continue };
        let cached = assets.standing.totals.lock().expect("totals").get(&owner).filter(|(at, _)| *at == top.change).map(|(_, totals)| totals.clone());
        let counted = match cached {
            Some(totals) => totals,
            None => {
                let Some(totals) = Owner::from_key(&owner)
                    .and_then(|owned| assets.library(&owned).ok())
                    .and_then(|library| library.totals().ok())
                else {
                    continue;
                };
                assets.standing.totals.lock().expect("totals").insert(owner.clone(), (top.change, totals.clone()));
                totals
            }
        };
        sum.conversations += counted.conversations;
        sum.files += counted.files;
        sum.file_bytes += counted.file_bytes;
        sum.record_bytes += counted.record_bytes;
    }
    sum
}

/// Each machine's line over the last `hours`: how far behind it stood in
/// what `account` reads, a point a step. Lines of many machines at once are
/// drawn coarser (`coarse`): a row's short line needs no more.
fn points(assets: &Assets, machines: &[MachineName], hours: u32, now: i64, coarse: bool) -> io::Result<(i64, Vec<LagPoint>)> {
    let since = now - i64::from(hours) * HOUR;
    let step = match (hours <= 24, coarse) {
        (true, false) => DAY_STEP,
        (true, true) => COARSE_DAY_STEP,
        (false, false) => HOUR,
        (false, true) => COARSE_STEP,
    };
    let mut points = Vec::new();
    for machine in machines {
        let name = machine.to_string();
        let rows: Vec<(String, i64, i64, i64)> = assets
            .control
            .read_now(move |c| {
                let mut statement = c.prepare_cached(
                    "SELECT owner, at, behind, online FROM sync_lag WHERE machine = ?1 AND grain IN (0, 1) AND at >= ?2",
                )?;
                let rows = statement.query_map(params![name, since], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        let mut line: BTreeMap<i64, (u64, bool)> = BTreeMap::new();
        for (owner, at, behind, online) in rows {
            if !counts(&owner) {
                continue;
            }
            let point = line.entry(at - at % step).or_insert((0, false));
            point.0 = point.0.max(behind.max(0) as u64);
            point.1 |= online != 0;
        }
        points.extend(
            line.into_iter().map(|(at, (behind, online))| LagPoint { machine: machine.clone(), at, behind, online }),
        );
    }
    Ok((step, points))
}

/// The most points the pages kept hold between them, about 24 MiB.
const POINTS_KEPT: usize = 1 << 20;

/// What a page kept weighs: its points, each device about as much as one.
fn weight(status: &SyncStatus) -> usize {
    status.points.len() + status.devices.len()
}

/// `member`'s kind as the directory writes it.
fn kind_of(member: &Member) -> String {
    serde_json::to_value(member.kind).ok().and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default()
}

/// How `account`'s machines stand over the last `hours`, asked by `from`,
/// on which the account must be logged in.
pub(crate) fn status(
    hub: &crate::Hub,
    from: &MachineName,
    account: &AccountName,
    hours: u32,
    only: Option<&MachineName>,
) -> Result<Reply, String> {
    let hours = hours.clamp(1, 24 * 30);
    let assets = &hub.assets;
    let store = hub.store();
    let directory = store.indexed();
    if !directory.logged_in(from, account) {
        return Err(format!("{account} is not logged in on {from}"));
    }
    let opened = directory.all.account(account).map_or(0, |found| found.opened.as_millisecond());
    let key = (account.clone(), opened, hours, only.cloned());
    let cached = assets.standing.statuses.lock().expect("statuses").get(&key).filter(|(at, _)| at.elapsed() < STATUS_STANDS).map(|(_, status)| status.clone());
    if let Some(status) = cached {
        // Logged out meanwhile, not even a page kept is given (AP12).
        if !hub.store().indexed().logged_in(from, account) {
            return Err(format!("{account} is not logged in on {from}"));
        }
        return Ok(Reply::SyncStatus { status: Box::new(status) });
    }
    let online: HashMap<MachineName, i64> =
        hub.sessions().iter().map(|(name, session)| (name.clone(), session.since.as_millisecond())).collect();
    let (kept, _) = kept(assets, &directory).map_err(|error| error.to_string())?;
    let now = now();
    let members = machines_of(&directory, account);
    let mut devices = Vec::new();
    let mut left_of = LeftOf::new();
    for member in &members {
        let report = kept.get(&member.name);
        let reckoned = report.map(|kept| reckon(assets, &directory, account, kept));
        let behind = reckoned.as_ref().map_or(0, |reckoned| reckoned.behind);
        let (left, about) = report.map_or((Vec::new(), false), |kept| {
            left(assets, &directory, &member.name, account, kept, &mut left_of)
        });
        let since = online.get(&member.name).copied().or_else(|| report.map(|kept| kept.seen.unwrap_or(kept.at)));
        let facts = report.map(|kept| &kept.report);
        devices.push(DeviceSync {
            machine: member.name.clone(),
            kind: kind_of(member),
            online: online.contains_key(&member.name),
            since,
            reported: report.map(|kept| kept.at),
            behind,
            left,
            about,
            unpulled: facts.map_or(0, |facts| facts.unpulled),
            unpulled_scopes: facts.map_or_else(Vec::new, |facts| {
                facts.unpulled_scopes.iter().take(fastctx_types::sync_status::UNPULLED_TOLD).cloned().collect()
            }),
            missing_objects: facts.map_or(0, |facts| facts.missing_objects),
            missing_bytes: facts.map_or(0, |facts| facts.missing_bytes),
            unsent_frames: facts.map_or(0, |facts| facts.unsent_frames),
            unsent_bytes: facts.map_or(0, |facts| facts.unsent_bytes),
            rate: facts.map_or(0, |facts| facts.rate),
            synced: report
                .zip(reckoned.as_ref())
                .is_some_and(|(kept, reckoned)| synced(kept, reckoned, online.contains_key(&member.name), now)),
        });
    }
    // One machine asked: its line alone, in full. All of them: coarser when
    // there are many.
    let machines: Vec<MachineName> =
        members.iter().map(|member| member.name.clone()).filter(|name| only.is_none_or(|only| only == name)).collect();
    let coarse = only.is_none() && machines.len() > MANY;
    let (step, points) = points(assets, &machines, hours, now, coarse).map_err(|error| error.to_string())?;
    let totals = totals(assets, &directory, account);
    let status = SyncStatus { at: now, hours, step, devices, points, totals };
    {
        // What stands no more goes, and never more than so many pages, nor
        // more than so many points between them (AO03, AP10): a page past
        // what is left is reckoned again when asked.
        let mut statuses = assets.standing.statuses.lock().expect("statuses");
        statuses.retain(|_, (at, _)| at.elapsed() < STATUS_STANDS);
        let held: usize = statuses.values().map(|(_, kept)| weight(kept)).sum();
        if statuses.len() < STATUSES_AT_MOST && held + weight(&status) <= POINTS_KEPT {
            statuses.insert(key, (Instant::now(), status.clone()));
        }
    }
    // Logged out of the machine as it was reckoned, the page is not given
    // (AO08).
    if !hub.store().indexed().logged_in(from, account) {
        return Err(format!("{account} is not logged in on {from}"));
    }
    Ok(Reply::SyncStatus { status: Box::new(status) })
}
