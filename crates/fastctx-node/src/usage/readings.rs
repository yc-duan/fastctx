//! Quota snapshots advance within a provider period; confirmed resets are durable.
use super::Week;
use super::ledger::{Book, Entry, Reading};
use super::resets::{Detector, WEEK_MINUTES};
use crate::Node;
use fastctx_types::agents::LimitWindow;
use fastctx_types::usage::{UsageReset, UsageResetSource};
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::Path;

pub(super) type Lowered = HashMap<String, Detector>;

pub(super) fn same(a: Timestamp, b: Timestamp) -> bool {
    a.duration_since(b).abs() <= SignedDuration::from_secs(120)
}

fn merge_live(
    known: &mut Vec<LimitWindow>,
    windows: Vec<LimitWindow>,
    at: Timestamp,
    detectors: &mut Lowered,
    source: &str,
    enabled: bool,
) -> Vec<UsageReset> {
    let mut resets = Vec::new();
    for mut incoming in windows {
        // The host reports usage, never an accounting boundary.
        incoming.since = None;
        let old = known.iter().position(|old| old.name == incoming.name && old.minutes == incoming.minutes);
        if let Some(index) = old {
            let old = &mut known[index];
            match (old.resets_at, incoming.resets_at) {
                (Some(a), Some(b)) if same(a, b) => {
                    if old.minutes == Some(WEEK_MINUTES) && at < a {
                        if let Some(reset) =
                            detectors.entry(old.name.clone()).or_default().observe(old, &incoming, at, source, enabled)
                        {
                            resets.push(reset);
                        }
                    } else {
                        old.used_tenths = old.used_tenths.max(incoming.used_tenths);
                    }
                    continue;
                }
                (Some(a), Some(b)) if b < a => continue,
                (Some(_), Some(end)) if incoming.minutes == Some(WEEK_MINUTES) => {
                    let since = end - SignedDuration::from_secs(super::WEEK_SECONDS);
                    if since <= at && at < end {
                        resets.push(UsageReset {
                            window: incoming.name.clone(),
                            since,
                            confirmed_at: at,
                            resets_at: end,
                            before_used_tenths: Some(old.used_tenths),
                            used_tenths: incoming.used_tenths,
                            source: UsageResetSource::Scheduled,
                        });
                    }
                }
                _ => {}
            }
        }
        detectors.remove(&incoming.name);
        if incoming.minutes == Some(WEEK_MINUTES) {
            let mut baseline = incoming.clone();
            detectors.entry(incoming.name.clone()).or_default().observe(&mut baseline, &incoming, at, source, false);
        }
        if let Some(index) = old {
            known[index] = incoming;
        } else {
            known.push(incoming);
        }
    }
    known.sort_by(|a, b| a.name.cmp(&b.name).then(a.minutes.cmp(&b.minutes)));
    resets
}

pub(super) fn record(
    book: &mut Book,
    path: &Path,
    incoming: Vec<LimitWindow>,
    at: Timestamp,
    account: Option<String>,
    source: &str,
) -> io::Result<bool> {
    if incoming.is_empty() {
        return Ok(false);
    }
    let old = book.readings.get(&account);
    if old.is_some_and(|(seen, _)| *seen > at) {
        return Ok(false);
    }
    let mut windows = old.map_or_else(Vec::new, |(_, windows)| windows.clone());
    let detectors = book.lowered.entry(account.clone()).or_default();
    let resets = merge_live(&mut windows, incoming, at, detectors, source, book.follow);
    if resets.is_empty()
        && windows.iter().all(|window| window.resets_at.is_some())
        && old.is_some_and(|(_, old)| *old == windows)
    {
        return Ok(false);
    }
    book.commit(path, Entry::Reading { at, windows, account, resets })?;
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
    // One app-server per login gives them, each as it hears it.
    super::with_book(node, &path, |book| record(book, &path, windows, at, account, ""))
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
