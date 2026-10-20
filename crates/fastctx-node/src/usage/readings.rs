//! Account windows merge by reset period, so delayed sessions cannot move them backwards.
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

pub(super) fn merge(known: &mut Vec<LimitWindow>, windows: Vec<LimitWindow>) {
    for incoming in windows {
        let Some(old) = known.iter_mut().find(|old| old.name == incoming.name && old.minutes == incoming.minutes)
        else {
            known.push(incoming);
            continue;
        };
        match (old.resets_at, incoming.resets_at) {
            (Some(a), Some(b)) if same(a, b) => {
                old.used_tenths = old.used_tenths.max(incoming.used_tenths);
            }
            (Some(a), Some(b)) if b < a => {}
            (_, Some(_)) => *old = incoming,
            _ => old.used_tenths = old.used_tenths.max(incoming.used_tenths),
        }
    }
    known.sort_by(|a, b| a.name.cmp(&b.name).then(a.minutes.cmp(&b.minutes)));
}

pub(super) fn record(book: &mut Book, path: &Path, incoming: Vec<LimitWindow>, at: Timestamp) -> io::Result<bool> {
    if incoming.is_empty() {
        return Ok(false);
    }
    let mut windows = book.reading.as_ref().map_or_else(Vec::new, |(_, windows)| windows.clone());
    merge(&mut windows, incoming);
    if book.reading.as_ref().is_some_and(|(_, old)| *old == windows) {
        return Ok(false);
    }
    book.commit(path, Entry::Reading { at, windows })?;
    Ok(true)
}

pub(super) fn last_reading(node: &Node, path: &Path) -> Option<Reading> {
    super::with_book(node, path, |book| Ok(book.reading.clone())).unwrap_or_else(|error| {
        node.log.line(format_args!("reading usage windows: {error}"));
        None
    })
}

pub(super) fn week(reading: Option<&Reading>, now: Timestamp) -> Week {
    let reset = reading
        .and_then(|(_, windows)| windows.iter().find(|window| window.minutes == Some(10_080)))
        .and_then(|w| w.resets_at);
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

pub(crate) fn codex_reading(node: &Node, dir: &Path, windows: Vec<LimitWindow>, at: Timestamp) -> io::Result<bool> {
    let path = super::ledger(node, "codex", dir);
    super::with_book(node, &path, |book| record(book, &path, windows, at))
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
