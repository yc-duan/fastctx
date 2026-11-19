//! An account's windows as they were last read: the newest reading of each
//! window stands, higher or lower than the one before it (用户 2026-11-19:
//! 官方读数有什么填什么；不设重置检测), the newest by when the answer that
//! carried it came, not by when it reached the node. The week's turning at its
//! own reset is recorded; nothing else is looked for in the readings.
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

/// When each of an account's windows was last read, by its name and length:
/// a reading of one that is older is said late. Not kept from one run of
/// the node to the next; a window not heard of since is as old as the last
/// reading the ledger holds.
pub(super) type Heard = HashMap<(String, Option<u32>), Timestamp>;

pub(super) fn same(a: Timestamp, b: Timestamp) -> bool {
    a.duration_since(b).abs() <= SignedDuration::from_secs(120)
}

/// Takes `windows` as read at `at` into what is `known`: each as it now
/// stands, whether it rose or fell, but for one of a period before the one
/// known, which was said late. A reading no answer is known to have carried
/// (`answered`), as an earlier release of the mod sent with any sample of a
/// session's figures, may be what an earlier answer carried: it raises a
/// window and never lowers it. A week's window that moved on to its next
/// period is told as that week's own reset. Which windows were taken.
fn merge_live(
    known: &mut Vec<LimitWindow>,
    windows: Vec<LimitWindow>,
    at: Timestamp,
    answered: bool,
) -> (Vec<UsageReset>, Vec<(String, Option<u32>)>) {
    let mut resets = Vec::new();
    let mut taken = Vec::new();
    for mut incoming in windows {
        // The host reports usage, never an accounting boundary.
        incoming.since = None;
        let old = known.iter().position(|old| old.name == incoming.name && old.minutes == incoming.minutes);
        if let Some(index) = old {
            let old = &known[index];
            match (old.resets_at, incoming.resets_at) {
                (Some(a), Some(b)) if same(a, b) => {
                    // Said again as it was, it is no reading of now either.
                    if !answered && incoming.used_tenths <= old.used_tenths {
                        continue;
                    }
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
                (None, None) if !answered && incoming.used_tenths <= old.used_tenths => continue,
                _ => {}
            }
            taken.push((incoming.name.clone(), incoming.minutes));
            known[index] = incoming;
        } else {
            taken.push((incoming.name.clone(), incoming.minutes));
            known.push(incoming);
        }
    }
    known.sort_by(|a, b| a.name.cmp(&b.name).then(a.minutes.cmp(&b.minutes)));
    (resets, taken)
}

/// How long a reading the same as the one kept is left unwritten: the
/// ledger's last row is as old at most as this, which is what a node
/// started again takes for when each window was last read.
const UNCHANGED_KEPT: SignedDuration = SignedDuration::from_secs(60);

/// Records what an answer at `at` carried of `account`'s windows; whether
/// anything was written. Each window is taken where this is newer than what
/// was last read of it (`Heard`); `answered` as `merge_live` says.
pub(super) fn record(
    book: &mut Book,
    path: &Path,
    incoming: Vec<LimitWindow>,
    at: Timestamp,
    account: Option<String>,
    answered: bool,
) -> io::Result<bool> {
    let old = book.readings.get(&account);
    let ledgered = old.map(|(seen, _)| *seen);
    let heard = book.heard.entry(account.clone()).or_default();
    let incoming: Vec<_> = incoming
        .into_iter()
        .filter(|window| {
            let last = heard.get(&(window.name.clone(), window.minutes)).copied().or(ledgered);
            last.is_none_or(|last| last <= at)
        })
        .collect();
    if incoming.is_empty() {
        return Ok(false);
    }
    let mut windows = old.map_or_else(Vec::new, |(_, windows)| windows.clone());
    let (resets, taken) = merge_live(&mut windows, incoming, at, answered);
    // Only a window taken is read as of then: one an earlier mod said lower,
    // left as it was, makes no later answer late.
    let confirmed = !taken.is_empty();
    // An earlier mod's reading that took no window says nothing of now, and
    // moves no row's time on.
    if !confirmed && resets.is_empty() {
        return Ok(false);
    }
    for key in taken {
        heard.insert(key, at);
    }
    if resets.is_empty()
        && windows.iter().all(|window| window.resets_at.is_some())
        && old.is_some_and(|(_, old)| *old == windows)
        && (!confirmed || ledgered.is_some_and(|seen| at < seen + UNCHANGED_KEPT))
    {
        return Ok(false);
    }
    // The row is no older than the one before it: a reading of one window
    // said late, after another's newer one, is the account's as of then.
    let row = ledgered.map_or(at, |seen| seen.max(at));
    book.commit(path, Entry::Reading { at: row, windows, account, resets })?;
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
    super::with_book(node, &path, |book| record(book, &path, windows, at, account, true))
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

    fn window(name: &str, minutes: u32, end: Timestamp, tenths: u32) -> LimitWindow {
        LimitWindow {
            name: name.into(),
            minutes: Some(minutes),
            used_tenths: tenths,
            resets_at: Some(end),
            since: None,
        }
    }

    fn week(end: Timestamp, tenths: u32) -> Vec<LimitWindow> {
        vec![window("seven_day", WEEK_MINUTES, end, tenths)]
    }

    fn stands(book: &Book, minutes: u32) -> (u32, Timestamp) {
        let (_, windows) = book.reading.as_ref().expect("a reading");
        let found = super::window(windows, minutes).expect("the window");
        (found.used_tenths, found.resets_at.expect("its reset"))
    }

    #[test]
    fn the_newest_reading_stands_whether_it_rose_or_fell() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let at = Timestamp::from_second(1_800_000_000).unwrap();
        let end = at + HOUR * 72;
        let mut book = Book::default();
        let read = |book: &mut Book, hour: i32, windows: Vec<LimitWindow>, answered: bool| {
            record(book, path, windows, at + HOUR * hour, None, answered).unwrap()
        };
        assert!(read(&mut book, 0, week(end, 500), true));
        assert!(read(&mut book, 1, week(end, 550), true));
        // It fell, as the vendor says: that is how it stands.
        assert!(read(&mut book, 2, week(end, 300), true));
        assert_eq!(stands(&book, WEEK_MINUTES), (300, end));
        // The same said again by another answer is a reading all the same,
        // written where the last row is a minute old or more.
        assert!(read(&mut book, 3, week(end, 300), true));
        assert!(
            !record(&mut book, path, week(end, 300), at + HOUR * 3 + SignedDuration::from_secs(30), None, true)
                .unwrap()
        );
        assert!(read(&mut book, 4, week(end, 550), true));
        assert!(read(&mut book, 5, week(end, 300), true));
        // An answer that came before the one known is said late; so is one
        // of the week before.
        assert!(!read(&mut book, 1, week(end, 900), true));
        assert!(!read(&mut book, 6, week(end - HOUR * 168, 990), true));
        assert_eq!(stands(&book, WEEK_MINUTES), (300, end));
        // What no answer is known to have carried raises a window, never
        // lowers it, and makes no answer after it late.
        assert!(!read(&mut book, 7, week(end, 120), false));
        assert_eq!(stands(&book, WEEK_MINUTES), (300, end));
        assert!(read(&mut book, 8, week(end, 320), false));
        assert_eq!(stands(&book, WEEK_MINUTES), (320, end));
        // Each window by when it was read: a newer five hours does not make
        // the week's reading of a little before it late.
        let five = at + HOUR * 12;
        assert!(read(&mut book, 10, vec![window("five_hour", 300, five, 40)], true));
        assert!(read(&mut book, 9, week(end, 330), true));
        assert_eq!((stands(&book, 300).0, stands(&book, WEEK_MINUTES).0), (40, 330));
        // An earlier mod saying again what stands, of a window with no reset
        // time, writes nothing and moves no row's time on.
        let open = |tenths| vec![LimitWindow { resets_at: None, ..window("open", 60, five, tenths) }];
        assert!(read(&mut book, 11, open(10), true));
        assert!(!read(&mut book, 12, open(10), false));
        assert!(read(&mut book, 12, open(11), false));
        assert!(book.reset_history().is_empty());

        // The week turned at its own reset: recorded as that, and no other.
        let next = end + HOUR * 168;
        assert!(read(&mut book, 73, week(next, 4), true));
        assert_eq!(stands(&book, WEEK_MINUTES), (4, next));
        let history = book.reset_history();
        assert_eq!(history.len(), 1);
        assert_eq!((history[0].1.source, history[0].1.since), (UsageResetSource::Scheduled, end));

        // Read again from its files, the ledger stands as it was written.
        let mut again = Book::default();
        again.load_at(path, at + HOUR * 80).unwrap();
        assert_eq!(stands(&again, WEEK_MINUTES), (4, next));
        // A node started again takes no reading older than the last kept.
        assert!(!record(&mut again, path, week(next, 90), at + HOUR * 72, None, true).unwrap());
    }
}
