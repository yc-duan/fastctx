//! What a Claude Code login's week is reckoned to hold, which shares and its
//! Fable line are parts of (`usage-weekly-reset-detection`, 2026-11-19). Its
//! owner says what a week is expected to hold and how far a week's figure may
//! stand from that; each week's figure is what the week before came to, the
//! ledger's quota dollars between its first reading and its last over how far
//! the reading rose, held within that reach. Nothing is looked for in the
//! readings beyond that: a week whose reading rose too little says nothing
//! and the figure before stands. A week's usage is reset by hand alone, by
//! scaling what everyone used of it until then; the ledger keeps what was
//! used as it was.
//!
//! Kept beside the ledger and not in it: a row a node of before does not know
//! is one it takes for damage (`quarantine`). Every read and write of it is
//! made under the ledger's lock (`super::with_book`), and a file that cannot
//! be read is never written over: what it holds may be resets in force.

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
/// set no other, in percent either way (用户 2026-11-19: 默认波动10%).
const SWING: u8 = 10;
/// The widest an owner may set it (用户 2026-11-19: 下限0上限20).
pub(crate) const WIDEST: u8 = 20;
/// The most a week may be expected to hold, $10,000,000: what is reckoned of
/// it stays well within its integers.
pub(crate) const MOST: Usd = Usd(10_000_000 * 1_000_000);
/// How far a week's reading must rise for the week to say what it came to,
/// in tenths of a percent: less, and a tenth of a point of rounding moves the
/// figure by more than the reach.
const ROSE: u32 = 250;
/// The weeks looked back over for what they came to: the last four whole.
const BACK: i64 = 5;
const WEEKS_KEPT: usize = 26;
/// The resets kept of weeks that are over; this week's are all kept.
const RESETS_KEPT: usize = 60;
/// How soon a week before that could not be measured is tried again, and for
/// how long into the week.
const RETRY: SignedDuration = SignedDuration::from_hours(1);
const TRYING: SignedDuration = SignedDuration::from_hours(24);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    swing: Option<u8>,
    /// The start of the week the weeks before were last measured for, and the
    /// vendor's account the login was in then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    through: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    through_account: Option<String>,
    /// When the week before this one was last looked for and not found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tried: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    weeks: Vec<Measured>,
    /// Oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    resets: Vec<Made>,
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

/// A reset made by hand, with the vendor's account whose week it was.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Made {
    #[serde(flatten)]
    told: UsageHandReset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<String>,
}

fn read(path: &Path) -> io::Result<Kept> {
    match fastctx_platform::disk::read(path.join(FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{FILE}: {error}"))),
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

/// Keeps how far a week's figure may stand from what its owner expects; to
/// be called under the ledger's lock.
pub(super) fn set_swing(path: &Path, swing: u8) -> io::Result<()> {
    let mut kept = read(path)?;
    kept.swing = Some(swing);
    write(path, &kept)
}

fn same(a: Timestamp, b: Timestamp) -> bool {
    super::readings::same(a, b)
}

/// The vendor's account the login is in now, as the ledger last recorded.
fn current(book: &Book) -> Option<String> {
    book.accounts.iter().max_by_key(|(at, _)| *at).map(|(_, account)| account.clone())
}

/// `part` of `whole` in tenths of a percent.
fn tenths(part: Usd, whole: Usd) -> u32 {
    (u128::from(part.0) * 1000 / u128::from(whole.0.max(1))).min(u128::from(u32::MAX)) as u32
}

/// A week of a login as it is reckoned now.
pub(super) struct Reckoned {
    pub week: Week,
    /// What its owner expects a week to hold.
    pub base: Usd,
    pub swing: u8,
    /// This week's figure.
    pub figure: Usd,
    /// Why what is kept beside the ledger could not be read: no reset is in
    /// force meanwhile, and none is made.
    pub trouble: Option<String>,
    account: Option<String>,
    /// Each reset of this week in force, with what the use until it counts
    /// as of what it was.
    cuts: Vec<(Timestamp, f64)>,
    kept: Kept,
}

impl Reckoned {
    /// What a use made at `at` counts as of what it was: less where the
    /// week's usage was reset after it. What the ledger sums by the hour is
    /// cut at each reset and stands at the start of its stretch, so a use
    /// at the very moment of a reset is the stretch after it, as here.
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
    /// whole, the resets to one who may make them.
    pub fn told(&self, whole: bool, may_reset: bool) -> UsageReckoning {
        let ours = |account: &Option<String>| *account == self.account;
        UsageReckoning {
            base: self.base,
            swing: self.swing,
            week: self.figure,
            weeks: if whole {
                self.kept.weeks.iter().filter(|one| ours(&one.account)).map(|one| one.week.clone()).collect()
            } else {
                Vec::new()
            },
            resets: (whole && may_reset).then(|| {
                self.kept.resets.iter().rev().filter(|made| ours(&made.account)).map(|made| made.told.clone()).collect()
            }),
        }
    }
}

/// The figure `came_to` gives within the reach of `base`.
fn within(base: Usd, swing: u8, came_to: Option<Usd>) -> Usd {
    let reach = |percent: u64| Usd((u128::from(base.0) * u128::from(percent) / 100).min(u128::from(u64::MAX)) as u64);
    let (low, high) = (reach(100u64.saturating_sub(u64::from(swing))), reach(100 + u64::from(swing)));
    came_to.map_or(base, |came_to| Usd(came_to.0.clamp(low.0, high.0)))
}

/// The login at `path` as it is reckoned at `now`, from what is kept beside
/// its ledger; nothing is measured or written (`measure_due`).
pub(super) fn of(path: &Path, book: &Book, agent: &str, now: Timestamp) -> Reckoned {
    let week = super::readings::week(book.reading.as_ref(), now);
    let base = super::week_at(path).unwrap_or(super::allowance::DEFAULT_WEEK).min(MOST);
    let (kept, trouble) = match read(path) {
        Ok(kept) => (kept, None),
        Err(error) => (Kept::default(), Some(error.to_string())),
    };
    let swing = kept.swing.unwrap_or(SWING).min(WIDEST);
    let account = current(book);
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
        .filter(|made| week.window && made.account == account && made.told.undone.is_none())
        .filter(|made| same(made.told.week_end, week.end) && made.told.from.0 > 0)
        .map(|made| (made.told.at, made.told.to.0 as f64 / made.told.from.0 as f64))
        .collect();
    Reckoned { week, base, swing, figure: within(base, swing, came_to), trouble, account, cuts, kept }
}

/// Measures the weeks before this one where this week has not been measured
/// for the account the login is in: once a week, or again within a day of
/// its start where the week before could not be found yet. When nothing is
/// due until, where it is known, and whether anything was measured.
pub(super) fn measure_due(
    path: &Path,
    book: &Book,
    agent: &str,
    now: Timestamp,
) -> io::Result<(Option<Timestamp>, bool)> {
    let week = super::readings::week(book.reading.as_ref(), now);
    if agent != "claude" || !week.window {
        return Ok((None, false));
    }
    let account = current(book);
    let mut kept = read(path)?;
    if kept.through.is_some_and(|through| same(through, week.start)) && kept.through_account == account {
        return Ok((Some(week.end), false));
    }
    if let Some(tried) = kept.tried.filter(|tried| *tried >= week.start && now < *tried + RETRY) {
        return Ok((Some(tried + RETRY), false));
    }
    let measured = measure(path, book, week, &account, now)?;
    // The week before said what it came to; one it did not, its readings
    // too few as yet, is looked for again while more of them may come.
    let before = measured.iter().any(|one| same(one.week.end, week.start) && one.week.came_to.is_some());
    for new in measured {
        kept.weeks.retain(|old| !(old.account == new.account && same(old.week.end, new.week.end)));
        kept.weeks.push(new);
    }
    kept.weeks.sort_by_key(|one| (one.week.end, one.week.start));
    while kept.weeks.len() > WEEKS_KEPT {
        // The newest week that said something, of each account, stays: it is
        // what the next weeks are held to while the weeks after say nothing.
        let newest = |at: usize| {
            let one = &kept.weeks[at];
            one.week.came_to.is_some()
                && !kept.weeks[at + 1..]
                    .iter()
                    .any(|later| later.account == one.account && later.week.came_to.is_some())
        };
        let Some(at) = (0..kept.weeks.len()).find(|at| !newest(*at)) else { break };
        kept.weeks.remove(at);
    }
    let until = if before || now >= week.start + TRYING {
        kept.through = Some(week.start);
        kept.through_account = account;
        kept.tried = None;
        week.end
    } else {
        kept.tried = Some(now);
        now + RETRY
    };
    write(path, &kept)?;
    Ok((Some(until), true))
}

/// What each of the weeks before `week` came to, as the ledger's days and
/// readings still tell it: each week `account` read, by the reset its
/// readings named, from its first reading to its last, and what was used
/// meanwhile while the login was in that account.
fn measure(
    path: &Path,
    book: &Book,
    week: Week,
    account: &Option<String>,
    now: Timestamp,
) -> io::Result<Vec<Measured>> {
    let span = SignedDuration::from_secs(WEEK_SECONDS);
    let since = week.start - SignedDuration::from_secs(BACK * WEEK_SECONDS);
    let since = Timestamp::from_second(since.as_second().div_euclid(86400) * 86400).expect("an earlier day");
    let history = book.reset_history();
    // Read apart from the days the book keeps for its reports, which hold
    // the whole year and would be let go for these few weeks.
    let mut days = super::history::History::default();
    let entries = days.entries(path, since, now, book.reading.as_ref(), &history, &[])?;
    let mut owners: Vec<(Timestamp, String)> = entries
        .iter()
        .filter_map(|row| match row {
            Entry::Account { at, account } => Some((*at, account.clone())),
            _ => None,
        })
        .collect();
    owners.extend(book.accounts.iter().cloned());
    owners.sort();
    owners.dedup();
    let owner_at = |at: Timestamp| {
        owners.iter().rev().find(|(since, _)| *since <= at).or(owners.first()).map(|(_, key)| key.clone())
    };
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
                (read_as == *account && over && *at < end && end - span >= since).then_some((
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
    // first reading and its last, and where the login changed account.
    let mut cut: Vec<Timestamp> =
        read.chunk_by(|a, b| same(a.0, b.0)).flat_map(|read| [read[0].1, read[read.len() - 1].1]).collect();
    cut.extend(owners.iter().map(|(at, _)| *at).filter(|at| *at >= since));
    let entries = days.entries(path, since, now, book.reading.as_ref(), &history, &cut)?;
    let mut measured = Vec::new();
    for read in read.chunk_by(|a, b| same(a.0, b.0)) {
        let (end, first, low) = read[0];
        let (_, last, high) = read[read.len() - 1];
        let spent = entries
            .iter()
            .filter(|row| !matches!(row, Entry::Reading { .. } | Entry::Account { .. }))
            .filter(|row| row.at() >= first && row.at() < last && owner_at(row.at()) == *account)
            .flat_map(Entry::uses)
            .filter_map(|(_, usage)| {
                let usd = usage.usd?;
                Some(if fastctx_hosts::claude::pricing::is_fable(&usage.model) { quota_of_fable(usd) } else { usd })
            })
            .fold(Usd::ZERO, Usd::saturating_add);
        let rise_tenths = high.saturating_sub(low);
        let came_to = (rise_tenths >= ROSE)
            .then(|| Usd((u128::from(spent.0) * 1000 / u128::from(rise_tenths)).min(u128::from(u64::MAX)) as u64));
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

/// What everyone has used of this week until `now` as it counts now, in
/// quota dollars, while the login was in the account it is in: the
/// ledger's charges and what its meters hold not yet booked.
fn used(book: &Book, reckoned: &Reckoned, now: Timestamp) -> Usd {
    let booked = book.recent.iter().map(|charge| (charge.at, quota(charge)));
    let held = book.balances().into_iter().flat_map(|row| {
        let at = row.at();
        row.uses().into_iter().filter_map(move |(_, usage)| Some((at, usage.usd?)))
    });
    booked
        .chain(held)
        .filter(|(at, _)| *at >= reckoned.week.start && *at < now && book.account_at(*at) == reckoned.account)
        .fold(Usd::ZERO, |sum, (at, usd)| sum.saturating_add(reckoned.counted(at, usd)))
}

fn refused(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, why.into())
}

/// A reset of a week's usage as asked for.
pub(crate) struct Asked<'a> {
    /// What its maker named it: given again, it is made once.
    pub id: &'a str,
    /// What everyone's use counts as from then on, in tenths of a percent of
    /// the week's figure; none takes the reset `id` back.
    pub to_tenths: Option<u32>,
    /// The end of the week its maker looked at: a week that has turned
    /// since is not reset.
    pub week_end: Option<Timestamp>,
    /// The account that made it.
    pub by: Option<String>,
}

/// Resets what everyone has used of this week of the login at `path`, or
/// takes such a reset back: the newest, of this week. A reset made already
/// is not made again. What it refuses is an `InvalidInput` error.
pub(super) fn reset(path: &Path, book: &Book, agent: &str, now: Timestamp, asked: Asked<'_>) -> io::Result<()> {
    if agent != "claude" {
        return Err(refused("only a Claude Code login's week is reckoned in dollars"));
    }
    if asked.id.is_empty() || asked.id.len() > 80 {
        return Err(refused("a reset is named by up to 80 characters"));
    }
    let reckoned = of(path, book, agent, now);
    if let Some(trouble) = &reckoned.trouble {
        return Err(io::Error::other(format!("reading what the week is reckoned to hold: {trouble}")));
    }
    let week = reckoned.week;
    if !week.window {
        return Err(refused("the login's week has not been read yet"));
    }
    if asked.week_end.is_some_and(|end| !same(end, week.end)) {
        return Err(refused("the login's week has turned since it was looked at; look at it again"));
    }
    let mut kept = reckoned.kept.clone();
    let account = reckoned.account.clone();
    let made = kept.resets.iter().position(|made| made.told.id == asked.id);
    match asked.to_tenths {
        Some(to_tenths) => {
            if let Some(made) = made {
                // Made already: given again, the same is answered; another
                // part asked under its name is not that reset.
                return if kept.resets[made].told.to_tenths == to_tenths {
                    Ok(())
                } else {
                    Err(refused("a reset of that name was made to another part of the week"))
                };
            }
            let figure = reckoned.figure;
            let used = used(book, &reckoned, now);
            let to = Usd((u128::from(figure.0) * u128::from(to_tenths) / 1000).min(u128::from(u64::MAX)) as u64);
            if to >= used {
                let stands = tenths(used, figure);
                return Err(refused(format!(
                    "the week's usage stands at {}.{}% of its figure, and is reset only to less",
                    stands / 10,
                    stands % 10
                )));
            }
            kept.resets.push(Made {
                told: UsageHandReset {
                    id: asked.id.to_string(),
                    at: now,
                    week_end: week.end,
                    figure,
                    from: used,
                    to,
                    from_tenths: tenths(used, figure),
                    to_tenths,
                    by: asked.by,
                    undone: None,
                },
                account,
            });
            // Those of a week not over, of any account, are all kept: one let
            // go would count again, and its name could be made again.
            while kept.resets.len() > RESETS_KEPT {
                let Some(at) = kept.resets.iter().position(|made| made.told.week_end <= now) else { break };
                kept.resets.remove(at);
            }
        }
        None => {
            let Some(made) = made else { return Err(refused("there is no such reset")) };
            let this = &kept.resets[made];
            if this.told.undone.is_some() {
                return Ok(());
            }
            if !same(this.told.week_end, week.end) || this.account != account {
                return Err(refused("the week that reset was made in is over"));
            }
            let later = kept.resets[made + 1..].iter().any(|later| {
                later.account == account && later.told.undone.is_none() && same(later.told.week_end, week.end)
            });
            if later {
                return Err(refused("only the newest reset is taken back"));
            }
            kept.resets[made].told.undone = Some(now);
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

    /// A reading of the week that ends at `end`, used `tenths`, as `account`
    /// read it.
    fn read_as(path: &Path, at: Timestamp, end: Timestamp, tenths: u32, account: Option<&str>) {
        let week = LimitWindow {
            name: "seven_day".into(),
            minutes: Some(10_080),
            used_tenths: tenths,
            resets_at: Some(end),
            since: None,
        };
        row(path, &Entry::Reading { at, windows: vec![week], account: account.map(Into::into), resets: Vec::new() });
    }

    fn reading(path: &Path, at: Timestamp, end: Timestamp, tenths: u32) {
        read_as(path, at, end, tenths, None);
    }

    fn book(path: &Path, now: Timestamp) -> Book {
        let mut book = Book::default();
        book.load_at(path, now).unwrap();
        book
    }

    /// The login reckoned at `now`, its weeks before measured first.
    fn reckoned(path: &Path, book: &Book, agent: &str, now: Timestamp) -> Reckoned {
        measure_due(path, book, agent, now).unwrap();
        of(path, book, agent, now)
    }

    fn dollars(usd: Usd) -> f64 {
        (usd.0 as f64 / 10_000.0).round() / 100.0
    }

    fn asked(id: &str, to_tenths: Option<u32>) -> Asked<'_> {
        Asked { id, to_tenths, week_end: None, by: Some("ann".into()) }
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
        let book = book(path, now);

        // Nothing is measured until it is due: the figure is what is expected.
        assert_eq!(dollars(of(path, &book, "claude", now).figure), 3000.0);
        // $2,050 of quota over sixty points is $3,416.67 a week: above the
        // reach of the default $3,000, ten of a hundred either way.
        let reckoned = reckoned(path, &book, "claude", now);
        assert_eq!((reckoned.week.end, reckoned.week.window), (end, true));
        assert_eq!((dollars(reckoned.base), reckoned.swing, dollars(reckoned.figure)), (3000.0, 10, 3300.0));
        let told = reckoned.told(true, true);
        assert_eq!(told.weeks.len(), 1);
        let measured = &told.weeks[0];
        assert_eq!((dollars(measured.spent), measured.rise_tenths), (2050.0, 600));
        assert_eq!(measured.came_to, Some(Usd(3_416_666_666)));
        // Measured once a week: due again when the week ends.
        assert_eq!(measure_due(path, &book, "claude", now + HOUR).unwrap(), (Some(end), false));
        // One who sees their own part is told the three figures alone.
        let own = reckoned.told(false, false);
        assert_eq!((own.base, own.swing, own.week), (told.base, told.swing, told.week));
        assert!(own.weeks.is_empty() && own.resets.is_none() && told.resets == Some(Vec::new()));

        // A wider reach, none at all, then another expectation: the same
        // week before, held within what its owner says now.
        set_swing(path, 20).unwrap();
        assert_eq!(of(path, &book, "claude", now).figure, Usd(3_416_666_666));
        set_swing(path, 0).unwrap();
        assert_eq!(dollars(of(path, &book, "claude", now).figure), 3000.0);
        set_swing(path, 10).unwrap();
        fastctx_platform::disk::write(path.join("week"), b"5000000000").unwrap();
        assert_eq!(dollars(of(path, &book, "claude", now).figure), 4500.0);
        fastctx_platform::disk::write(path.join("week"), b"3400000000").unwrap();
        assert_eq!(of(path, &book, "claude", now).figure, Usd(3_416_666_666));

        // Codex keeps no dollars: its week is what its owner set.
        assert_eq!(dollars(of(path, &book, "codex", now).figure), 3400.0);
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
        let book = book(path, now);
        let reckoned = reckoned(path, &book, "claude", now);
        let weeks = reckoned.told(true, true).weeks;
        assert_eq!(weeks.iter().map(|week| week.came_to).collect::<Vec<_>>(), vec![Some(Usd(2_900_000_000)), None]);
        assert_eq!(dollars(reckoned.figure), 2900.0);

        // A week whose reading fell on the way is taken from end to end: the
        // reach holds what that makes of it.
        let fallen = tempfile::tempdir().unwrap();
        reading(fallen.path(), start - DAY * 7 + HOUR, start, 0);
        cost(fallen.path(), start - DAY * 5, "ann", "claude-opus-5-5", 1800);
        reading(fallen.path(), start - DAY * 4, start, 600);
        reading(fallen.path(), start - DAY * 4 + HOUR, start, 20);
        cost(fallen.path(), start - DAY * 2, "ann", "claude-opus-5-5", 900);
        reading(fallen.path(), start - HOUR, start, 320);
        reading(fallen.path(), start + HOUR, end, 5);
        let book = self::book(fallen.path(), now);
        let reckoned = self::reckoned(fallen.path(), &book, "claude", now);
        assert_eq!(reckoned.told(true, true).weeks[0].rise_tenths, 320);
        assert_eq!(dollars(reckoned.figure), 3300.0);

        // With no week before to measure, it is looked for again within the
        // hour on the week's first day, and then left; the figure meanwhile
        // is what is expected.
        let empty = tempfile::tempdir().unwrap();
        reading(empty.path(), start + HOUR, end, 5);
        let book = self::book(empty.path(), start + HOUR * 2);
        let (until, measured) = measure_due(empty.path(), &book, "claude", start + HOUR * 2).unwrap();
        assert_eq!((until, measured), (Some(start + HOUR * 3), true));
        assert_eq!(measure_due(empty.path(), &book, "claude", start + HOUR * 2).unwrap(), (until, false));
        assert_eq!(measure_due(empty.path(), &book, "claude", start + DAY * 2).unwrap(), (Some(end), true));
        assert_eq!(dollars(of(empty.path(), &book, "claude", now).figure), 3000.0);
    }

    #[test]
    fn another_accounts_weeks_and_use_are_not_this_ones() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let end = now + DAY * 3;
        let start = end - DAY * 7;
        // The week before: account a rose fifty points over $1,500, and the
        // login was in account b for a day meanwhile, which spent $500.
        row(path, &Entry::Account { at: start - DAY * 8, account: "a".into() });
        read_as(path, start - DAY * 7 + HOUR, start, 0, Some("a"));
        cost(path, start - DAY * 6, "ann", "claude-opus-5-5", 1500);
        row(path, &Entry::Account { at: start - DAY * 4, account: "b".into() });
        read_as(path, start - DAY * 4 + HOUR, start + HOUR * 5, 10, Some("b"));
        cost(path, start - DAY * 4 + HOUR * 2, "ann", "claude-opus-5-5", 500);
        row(path, &Entry::Account { at: start - DAY * 3, account: "a".into() });
        read_as(path, start - HOUR, start, 500, Some("a"));
        read_as(path, start + HOUR, end, 3, Some("a"));
        let book = book(path, now);
        let reckoned = reckoned(path, &book, "claude", now);
        let weeks = reckoned.told(true, true).weeks;
        assert_eq!(weeks.len(), 1);
        assert_eq!((dollars(weeks[0].spent), weeks[0].came_to), (1500.0, Some(Usd(3_000_000_000))));

        // A reset of account a's week is nothing to account b's.
        reset(path, &book, "claude", now, asked("a-reset", Some(0))).unwrap_err();
        cost(path, now - HOUR, "ann", "claude-opus-5-5", 100);
        let book = self::book(path, now);
        reset(path, &book, "claude", now, asked("a-reset", Some(0))).unwrap();
        assert_eq!(of(path, &book, "claude", now).cuts(), vec![now]);
        row(path, &Entry::Account { at: now + HOUR, account: "b".into() });
        let book = self::book(path, now + HOUR * 2);
        let other = of(path, &book, "claude", now + HOUR * 2);
        assert!(other.cuts().is_empty() && other.told(true, true).resets == Some(Vec::new()));
    }

    #[test]
    fn a_reset_by_hand_scales_what_was_used_until_then() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let end = ledger(path, now);
        let book = book(path, now);
        // $550 of quota used of a figure of $3,300: 16.7 of a hundred.
        let before = reckoned(path, &book, "claude", now);
        assert_eq!(dollars(used(&book, &before, now)), 550.0);
        let refusal = reset(path, &book, "claude", now, asked("r1", Some(167))).unwrap_err();
        assert_eq!(refusal.kind(), io::ErrorKind::InvalidInput);
        // Asked of a week that has turned since, it is refused.
        let stale = Asked { week_end: Some(end - DAY * 7), ..asked("r0", Some(80)) };
        assert_eq!(reset(path, &book, "claude", now, stale).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        let looked = Asked { week_end: Some(end), ..asked("r1", Some(87)) };
        reset(path, &book, "claude", now, looked).unwrap();
        // Given again, it is made once.
        reset(path, &book, "claude", now + HOUR, asked("r1", Some(87))).unwrap();
        let after = of(path, &book, "claude", now + HOUR);
        assert_eq!(after.cuts(), vec![now]);
        // To the cent what was asked: 8.7 of a hundred of $3,300.
        assert_eq!(dollars(used(&book, &after, now + HOUR)), 287.1);
        let made = &after.told(true, true).resets.unwrap()[0];
        assert_eq!((dollars(made.figure), dollars(made.from), dollars(made.to)), (3300.0, 550.0, 287.1));
        assert_eq!((made.from_tenths, made.to_tenths, made.by.as_deref()), (166, 87, Some("ann")));
        // What was used before the reset counts as it; at it and after it,
        // whole; and the week before not at all.
        assert_eq!(after.counted(now - SignedDuration::from_secs(1), Usd(550)), Usd(287));
        assert_eq!(after.counted(now, Usd(100)), Usd(100));
        assert_eq!(after.counted(now + HOUR, Usd(100)), Usd(100));
        // Its name asked for another part is not that reset.
        let other = reset(path, &book, "claude", now + HOUR, asked("r1", Some(20))).unwrap_err();
        assert_eq!(other.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(after.counted(after.week.start - DAY, Usd(100)), Usd(100));
        // Only one who may make resets is told them.
        assert!(after.told(true, false).resets.is_none());

        // A second, to nothing: what stood at 8.7 counts as none.
        reset(path, &book, "claude", now + HOUR, asked("r2", Some(0))).unwrap();
        let cleared = of(path, &book, "claude", now + HOUR * 2);
        assert_eq!(used(&book, &cleared, now + HOUR * 2), Usd::ZERO);
        // Only the newest is taken back; then the one before it.
        let refusal = reset(path, &book, "claude", now + HOUR * 2, asked("r1", None)).unwrap_err();
        assert_eq!(refusal.kind(), io::ErrorKind::InvalidInput);
        assert!(reset(path, &book, "claude", now + HOUR * 2, asked("none", None)).is_err());
        reset(path, &book, "claude", now + HOUR * 2, asked("r2", None)).unwrap();
        reset(path, &book, "claude", now + HOUR * 2, asked("r2", None)).unwrap();
        let halved = of(path, &book, "claude", now + HOUR * 2);
        assert_eq!(dollars(used(&book, &halved, now + HOUR * 2)), 287.1);

        // However many are made and taken back since, a reset of this week
        // in force stays.
        for at in 0..70 {
            let id = format!("again-{at}");
            reset(path, &book, "claude", now + HOUR * 2, asked(&id, Some(1))).unwrap();
            reset(path, &book, "claude", now + HOUR * 2, asked(&id, None)).unwrap();
        }
        assert_eq!(dollars(used(&book, &of(path, &book, "claude", now + HOUR * 3), now + HOUR * 3)), 287.1);
        reset(path, &book, "claude", now + HOUR * 3, asked("r1", None)).unwrap();
        let undone = of(path, &book, "claude", now + HOUR * 3);
        assert_eq!(dollars(used(&book, &undone, now + HOUR * 3)), 550.0);

        // A reset of the week before is nothing to the week after it.
        reset(path, &book, "claude", now + HOUR * 3, asked("r3", Some(0))).unwrap();
        let next = now + DAY * 4;
        reading(path, next - HOUR, undone.week.end + DAY * 7, 3);
        let book = self::book(path, next);
        let reckoned = of(path, &book, "claude", next);
        assert!(reckoned.cuts().is_empty());
        assert!(reset(path, &book, "claude", next, asked("r3", None)).is_err());
    }

    #[test]
    fn what_cannot_be_read_is_never_written_over() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        ledger(path, now);
        let book = book(path, now);
        reckoned(path, &book, "claude", now);
        reset(path, &book, "claude", now, asked("r1", Some(0))).unwrap();
        let kept = fastctx_platform::disk::read(path.join(FILE)).unwrap();
        let damaged = &kept[..kept.len() / 2];
        fastctx_platform::disk::write(path.join(FILE), damaged).unwrap();
        // No reset is in force meanwhile, none is made, nothing is measured,
        // and the file stays as it is for whoever looks at it.
        let reckoned = of(path, &book, "claude", now + HOUR);
        assert!(reckoned.trouble.is_some() && reckoned.cuts().is_empty());
        assert!(reset(path, &book, "claude", now + HOUR, asked("r2", Some(0))).is_err());
        assert!(measure_due(path, &book, "claude", now + DAY * 8).is_err());
        assert!(set_swing(path, 5).is_err());
        assert_eq!(fastctx_platform::disk::read(path.join(FILE)).unwrap(), damaged);
    }
}
