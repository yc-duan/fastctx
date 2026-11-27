//! What a Claude Code login's vendor read of the week it is in, kept as the
//! ledger is read (`Book`), and what that says of how much the week holds:
//! the quota dollars used while its readings rose, over how far they rose
//! (`reckoning::of`). The weeks before are measured the same way, from the
//! days they were read in (`reckoning::measure`).

use super::Week;
use super::ledger::Book;
use super::readings::{same, window};
use super::resets::WEEK_MINUTES;
use fastctx_types::agents::LimitWindow;
use fastctx_types::usage::Usd;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How far a reading must fall below the highest of its stretch, in tenths
/// of a point, for its vendor to have given the week's use back there: less
/// is readings of sessions that saw the window a moment apart.
const FELL: u32 = 50;
/// How many readings from a fall on must all stay that far down for it to
/// be one: a single stale reading falls too, and the next ones rise again.
const FELL_FOR: usize = 10;
/// The most readings of a week kept, far beyond the few thousand of a week
/// in steady use: the oldest go first.
const KEPT_AT_MOST: usize = 100_000;

/// One account's 7-day readings of one week, `(at, tenths)` in the order
/// they were read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct WeekRead {
    pub end: Timestamp,
    pub points: Vec<(Timestamp, u32)>,
}

/// Takes the 7-day window of `windows`, read for `account` at `at`, into
/// what is read of its week: a reading of a later week starts that week, one
/// of an earlier week is behind it and taken nowhere.
pub(super) fn take(
    read: &mut HashMap<Option<String>, WeekRead>,
    account: Option<String>,
    at: Timestamp,
    windows: &[LimitWindow],
) {
    let Some(weekly) = window(windows, WEEK_MINUTES) else { return };
    let Some(end) = weekly.resets_at else { return };
    if at >= end {
        return;
    }
    let point = (at, weekly.used_tenths);
    match read.get_mut(&account) {
        Some(kept) if same(kept.end, end) => {
            let place = kept.points.partition_point(|(seen, _)| *seen <= at);
            kept.points.insert(place, point);
            if kept.points.len() > KEPT_AT_MOST {
                kept.points.remove(0);
            }
        }
        Some(kept) if end < kept.end => {}
        _ => {
            read.insert(account, WeekRead { end, points: vec![point] });
        }
    }
}

/// A week's readings, `(at, tenths)` in the order they came, in stretches:
/// split where a reading fell `FELL` below the highest before it and the
/// next readings stayed down, as where the vendor gave the week's use back.
pub(super) fn stretches(points: &[(Timestamp, u32)]) -> Vec<&[(Timestamp, u32)]> {
    let mut stretches = Vec::new();
    let (mut from, mut high) = (0, 0u32);
    for at in 0..points.len() {
        let down = |tenths: u32| tenths.saturating_add(FELL) <= high;
        if at > from && points[at..(at + FELL_FOR).min(points.len())].iter().all(|(_, tenths)| down(*tenths)) {
            stretches.push(&points[from..at]);
            from = at;
            high = 0;
        }
        high = high.max(points[at].1);
    }
    if from < points.len() {
        stretches.push(&points[from..]);
    }
    stretches
}

/// What `points` say of a week: the quota dollars `spent_between` the first
/// and the last reading of each stretch that rose, and how far, in tenths of
/// a point, they rose together.
pub(super) fn rose(points: &[(Timestamp, u32)], spent_between: impl Fn(Timestamp, Timestamp) -> Usd) -> (Usd, u32) {
    let (mut spent, mut rise) = (Usd::ZERO, 0u32);
    for one in stretches(points) {
        let (first, low) = one[0];
        let (last, _) = one[one.len() - 1];
        let high = one.iter().map(|(_, tenths)| *tenths).max().unwrap_or(low);
        // A stretch its readings never rose in says nothing of the week.
        if high <= low {
            continue;
        }
        spent = spent.saturating_add(spent_between(first, last));
        rise = rise.saturating_add(high - low);
    }
    (spent, rise)
}

/// What this week's own readings say of it for `account`: the quota dollars
/// used while they rose, of that account, and how far they rose; none before
/// they rose at all.
pub(super) fn this_week(book: &Book, account: &Option<String>, week: Week) -> Option<(Usd, u32)> {
    let read = book.week_read.get(account).filter(|read| same(read.end, week.end))?;
    let used: Vec<(Timestamp, Usd)> = super::reckoning::quota_used(book)
        .filter(|(at, _)| super::ledger::account_at(&book.accounts, *at) == account.as_deref())
        .collect();
    let spent_between = |first: Timestamp, last: Timestamp| {
        used.iter()
            .filter(|(at, _)| *at >= first && *at < last)
            .fold(Usd::ZERO, |sum, (_, usd)| sum.saturating_add(*usd))
    };
    let (spent, rise) = rose(&read.points, spent_between);
    (rise > 0).then_some((spent, rise))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::SignedDuration;

    fn at(minute: i64) -> Timestamp {
        Timestamp::from_second(1_800_000_000 + minute * 60).unwrap()
    }

    fn weekly(end: Timestamp, tenths: u32) -> Vec<LimitWindow> {
        vec![LimitWindow {
            name: "seven_day".into(),
            minutes: Some(WEEK_MINUTES),
            used_tenths: tenths,
            resets_at: Some(end),
            since: None,
        }]
    }

    fn tenths(points: &[(Timestamp, u32)]) -> Vec<u32> {
        points.iter().map(|(_, tenths)| *tenths).collect()
    }

    #[test]
    fn a_weeks_readings_are_kept_in_the_order_read_and_a_later_week_starts_anew() {
        let week = SignedDuration::from_hours(24 * 7);
        let end = at(10_000);
        let mut read = HashMap::new();
        take(&mut read, None, at(2), &weekly(end, 30));
        // Said late: it stands where it was read.
        take(&mut read, None, at(1), &weekly(end, 20));
        // The same week, its reset said a minute apart.
        take(&mut read, None, at(3), &weekly(end - SignedDuration::from_secs(60), 40));
        // A week before it, read late: no part of this one.
        take(&mut read, None, at(0), &weekly(end - week, 900));
        // Another account's: kept apart.
        take(&mut read, Some("other".into()), at(4), &weekly(end, 700));
        assert_eq!(tenths(&read[&None].points), vec![20, 30, 40]);
        assert_eq!(tenths(&read[&Some("other".into())].points), vec![700]);
        // A reading of the week after starts it.
        take(&mut read, None, at(5), &weekly(end + week, 5));
        assert_eq!(read[&None].points, vec![(at(5), 5)]);
        // A reading said as of its week's end or later reads no part of it.
        take(&mut read, None, end + week, &weekly(end + week, 6));
        assert_eq!(read[&None].points.len(), 1);
    }

    #[test]
    fn a_week_splits_where_its_readings_fell_and_stayed_down_and_nowhere_else() {
        let points = |tenths: &[u32]| tenths.iter().enumerate().map(|(i, t)| (at(i as i64), *t)).collect::<Vec<_>>();
        let split = |tenths: &[u32]| {
            let points = points(tenths);
            stretches(&points).iter().map(|one| self::tenths(one)).collect::<Vec<_>>()
        };
        // A stale reading a point below, the next ones on up: one stretch.
        assert_eq!(split(&[0, 100, 200, 190, 210, 300]).len(), 1);
        // Down six points, and up again before ten more readings: still one.
        assert_eq!(split(&[0, 100, 200, 140, 210, 220, 230, 240, 250, 260, 270, 280]).len(), 1);
        // Given back to nothing and rising again: two.
        assert_eq!(split(&[0, 400, 820, 0, 10, 100, 640]), vec![vec![0, 400, 820], vec![0, 10, 100, 640]]);
        // Given back in part: two, the second rising from what was left.
        assert_eq!(split(&[0, 870, 540, 560, 600, 730]), vec![vec![0, 870], vec![540, 560, 600, 730]]);
    }

    #[test]
    fn what_a_week_rose_is_what_its_stretches_rose_and_used_between_their_ends() {
        let points: Vec<(Timestamp, u32)> =
            [0, 400, 820, 0, 10, 100, 640, 640].iter().enumerate().map(|(i, t)| (at(i as i64 * 10), *t)).collect();
        // A dollar a minute.
        let spent =
            |first: Timestamp, last: Timestamp| Usd(last.duration_since(first).as_secs() as u64 / 60 * 1_000_000);
        // 0 to 820 over twenty minutes, 0 to 640 over forty.
        assert_eq!(rose(&points, spent), (Usd(60_000_000), 820 + 640));
        // A stretch that never rose adds nothing: not even what it used.
        let flat: Vec<(Timestamp, u32)> = [500, 500, 500].iter().enumerate().map(|(i, t)| (at(i as i64), *t)).collect();
        assert_eq!(rose(&flat, spent), (Usd::ZERO, 0));
    }
}
