//! Account windows merge by reset period, so delayed sessions cannot move
//! them backwards, while a period whose usage was reset in place is seen.
use super::Week;
use super::ledger::{Book, Entry, Reading};
use crate::Node;
use fastctx_types::agents::LimitWindow;
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;
use std::io;
use std::path::Path;

pub(super) fn same(a: Timestamp, b: Timestamp) -> bool {
    a.duration_since(b).abs() <= SignedDuration::from_secs(120)
}

/// Within one period usage only rises: sessions report what they last saw,
/// in no fixed order, so a reading a little lower is a late one. Falling to
/// half or less, and by two points or more, is the period's usage reset with
/// its end kept, as a vendor does when the plan changes.
fn reset_in_place(known: u32, incoming: u32) -> bool {
    incoming * 2 <= known && known - incoming >= 20
}

/// Merges `windows`, observed no earlier than `known`, into it.
pub(super) fn merge(known: &mut Vec<LimitWindow>, windows: Vec<LimitWindow>) {
    for incoming in windows {
        let Some(old) = known.iter_mut().find(|old| old.name == incoming.name && old.minutes == incoming.minutes)
        else {
            known.push(incoming);
            continue;
        };
        match (old.resets_at, incoming.resets_at) {
            (Some(a), Some(b)) if same(a, b) && reset_in_place(old.used_tenths, incoming.used_tenths) => {
                *old = incoming;
            }
            (Some(a), Some(b)) if same(a, b) => {
                old.used_tenths = old.used_tenths.max(incoming.used_tenths);
            }
            (Some(a), Some(b)) if b < a => {}
            (_, Some(_)) => *old = incoming,
            // Without a reset identifier, only a newer observation can establish recovery.
            (_, None) => *old = incoming,
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
    merge(&mut windows, incoming);
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
    let reset = reading.and_then(|(_, windows)| window(windows, 10_080)).and_then(|w| w.resets_at);
    let week = SignedDuration::from_secs(super::WEEK_SECONDS);
    match reset {
        Some(mut end) => {
            if now >= end {
                end += SignedDuration::from_secs(
                    super::WEEK_SECONDS * (now.duration_since(end).as_secs() / super::WEEK_SECONDS + 1),
                );
            }
            Week { start: end - week, end, window: true }
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
            })
        })
        .collect()
}
