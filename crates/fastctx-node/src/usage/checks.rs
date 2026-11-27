//! How far what a Claude Code login's ledger reckons of its week stood from
//! each reading its vendor gave of that week, counted beside the ledger: the
//! vendor's readings are what the reckoning answers to (用户 2026-11-27:
//! 最准确的一个参照就是你从官方接口读出来的那个百分比). Each reading is
//! compared with the share reckoned just before it was taken in. Nothing of
//! it leaves this machine.

use super::Week;
use super::ledger::Book;
use super::readings::{same, window};
use super::resets::WEEK_MINUTES;
use fastctx_platform::Permissions;
use fastctx_types::agents::LimitWindow;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

const FILE: &str = "checks.json";
/// The latest readings kept that stood two points or more from what was
/// reckoned.
const APART_KEPT: usize = 20;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Counted {
    /// When counting began.
    since: Option<Timestamp>,
    /// Readings compared, and of them how many stood within one point of
    /// what was reckoned and within two.
    compared: u64,
    within_one: u64,
    within_two: u64,
    /// The latest of those further apart: when, what was reckoned and what
    /// was read, in tenths of a point; the oldest first.
    apart: Vec<(Timestamp, u32, u32)>,
}

/// What a login's ledger reckoned of its week at a moment: which week, of
/// which of the vendor's accounts, and how much of it was used, in tenths of
/// a point.
pub(super) struct Reckoned {
    pub week: Week,
    pub account: Option<String>,
    pub tenths: u32,
}

/// The 7-day window of a reading as it came: when its week ends and how much
/// of it was used, in tenths of a point.
pub(super) fn weekly(windows: &[LimitWindow]) -> Option<(Timestamp, u32)> {
    let weekly = window(windows, WEEK_MINUTES)?;
    Some((weekly.resets_at?, weekly.used_tenths))
}

/// What a reading whose 7-day window was `weekly` read of `reckoned`'s week,
/// once taken into `book`: none where it read another week, or its window
/// was not the one kept (one said late, of a week before, is not).
pub(super) fn read(book: &Book, reckoned: &Reckoned, weekly: Option<(Timestamp, u32)>) -> Option<u32> {
    let (end, read) = weekly.filter(|(end, _)| same(*end, reckoned.week.end))?;
    let (_, kept) = book.readings.get(&reckoned.account)?;
    let kept = window(kept, WEEK_MINUTES)?;
    (kept.used_tenths == read && kept.resets_at.is_some_and(|kept| same(kept, end))).then_some(read)
}

/// Counts the reading of `read` tenths of `reckoned`'s week, taken at `at`.
pub(super) fn note(path: &Path, at: Timestamp, reckoned: &Reckoned, read: u32) -> io::Result<()> {
    let file = path.join(FILE);
    let mut counted: Counted = match fastctx_platform::disk::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Counted::default(),
        Err(error) => return Err(error),
    };
    counted.since.get_or_insert(at);
    counted.compared += 1;
    let apart = reckoned.tenths.abs_diff(read);
    if apart <= 10 {
        counted.within_one += 1;
    }
    if apart <= 20 {
        counted.within_two += 1;
    } else {
        counted.apart.push((at, reckoned.tenths, read));
        if counted.apart.len() > APART_KEPT {
            counted.apart.remove(0);
        }
    }
    let bytes = serde_json::to_vec(&counted).map_err(io::Error::other)?;
    fastctx_platform::publish(&file, &bytes, Permissions::Private)
}
