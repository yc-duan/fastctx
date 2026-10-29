//! Account windows merge by reset period, so delayed sessions cannot move
//! them backwards, while a week whose usage was reset in place is seen.
use super::Week;
use super::ledger::{Book, Entry, Reading};
use crate::Node;
use fastctx_types::agents::LimitWindow;
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::Path;

/// The minutes of a week, the window whose usage only rises until it resets.
const WEEK_MINUTES: u32 = 10_080;

/// Of each weekly window of one account, by name, a reading lower than the
/// one kept, and when it was seen, waiting for the next reading to say
/// whether the week was reset in place (`merge`).
pub(super) type Lowered = HashMap<String, (Timestamp, u32)>;

pub(super) fn same(a: Timestamp, b: Timestamp) -> bool {
    a.duration_since(b).abs() <= SignedDuration::from_secs(120)
}

/// Merges `windows`, observed at `at`, no earlier than `known`, into it.
/// Within one period usage only rises, so in a week a reading lower than
/// the one kept is the week cleared in place, its reset kept, as a vendor
/// does when the plan changes; from then its usage counts anew (`since`).
/// Each session reports what the last answer to it carried, and one whose
/// request failed may report an older figure: so a lower reading is held
/// in `lowered` until the next, and taken when that one is lower too. A
/// shorter window resets soon enough by itself, and keeps the higher. Rows
/// read back from the ledger, `lowered` none, are as merged when written.
pub(super) fn merge(
    known: &mut Vec<LimitWindow>,
    windows: Vec<LimitWindow>,
    at: Timestamp,
    mut lowered: Option<&mut Lowered>,
) {
    for incoming in windows {
        let Some(old) = known.iter_mut().find(|old| old.name == incoming.name && old.minutes == incoming.minutes)
        else {
            known.push(incoming);
            continue;
        };
        // A lower reading held from another period is no second word on
        // this one.
        let mut forget = |name: &str| {
            if let Some(lowered) = lowered.as_deref_mut() {
                lowered.remove(name);
            }
        };
        match (old.resets_at, incoming.resets_at) {
            (Some(a), Some(b)) if same(a, b) && incoming.since.is_some() && incoming.since != old.since => {
                forget(&old.name);
                *old = incoming;
            }
            (Some(a), Some(b))
                if same(a, b) && old.minutes == Some(WEEK_MINUTES) && incoming.used_tenths < old.used_tenths =>
            {
                let Some(lowered) = lowered.as_deref_mut() else { continue };
                match lowered.remove(&old.name) {
                    Some((first, _)) => *old = LimitWindow { since: Some(first), ..incoming },
                    None => {
                        lowered.insert(old.name.clone(), (at, incoming.used_tenths));
                    }
                }
            }
            (Some(a), Some(b)) if same(a, b) => {
                old.used_tenths = old.used_tenths.max(incoming.used_tenths);
                if let Some(lowered) = lowered.as_deref_mut() {
                    lowered.remove(&old.name);
                }
            }
            (Some(a), Some(b)) if b < a => {}
            // Without a reset identifier, only a newer observation can establish recovery.
            (_, Some(_)) | (_, None) => {
                forget(&old.name);
                *old = incoming;
            }
        }
    }
    known.sort_by(|a, b| a.name.cmp(&b.name).then(a.minutes.cmp(&b.minutes)));
}

pub(super) fn record(
    book: &mut Book,
    path: &Path,
    incoming: Vec<LimitWindow>,
    at: Timestamp,
    account: Option<String>,
) -> io::Result<bool> {
    if incoming.is_empty() {
        return Ok(false);
    }
    let old = book.readings.get(&account);
    if old.is_some_and(|(seen, _)| *seen > at) {
        return Ok(false);
    }
    let mut windows = old.map_or_else(Vec::new, |(_, windows)| windows.clone());
    merge(&mut windows, incoming, at, Some(book.lowered.entry(account.clone()).or_default()));
    if windows.iter().all(|window| window.resets_at.is_some()) && old.is_some_and(|(_, old)| *old == windows) {
        return Ok(false);
    }
    book.commit(path, Entry::Reading { at, windows, account })?;
    Ok(true)
}

pub(super) fn last_reading(node: &Node, path: &Path) -> Option<Reading> {
    super::with_book(node, path, |book| Ok(book.reading.clone())).unwrap_or_else(|error| {
        node.log.line(format_args!("reading usage windows: {error}"));
        None
    })
}

/// The window of `minutes` that is most used: an account with several
/// allowances of one length, as Codex's buckets, is held by the tightest.
pub(super) fn window(windows: &[LimitWindow], minutes: u32) -> Option<&LimitWindow> {
    windows.iter().filter(|window| window.minutes == Some(minutes)).max_by_key(|window| window.used_tenths)
}

pub(super) fn week(reading: Option<&Reading>, now: Timestamp) -> Week {
    let weekly = reading.and_then(|(_, windows)| window(windows, WEEK_MINUTES));
    let reset = weekly.and_then(|w| w.resets_at);
    let week = SignedDuration::from_secs(super::WEEK_SECONDS);
    match reset {
        Some(mut end) => {
            if now >= end {
                end += SignedDuration::from_secs(
                    super::WEEK_SECONDS * (now.duration_since(end).as_secs() / super::WEEK_SECONDS + 1),
                );
            }
            // A week cleared in place counts from then, for those it is
            // shared with as for the account (`world-usage.md`).
            let start =
                weekly.and_then(|w| w.since).filter(|since| *since > end - week && *since < end).unwrap_or(end - week);
            Week { start, end, window: true }
        }
        None => Week { start: now - week, end: now, window: false },
    }
}

pub(crate) fn codex_reading(
    node: &Node,
    dir: &Path,
    windows: Vec<LimitWindow>,
    at: Timestamp,
    account: Option<String>,
) -> io::Result<bool> {
    let path = super::ledger(node, "codex", dir);
    super::with_book(node, &path, |book| record(book, &path, windows, at, account))
}

pub(super) fn claude_windows(limits: &[Value]) -> Vec<LimitWindow> {
    limits
        .iter()
        .filter_map(|limit| {
            let name = limit.get("kind")?.as_str()?.to_string();
            let percent = limit.get("percentUsed")?.as_f64().filter(|p| p.is_finite() && *p >= 0.0)?;
            Some(LimitWindow {
                minutes: match name.as_str() {
                    "five_hour" => Some(300),
                    "seven_day" => Some(10_080),
                    _ => None,
                },
                name,
                used_tenths: (percent * 10.0).round() as u32,
                resets_at: limit.get("resetsAt").and_then(Value::as_str).and_then(|at| at.parse().ok()),
                since: None,
            })
        })
        .collect()
}
