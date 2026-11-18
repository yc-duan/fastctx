//! What a Claude Code login's week is reckoned to hold, which shares and its
//! Fable line are parts of (`usage-weekly-reset-detection`, 2026-11-19). Its
//! owner says what a week is expected to hold and how far a week's figure may
//! stand from that; each week's figure is what the week before came to, the
//! ledger's quota dollars between its first reading and its last over how far
//! the reading rose, held within that reach, and stands all week. Nothing is
//! looked for in the readings beyond that: a week whose reading rose too
//! little says nothing and the figure before stands. A week's usage is reset
//! by hand alone, by scaling what everyone used of it until then; the ledger
//! keeps what was used as it was.
//!
//! Kept beside the ledger and not in it: a row a node of before does not know
//! is one it takes for damage (`quarantine`).

use super::ledger::{Book, Entry};
use super::recent::Charge;
use super::resets::WEEK_MINUTES;
use super::{WEEK_SECONDS, Week};
use fastctx_types::usage::{UsageHandReset, UsageMeasured, UsageReckoning, Usd};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

const FILE: &str = "reckoning.json";
/// How far a week's figure may stand from what its owner expects where they
/// set no other, in percent either way (用户 2026-11-19: 波动幅度默认取±5%).
const SWING: u8 = 5;
/// The widest an owner may set it: the figure moves within a small reach.
pub(crate) const WIDEST: u8 = 20;
/// How far a week's reading must rise for the week to say what it came to,
/// in tenths of a percent: less, and a tenth of a point of rounding moves the
/// figure by more than the reach.
const ROSE: u32 = 250;
/// A reading this much lower than the one before it, in tenths of a
/// percent, is the vendor clearing the week, not two answers heard out of
/// their order.
const FELL: u32 = 50;
/// The weeks looked back over for what they came to: the last four whole.
const BACK: i64 = 5;
const WEEKS_KEPT: usize = 26;
const RESETS_KEPT: usize = 60;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    swing: Option<u8>,
    /// The start of the week the weeks before were last measured for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    through: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    weeks: Vec<Measured>,
    /// Oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    resets: Vec<UsageHandReset>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Measured {
    #[serde(flatten)]
    week: UsageMeasured,
    /// The vendor's account the login was in: another account's weeks say
    /// nothing of this one's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<String>,
}

fn read(path: &Path) -> io::Result<Kept> {
    match fastctx_platform::disk::read(path.join(FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Kept::default()),
        Err(error) => Err(error),
    }
}

fn write(path: &Path, kept: &Kept) -> io::Result<()> {
    fastctx_platform::disk::create_dir_all(path)?;
    let bytes = serde_json::to_vec(kept).map_err(io::Error::other)?;
    fastctx_platform::publish(&path.join(FILE), &bytes, fastctx_platform::Permissions::Private)
}

/// How far a week's figure may stand from what the owner of the login at
/// `path` expects, as they set it.
pub(super) fn swing(path: &Path) -> Option<u8> {
    read(path).ok()?.swing
}

/// Keeps how far a week's figure may stand from what its owner expects.
pub(super) fn set_swing(path: &Path, swing: u8) -> io::Result<()> {
    let mut kept = read(path)?;
    kept.swing = Some(swing);
    write(path, &kept)
}

fn same(a: Timestamp, b: Timestamp) -> bool {
    super::readings::same(a, b)
}

/// A week of a login as it is reckoned now.
pub(super) struct Reckoned {
    pub week: Week,
    /// What its owner expects a week to hold.
    pub base: Usd,
    pub swing: u8,
    /// This week's figure.
    pub figure: Usd,
    /// Each reset of this week in force, with what the use before it counts
    /// as of what it was.
    cuts: Vec<(Timestamp, f64)>,
    kept: Kept,
}

impl Reckoned {
    /// What a use made at `at` counts as of what it was: less where the
    /// week's usage was reset after it.
    pub fn scale(&self, at: Timestamp) -> f64 {
        if at < self.week.start {
            return 1.0;
        }
        self.cuts.iter().filter(|(cut, _)| at < *cut).map(|(_, part)| part).product()
    }

    /// When each reset in force was made: what is summed is cut there.
    pub fn cuts(&self) -> Vec<Timestamp> {
        self.cuts.iter().map(|(at, _)| *at).collect()
    }

    /// What `usd` used at `at` counts as.
    pub fn counted(&self, at: Timestamp, usd: Usd) -> Usd {
        let scale = self.scale(at);
        if scale == 1.0 { usd } else { Usd((usd.0 as f64 * scale).round() as u64) }
    }

    /// As a reader is told it: the weeks measured to one who sees the login
    /// whole, the resets to its owner.
    pub fn told(&self, whole: bool, owner: bool) -> UsageReckoning {
        UsageReckoning {
            base: self.base,
            swing: self.swing,
            week: self.figure,
            weeks: if whole {
                self.kept.weeks.iter().map(|measured| measured.week.clone()).collect()
            } else {
                Vec::new()
            },
            resets: (whole && owner).then(|| self.kept.resets.iter().rev().cloned().collect()),
        }
    }
}

/// The figure `came_to` gives within the reach of `base`.
fn within(base: Usd, swing: u8, came_to: Option<Usd>) -> Usd {
    let reach = |percent: u64| Usd((u128::from(base.0) * u128::from(percent) / 100) as u64);
    let (low, high) = (reach(100 - u64::from(swing.min(100))), reach(100 + u64::from(swing)));
    came_to.map_or(base, |came_to| Usd(came_to.0.clamp(low.0, high.0)))
}

/// The login at `path` as it is reckoned at `now`. The weeks before this one
/// are measured the first time it is asked in a week, and kept.
pub(super) fn of(path: &Path, book: &mut Book, agent: &str, now: Timestamp) -> Reckoned {
    let week = super::readings::week(book.reading.as_ref(), now);
    let base = super::week_at(path).unwrap_or(super::allowance::DEFAULT_WEEK);
    let mut kept = read(path).unwrap_or_default();
    if agent == "claude" && week.window && !kept.through.is_some_and(|through| same(through, week.start)) {
        // A week that cannot be measured now is asked again the next time.
        if let Ok(measured) = measure(path, book, week, now) {
            let fresh = |one: &Measured| measured.iter().any(|new| same(new.week.end, one.week.end));
            kept.weeks.retain(|one| !fresh(one));
            kept.weeks.extend(measured);
            kept.weeks.sort_by_key(|one| (one.week.end, one.week.start));
            let extra = kept.weeks.len().saturating_sub(WEEKS_KEPT);
            kept.weeks.drain(..extra);
            kept.through = Some(week.start);
            let _ = write(path, &kept);
        }
    }
    let swing = kept.swing.unwrap_or(SWING).min(WIDEST);
    let account = book.accounts.iter().max_by_key(|(at, _)| *at).map(|(_, account)| account.clone());
    // Only Claude Code keeps dollars: another agent's week is what its
    // owner expects.
    let came_to = kept
        .weeks
        .iter()
        .rev()
        .filter(|one| agent == "claude" && one.account == account)
        .filter(|one| one.week.end <= week.start + SignedDuration::from_secs(120))
        .find_map(|one| one.week.came_to);
    let cuts = kept
        .resets
        .iter()
        .filter(|reset| {
            week.window && reset.undone.is_none() && same(reset.week_end, week.end) && reset.from_tenths > 0
        })
        .map(|reset| (reset.at, f64::from(reset.to_tenths) / f64::from(reset.from_tenths)))
        .collect();
    Reckoned { week, base, swing, figure: within(base, swing, came_to), cuts, kept }
}

/// What each of the weeks before `week` came to, as the ledger's days and
/// readings still tell it: each week the account the login is in now read,
/// by the reset its readings named, from its first reading to its last. A
/// week whose reading fell on the way says nothing: the vendor cleared it,
/// and what the ledger kept and what the reading rose by are of different
/// stretches.
fn measure(path: &Path, book: &Book, week: Week, now: Timestamp) -> io::Result<Vec<Measured>> {
    let span = SignedDuration::from_secs(WEEK_SECONDS);
    let since = week.start - SignedDuration::from_secs(BACK * WEEK_SECONDS);
    let since = Timestamp::from_second(since.as_second().div_euclid(86400) * 86400).expect("an earlier day");
    let history = book.reset_history();
    // Read apart from the days the book keeps for its reports, which hold
    // the whole year and would be let go for these few weeks.
    let mut days = super::history::History::default();
    let entries = days.entries(path, since, now, book.reading.as_ref(), &history, &[])?;
    let mut owners: Vec<(Timestamp, &String)> = entries
        .iter()
        .filter_map(|row| match row {
            Entry::Account { at, account } => Some((*at, account)),
            _ => None,
        })
        .collect();
    owners.sort_by_key(|(at, _)| *at);
    let owner_at = |at: Timestamp| {
        owners.iter().rev().find(|(since, _)| *since <= at).or(owners.first()).map(|(_, key)| (*key).clone())
    };
    let account = book.accounts.iter().max_by_key(|(at, _)| *at).map(|(_, account)| account.clone());
    // Each reading of a week that is over: the reset it named, when, and
    // how much of the week was used.
    let mut read: Vec<(Timestamp, Timestamp, u32)> = entries
        .iter()
        .filter_map(|row| match row {
            Entry::Reading { at, windows, account: read_as, .. } => {
                let window = super::readings::window(windows, WEEK_MINUTES)?;
                let end = window.resets_at?;
                let read_as = read_as.clone().or_else(|| owner_at(*at));
                let over = end <= week.start + SignedDuration::from_secs(120);
                (read_as == account && over && *at < end && end - span >= since).then_some((
                    end,
                    *at,
                    window.used_tenths,
                ))
            }
            _ => None,
        })
        .collect();
    read.sort_by_key(|(end, at, _)| (*end, *at));
    // What was used is summed by the hour: read again cut at each week's
    // first reading and its last, so that what lies between them is exact.
    let cut: Vec<Timestamp> =
        read.chunk_by(|a, b| same(a.0, b.0)).flat_map(|read| [read[0].1, read[read.len() - 1].1]).collect();
    let entries = days.entries(path, since, now, book.reading.as_ref(), &history, &cut)?;
    let mut measured = Vec::new();
    for read in read.chunk_by(|a, b| same(a.0, b.0)) {
        let (end, first, low) = read[0];
        let (_, last, high) = read[read.len() - 1];
        let spent = entries
            .iter()
            .filter(|row| !matches!(row, Entry::Reading { .. }) && row.at() >= first && row.at() < last)
            .flat_map(Entry::uses)
            .filter_map(|(_, usage)| {
                let usd = usage.usd?;
                Some(if fastctx_hosts::claude::pricing::is_fable(&usage.model) { quota_of_fable(usd) } else { usd })
            })
            .fold(Usd::ZERO, Usd::saturating_add);
        let fell = read.windows(2).any(|pair| pair[0].2 >= pair[1].2.saturating_add(FELL));
        let rise_tenths = high.saturating_sub(low);
        let came_to =
            (!fell && rise_tenths >= ROSE).then(|| Usd((u128::from(spent.0) * 1000 / u128::from(rise_tenths)) as u64));
        measured.push(Measured {
            week: UsageMeasured { start: end - span, end, spent, rise_tenths, came_to },
            account: account.clone(),
        });
    }
    Ok(measured)
}

/// What Fable's dollars used of the quota: two and a half times its price
/// (用户 2026-11-19: fable 写死 2.5).
pub(super) fn quota_of_fable(usd: Usd) -> Usd {
    Usd(usd.0.saturating_mul(5) / 2)
}

/// What a request used of the quota, in dollars: its API price, as Claude
/// Code reckons it, and for Fable two and a half times that (用户
/// 2026-11-15: fable是2.5倍; 其他模型严格按照api价格).
pub(super) fn quota(charge: &Charge) -> Usd {
    if charge.fable { quota_of_fable(charge.usd) } else { charge.usd }
}

/// What everyone has used of this week as it counts now, in quota dollars:
/// the ledger's charges and what its meters hold not yet booked.
fn used(book: &Book, reckoned: &Reckoned, now: Timestamp) -> Usd {
    let booked = book.recent.iter().map(|charge| (charge.at, quota(charge)));
    let held = book.balances().into_iter().flat_map(|row| {
        let at = row.at();
        row.uses().into_iter().filter_map(move |(_, usage)| Some((at, usage.usd?)))
    });
    booked
        .chain(held)
        .filter(|(at, _)| *at >= reckoned.week.start && *at <= now)
        .fold(Usd::ZERO, |sum, (at, usd)| sum.saturating_add(reckoned.counted(at, usd)))
}

fn refused(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, why.into())
}

/// Resets what everyone has used of this week of the login at `path` to
/// `to_tenths` of the week's figure, or with none takes the reset `id` back:
/// the newest one, of this week. A reset made already is not made again.
pub(super) fn reset(
    path: &Path,
    book: &mut Book,
    agent: &str,
    now: Timestamp,
    id: &str,
    to_tenths: Option<u32>,
) -> io::Result<()> {
    if agent != "claude" {
        return Err(refused("only a Claude Code login's week is reckoned in dollars"));
    }
    if id.is_empty() || id.len() > 80 {
        return Err(refused("a reset is named by up to 80 characters"));
    }
    let reckoned = of(path, book, agent, now);
    if !reckoned.week.window {
        return Err(refused("the login's week has not been read yet"));
    }
    // What is kept is read anew: measuring may have written it meanwhile.
    let mut kept = read(path)?;
    let made = kept.resets.iter().position(|reset| reset.id == id);
    match to_tenths {
        Some(to_tenths) => {
            if made.is_some() {
                return Ok(());
            }
            let used = used(book, &reckoned, now);
            let from_tenths =
                (u128::from(used.0) * 1000 / u128::from(reckoned.figure.0.max(1))).min(u128::from(u32::MAX)) as u32;
            if to_tenths >= from_tenths {
                return Err(refused(format!(
                    "the week's usage stands at {}.{}% of its figure, and is reset only to less",
                    from_tenths / 10,
                    from_tenths % 10
                )));
            }
            kept.resets.push(UsageHandReset {
                id: id.to_string(),
                at: now,
                week_end: reckoned.week.end,
                from_tenths,
                to_tenths,
                undone: None,
            });
            let extra = kept.resets.len().saturating_sub(RESETS_KEPT);
            kept.resets.drain(..extra);
        }
        None => {
            let Some(made) = made else { return Err(refused("there is no such reset")) };
            if kept.resets[made].undone.is_some() {
                return Ok(());
            }
            if !same(kept.resets[made].week_end, reckoned.week.end) {
                return Err(refused("the week that reset was made in is over"));
            }
            if kept.resets[made + 1..].iter().any(|later| later.undone.is_none()) {
                return Err(refused("only the newest reset is taken back"));
            }
            kept.resets[made].undone = Some(now);
        }
    }
    write(path, &kept)
}

#[cfg(test)]
mod tests {
    use super::super::ledger::day_file;
    use super::*;
    use fastctx_types::access::HubAccount;
    use fastctx_types::agents::LimitWindow;

    const HOUR: SignedDuration = SignedDuration::from_hours(1);
    const DAY: SignedDuration = SignedDuration::from_hours(24);

    fn row(path: &Path, entry: &Entry) {
        use std::io::Write as _;
        let file = day_file(path, entry.usage_at());
        let mut file =
            fastctx_platform::disk::open_with(std::fs::OpenOptions::new().create(true).append(true), file).unwrap();
        let mut bytes = serde_json::to_vec(entry).unwrap();
        bytes.push(b'\n');
        file.write_all(&bytes).unwrap();
    }

    fn who(name: &str) -> Option<HubAccount> {
        Some(HubAccount { hub: "w-4ecf5e28c4da2e66".parse().unwrap(), account: name.parse().unwrap() })
    }

    fn cost(path: &Path, at: Timestamp, name: &str, model: &str, dollars: u64) {
        row(path, &Entry::Cost { at, who: who(name), model: model.into(), usd: Usd(dollars * 1_000_000) });
    }

    /// A reading of the week that ends at `end`, used `tenths`.
    fn reading(path: &Path, at: Timestamp, end: Timestamp, tenths: u32) {
        let week = LimitWindow {
            name: "seven_day".into(),
            minutes: Some(10_080),
            used_tenths: tenths,
            resets_at: Some(end),
            since: None,
        };
        row(path, &Entry::Reading { at, windows: vec![week], account: None, resets: Vec::new() });
    }

    fn book(path: &Path, now: Timestamp) -> Book {
        let mut book = Book::default();
        book.load_at(path, now).unwrap();
        book
    }

    fn dollars(usd: Usd) -> f64 {
        usd.0 as f64 / 1_000_000.0
    }

    /// The week before rose sixty points over $1,800 of other models and
    /// $100 of Fable; this week ann used $300 and $40 of Fable, bob $150.
    fn ledger(path: &Path, now: Timestamp) -> Timestamp {
        let end = now + DAY * 3;
        let (before, start) = (end - DAY * 14, end - DAY * 7);
        reading(path, before + HOUR, start, 20);
        for day in 0..6 {
            cost(path, before + DAY * day + HOUR * 5, "ann", "claude-opus-5-5", 300);
        }
        cost(path, before + DAY * 3, "bob", "claude-fable-5-2", 100);
        reading(path, start - HOUR, start, 620);
        // Past the week's last reading: no part of what it came to.
        cost(path, start - SignedDuration::from_mins(20), "bob", "claude-opus-5-5", 999);
        reading(path, start + HOUR, end, 10);
        cost(path, start + DAY, "ann", "claude-opus-5-5", 300);
        cost(path, start + DAY * 2, "bob", "claude-opus-5-5", 150);
        cost(path, start + DAY * 3, "ann", "claude-fable-5-2", 40);
        reading(path, now - HOUR, end, 200);
        end
    }

    #[test]
    fn a_weeks_figure_is_what_the_week_before_came_to_within_reach() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let end = ledger(path, now);
        let mut book = book(path, now);

        // $2,050 of quota over sixty points is $3,416.67 a week: above the
        // reach of the default $3,000, five of a hundred either way.
        let reckoned = of(path, &mut book, "claude", now);
        assert_eq!((reckoned.week.end, reckoned.week.window), (end, true));
        assert_eq!((dollars(reckoned.base), reckoned.swing, dollars(reckoned.figure)), (3000.0, 5, 3150.0));
        let told = reckoned.told(true, true);
        assert_eq!(told.weeks.len(), 1);
        let measured = &told.weeks[0];
        assert_eq!((dollars(measured.spent), measured.rise_tenths), (2050.0, 600));
        assert_eq!(measured.came_to, Some(Usd(3_416_666_666)));
        // One who sees their own part is told the three figures alone.
        let own = reckoned.told(false, false);
        assert_eq!((own.base, own.swing, own.week), (told.base, told.swing, told.week));
        assert!(own.weeks.is_empty() && own.resets.is_none() && told.resets == Some(Vec::new()));

        // A wider reach, then another expectation: the same week before,
        // held within what its owner says now.
        set_swing(path, 10).unwrap();
        assert_eq!(dollars(of(path, &mut book, "claude", now).figure), 3300.0);
        fastctx_platform::disk::write(path.join("week"), b"5000000000").unwrap();
        assert_eq!(dollars(of(path, &mut book, "claude", now).figure), 4500.0);
        fastctx_platform::disk::write(path.join("week"), b"3400000000").unwrap();
        assert_eq!(of(path, &mut book, "claude", now).figure, Usd(3_416_666_666));

        // Codex keeps no dollars: its week is what its owner set.
        assert_eq!(dollars(of(path, &mut book, "codex", now).figure), 3400.0);
    }

    #[test]
    fn a_week_that_rose_too_little_leaves_the_figure_before() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let end = now + DAY * 3;
        let start = end - DAY * 7;
        // Two weeks back came to $2,900; the week before rose twenty points.
        reading(path, start - DAY * 14 + HOUR, start - DAY * 7, 0);
        cost(path, start - DAY * 10, "ann", "claude-opus-5-5", 1450);
        reading(path, start - DAY * 7 - HOUR, start - DAY * 7, 500);
        reading(path, start - DAY * 7 + HOUR, start, 0);
        cost(path, start - DAY * 3, "ann", "claude-opus-5-5", 900);
        reading(path, start - HOUR, start, 200);
        reading(path, start + HOUR, end, 5);
        let mut book = book(path, now);
        let reckoned = of(path, &mut book, "claude", now);
        let weeks = reckoned.told(true, true).weeks;
        assert_eq!(weeks.iter().map(|week| week.came_to).collect::<Vec<_>>(), vec![Some(Usd(2_900_000_000)), None]);
        assert_eq!(dollars(reckoned.figure), 2900.0);
        // A week whose reading fell on the way says nothing either, however
        // far it rose from end to end.
        let fallen = tempfile::tempdir().unwrap();
        reading(fallen.path(), start - DAY * 7 + HOUR, start, 0);
        cost(fallen.path(), start - DAY * 5, "ann", "claude-opus-5-5", 1800);
        reading(fallen.path(), start - DAY * 4, start, 600);
        reading(fallen.path(), start - DAY * 4 + HOUR, start, 20);
        cost(fallen.path(), start - DAY * 2, "ann", "claude-opus-5-5", 900);
        reading(fallen.path(), start - HOUR, start, 320);
        reading(fallen.path(), start + HOUR, end, 5);
        let mut book = self::book(fallen.path(), now);
        let reckoned = of(fallen.path(), &mut book, "claude", now);
        let weeks = reckoned.told(true, true).weeks;
        assert_eq!((weeks.len(), weeks[0].rise_tenths, weeks[0].came_to), (1, 320, None));
        assert_eq!(dollars(reckoned.figure), 3000.0);
        // With no week that says anything, the figure is what is expected.
        let empty = tempfile::tempdir().unwrap();
        reading(empty.path(), start + HOUR, end, 5);
        let mut book = self::book(empty.path(), now);
        assert_eq!(dollars(of(empty.path(), &mut book, "claude", now).figure), 3000.0);
    }

    #[test]
    fn a_reset_by_hand_scales_what_was_used_until_then() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        ledger(path, now);
        let mut book = book(path, now);
        // $550 of quota used of a figure of $3,150: 17.4 of a hundred.
        let before = of(path, &mut book, "claude", now);
        assert_eq!(dollars(used(&book, &before, now)), 550.0);
        let refusal = reset(path, &mut book, "claude", now, "r1", Some(174)).unwrap_err();
        assert_eq!(refusal.kind(), io::ErrorKind::InvalidInput);
        reset(path, &mut book, "claude", now, "r1", Some(87)).unwrap();
        // Given again, it is made once.
        reset(path, &mut book, "claude", now + HOUR, "r1", Some(10)).unwrap();
        let after = of(path, &mut book, "claude", now + HOUR);
        assert_eq!(after.cuts(), vec![now]);
        assert_eq!(dollars(used(&book, &after, now + HOUR)), 275.0);
        assert_eq!(after.counted(now - DAY, Usd(100)), Usd(50));
        // What is used after it counts whole, and the week before not at all.
        assert_eq!(after.counted(now, Usd(100)), Usd(100));
        assert_eq!(after.counted(after.week.start - DAY, Usd(100)), Usd(100));
        let told = after.told(true, true).resets.unwrap();
        assert_eq!((told.len(), told[0].from_tenths, told[0].to_tenths), (1, 174, 87));

        // A second, to nothing: what stood at 8.7 counts as none.
        reset(path, &mut book, "claude", now + HOUR, "r2", Some(0)).unwrap();
        let cleared = of(path, &mut book, "claude", now + HOUR * 2);
        assert_eq!(used(&book, &cleared, now + HOUR * 2), Usd::ZERO);
        assert_eq!(cleared.told(true, true).resets.unwrap()[0].from_tenths, 87);
        // Only the newest is taken back; then the one before it.
        let refusal = reset(path, &mut book, "claude", now + HOUR * 2, "r1", None).unwrap_err();
        assert_eq!(refusal.kind(), io::ErrorKind::InvalidInput);
        assert!(reset(path, &mut book, "claude", now + HOUR * 2, "none", None).is_err());
        reset(path, &mut book, "claude", now + HOUR * 2, "r2", None).unwrap();
        reset(path, &mut book, "claude", now + HOUR * 2, "r2", None).unwrap();
        let halved = of(path, &mut book, "claude", now + HOUR * 2);
        assert_eq!(dollars(used(&book, &halved, now + HOUR * 2)), 275.0);
        reset(path, &mut book, "claude", now + HOUR * 2, "r1", None).unwrap();
        let undone = of(path, &mut book, "claude", now + HOUR * 2);
        assert_eq!(dollars(used(&book, &undone, now + HOUR * 2)), 550.0);
        assert!(undone.cuts().is_empty() && undone.told(true, true).resets.unwrap().len() == 2);
        // A reset of the week before is nothing to the week after it.
        reset(path, &mut book, "claude", now + HOUR * 2, "r3", Some(0)).unwrap();
        let next = now + DAY * 4;
        reading(path, next - HOUR, undone.week.end + DAY * 7, 3);
        let mut book = self::book(path, next);
        let reckoned = of(path, &mut book, "claude", next);
        assert!(reckoned.cuts().is_empty());
        assert!(reset(path, &mut book, "claude", next, "r3", None).is_err());
    }
}
