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

/// How long changed readings below the kept figure must persist before
/// the week counts as cleared; unchanged and first-source readings still
/// cannot confirm a reset.
const LATE: SignedDuration = SignedDuration::from_secs(5 * 60);

/// The most sources a weekly window remembers the last figure of; past it
/// they are forgotten, and each one's next figure is news again.
const SOURCES: usize = 65_536;

/// Of each weekly window of one account, by name, what tells a figure that
/// came late from the week cleared in place (`merge`).
pub(super) type Lowered = HashMap<String, Heard>;

/// Of one weekly window in its period: the figure each source last gave,
/// since when news below the kept figure has come with none back up to it,
/// and the period's clearings, the latest last.
#[derive(Default)]
pub(super) struct Heard {
    last: HashMap<String, u32>,
    lower: Option<Timestamp>,
    cleared: Vec<Clearing>,
}

struct Clearing {
    since: Timestamp,
    /// The figure the week fell from: coming back up to it, it was none.
    from: u32,
    standing: bool,
}

/// Where readings come from.
#[derive(Clone, Copy)]
pub(super) enum Source<'a> {
    /// A session, or a Codex login's app-server, telling them now.
    Live(&'a str),
    /// The ledger read back, its rows carrying the clearings seen live.
    Ledger,
}

pub(super) fn same(a: Timestamp, b: Timestamp) -> bool {
    a.duration_since(b).abs() <= SignedDuration::from_secs(120)
}

/// Merges `windows`, observed at `at`, no earlier than `known`, into it.
/// Within one period usage only rises, so in a week a figure lower than
/// the one kept is the week cleared in place, its reset kept, as a vendor
/// does when the plan changes; from then its usage counts anew (`since`).
/// But each session gives what the last answer to it carried: while others
/// have seen the week rise, one whose answer took long gives the figure
/// from before, and one whose requests fail gives its old figure again.
/// So only a figure that changed is news, and the week counts as cleared
/// when news below the kept figure has held for `LATE`, none rising back
/// to it; a week that comes back up to the figure it fell from was not
/// cleared. A shorter window resets soon enough by itself, and keeps the
/// higher. Only where the login's owner chose to `follow` such clearings:
/// otherwise within one period the week only rises, and runs from its
/// reset alone.
pub(super) fn merge(
    known: &mut Vec<LimitWindow>,
    windows: Vec<LimitWindow>,
    at: Timestamp,
    lowered: &mut Lowered,
    source: Source<'_>,
    follow: bool,
) {
    for incoming in windows {
        let incoming = if follow { incoming } else { LimitWindow { since: None, ..incoming } };
        let Some(old) = known.iter_mut().find(|old| old.name == incoming.name && old.minutes == incoming.minutes)
        else {
            lowered.remove(&incoming.name);
            known.push(incoming);
            continue;
        };
        match (old.resets_at, incoming.resets_at) {
            (Some(a), Some(b)) if same(a, b) && follow && old.minutes == Some(WEEK_MINUTES) => {
                weekly(old, incoming, at, lowered.entry(old.name.clone()).or_default(), source);
            }
            (Some(a), Some(b)) if same(a, b) => old.used_tenths = old.used_tenths.max(incoming.used_tenths),
            (Some(a), Some(b)) if b < a => {}
            // Without a reset identifier, only a newer observation can establish recovery.
            (_, Some(_)) | (_, None) => {
                lowered.remove(&old.name);
                *old = incoming;
            }
        }
    }
    known.sort_by(|a, b| a.name.cmp(&b.name).then(a.minutes.cmp(&b.minutes)));
}

/// One weekly window's reading within the period kept (`merge`).
fn weekly(old: &mut LimitWindow, incoming: LimitWindow, at: Timestamp, heard: &mut Heard, source: Source<'_>) {
    match source {
        Source::Ledger => {
            match incoming.since.map(|since| (since, heard.cleared.iter().find(|clearing| clearing.since == since))) {
                Some((since, None)) => {
                    heard.cleared.push(Clearing { since, from: old.used_tenths, standing: true });
                    old.used_tenths = incoming.used_tenths;
                }
                // The clearing this row was written for, heard as it came: the
                // figure kept is still the height the week fell from, and the
                // row's own is the week's now, as a reload of it finds.
                Some((_, Some(clearing))) if old.used_tenths >= clearing.from => old.used_tenths = incoming.used_tenths,
                _ => old.used_tenths = old.used_tenths.max(incoming.used_tenths),
            }
        }
        Source::Live(source) => {
            if heard.last.len() >= SOURCES && !heard.last.contains_key(source) {
                heard.last.clear();
            }
            let before = heard.last.insert(source.to_string(), incoming.used_tenths);
            if before == Some(incoming.used_tenths) {
                return;
            }
            // A source's first figure is as old as its last answer that got
            // through; one that changed is news an answer just carried.
            let news = before.is_some();
            if incoming.used_tenths >= old.used_tenths {
                if !news
                    && heard.cleared.iter().any(|clearing| clearing.standing && incoming.used_tenths >= clearing.from)
                {
                    return;
                }
                old.used_tenths = incoming.used_tenths;
                heard.lower = None;
            } else if news {
                match heard.lower {
                    None => heard.lower = Some(at),
                    Some(first) if at.duration_since(first) >= LATE => {
                        heard.cleared.push(Clearing { since: first, from: old.used_tenths, standing: true });
                        old.used_tenths = incoming.used_tenths;
                        heard.lower = None;
                    }
                    Some(_) => {}
                }
            }
        }
    }
    for clearing in &mut heard.cleared {
        clearing.standing &= old.used_tenths < clearing.from;
    }
    old.since = heard.cleared.iter().rev().find(|clearing| clearing.standing).map(|clearing| clearing.since);
}

/// Records `incoming`, as `source` gave it at `at`, for `account`.
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
    let lowered = book.lowered.entry(account.clone()).or_default();
    merge(&mut windows, incoming, at, lowered, Source::Live(source), book.follow);
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
