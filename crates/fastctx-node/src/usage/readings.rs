//! An account's windows as they were last read: the newest reading stands,
//! higher or lower than the one before it (用户 2026-11-19: 官方读数有什么填
//! 什么；不设重置检测). The week's turning at its own reset is recorded; nothing
//! else is looked for in the readings.
use super::Week;
use super::ledger::{Book, Entry, Reading};
use super::resets::WEEK_MINUTES;
use crate::Node;
use fastctx_types::agents::LimitWindow;
use fastctx_types::usage::{UsageReset, UsageResetSource};
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::Path;

/// What each session last said of an account's windows.
pub(super) type Said = HashMap<String, Vec<LimitWindow>>;
/// The sessions remembered so; past them all are forgotten.
const SOURCES: usize = 4096;

pub(super) fn same(a: Timestamp, b: Timestamp) -> bool {
    a.duration_since(b).abs() <= SignedDuration::from_secs(120)
}

/// Takes `windows` as read at `at` into what is `known`: each as it now
/// stands, whether it rose or fell, but for one of a period before the one
/// known, which was said late. A week's window that moved on to its next
/// period is told as that week's own reset.
fn merge_live(known: &mut Vec<LimitWindow>, windows: Vec<LimitWindow>, at: Timestamp) -> Vec<UsageReset> {
    let mut resets = Vec::new();
    for mut incoming in windows {
        // The host reports usage, never an accounting boundary.
        incoming.since = None;
        let old = known.iter().position(|old| old.name == incoming.name && old.minutes == incoming.minutes);
        if let Some(index) = old {
            let old = &known[index];
            match (old.resets_at, incoming.resets_at) {
                (Some(a), Some(b)) if same(a, b) => {
                    // A boundary an earlier release recorded within this
                    // period stands until the period ends.
                    incoming.since = old.since;
                    incoming.resets_at = old.resets_at;
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
            known[index] = incoming;
        } else {
            known.push(incoming);
        }
    }
    known.sort_by(|a, b| a.name.cmp(&b.name).then(a.minutes.cmp(&b.minutes)));
    resets
}

/// Records what `source`, a session, read of `account`'s windows at `at`;
/// whether anything was written. A session says what its last answer
/// carried each time it is asked, so what it said last and says again is no
/// reading of now: another session may have read the account since.
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
    let said = book.said.entry(account.clone()).or_default();
    if said.get(source).is_some_and(|last| *last == incoming) {
        return Ok(false);
    }
    if said.len() >= SOURCES {
        said.clear();
    }
    said.insert(source.to_string(), incoming.clone());
    let mut windows = old.map_or_else(Vec::new, |(_, windows)| windows.clone());
    let resets = merge_live(&mut windows, incoming, at);
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

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: SignedDuration = SignedDuration::from_hours(1);

    fn week(end: Timestamp, tenths: u32) -> Vec<LimitWindow> {
        vec![LimitWindow {
            name: "seven_day".into(),
            minutes: Some(WEEK_MINUTES),
            used_tenths: tenths,
            resets_at: Some(end),
            since: None,
        }]
    }

    fn stands(book: &Book) -> (u32, Timestamp) {
        let (_, windows) = book.reading.as_ref().expect("a reading");
        let week = window(windows, WEEK_MINUTES).expect("the week's window");
        (week.used_tenths, week.resets_at.expect("its reset"))
    }

    #[test]
    fn the_newest_reading_stands_whether_it_rose_or_fell() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let at = Timestamp::from_second(1_800_000_000).unwrap();
        let end = at + HOUR * 72;
        let mut book = Book::default();
        let read = |book: &mut Book, hour: i32, source: &str, end: Timestamp, tenths: u32| {
            record(book, path, week(end, tenths), at + HOUR * hour, None, source).unwrap()
        };
        assert!(read(&mut book, 0, "a", end, 500));
        assert!(read(&mut book, 1, "b", end, 550));
        // It fell, as the vendor says: that is how it stands.
        assert!(read(&mut book, 2, "a", end, 300));
        assert_eq!(stands(&book), (300, end));
        // A session that says again what it said is no reading of now; nor
        // is one heard out of its turn, or one of the week before.
        assert!(!read(&mut book, 3, "b", end, 550));
        assert!(!read(&mut book, 1, "c", end, 900));
        assert!(!read(&mut book, 4, "c", end - HOUR * 168, 990));
        assert_eq!(stands(&book), (300, end));
        // Once it has something new to say, it is the newest.
        assert!(read(&mut book, 5, "b", end, 310));
        assert_eq!(stands(&book), (310, end));
        assert!(book.reset_history().is_empty());

        // The week turned at its own reset: recorded as that, and no other.
        let next = end + HOUR * 168;
        assert!(read(&mut book, 73, "a", next, 4));
        assert_eq!(stands(&book), (4, next));
        let history = book.reset_history();
        assert_eq!(history.len(), 1);
        assert_eq!((history[0].1.source, history[0].1.since), (UsageResetSource::Scheduled, end));

        // Read again from its files, the ledger stands as it was written.
        let mut again = Book::default();
        again.load_at(path, at + HOUR * 80).unwrap();
        assert_eq!(stands(&again), (4, next));
        let mut before = Book::default();
        for row in super::super::ledger::days(path, Timestamp::MIN)
            .unwrap()
            .iter()
            .flat_map(|file| super::super::ledger::read(file).unwrap())
        {
            if row.at() <= at + HOUR * 5 {
                before.commit(root.path().join("copy").as_path(), row).unwrap();
            }
        }
        assert_eq!(stands(&before), (310, end));
    }
}
