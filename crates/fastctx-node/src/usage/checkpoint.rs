//! What a usage ledger holds through the last day that no longer changes,
//! kept beside its day files so that loading the ledger reads the days after
//! that one and not every day since the ledger began: a login in steady use
//! writes about a megabyte a day, which nothing deletes.
//!
//! It is derived. The day files are the ledger; a checkpoint that does not
//! match them, a day it holds that has grown or a day it does not know, is
//! read past, and the next load makes it again. A row written for a day it
//! holds discards it (`Book::commit`).

use super::ledger::{Book, Identity, MONTH, RECENT, Reading};
use super::meter::Meter;
use super::recent::Charge;
use fastctx_types::usage::{UsageReset, Usd};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;
const NAME: &str = "book.checkpoint.json";

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    /// The day files it holds, oldest first, each with its length then.
    files: Vec<(String, u64)>,
    state: State,
}

/// What a ledger's rows leave in its `Book`, as a full read of the same
/// days leaves it. What only a session in this node's life holds, as the
/// detectors of early resets, a full read does not leave either.
#[derive(Serialize, Deserialize)]
pub(super) struct State {
    totals: Vec<(Identity, Timestamp, Usd)>,
    meters: Vec<Meter>,
    seen: Vec<(Identity, Timestamp, Timestamp)>,
    keys: Vec<(String, Option<Timestamp>)>,
    readings: Vec<(Option<String>, Reading)>,
    resets: Vec<(Option<String>, Vec<UsageReset>)>,
    /// None in a checkpoint made before it was kept: the week's readings are
    /// then those of the days after it.
    #[serde(default)]
    week_read: Vec<(Option<String>, super::week_read::WeekRead)>,
    accounts: Vec<(Timestamp, String)>,
    recent: Vec<Charge>,
}

fn file(path: &Path) -> PathBuf {
    path.join(NAME)
}

/// The first day file a checkpoint does not hold as of `now`: yesterday's,
/// which a row of a turn that ended before midnight may still reach.
pub(super) fn first_open(path: &Path, now: Timestamp) -> PathBuf {
    super::ledger::day_file(path, now - SignedDuration::from_hours(24))
}

/// The checkpoint of the ledger at `path` and the last day file it holds,
/// when it holds exactly the day files `files` has through that one, each
/// as long as it was then.
pub(super) fn restore(path: &Path, files: &[PathBuf]) -> Option<(State, PathBuf)> {
    let bytes = fastctx_platform::disk::read(file(path)).ok()?;
    let saved: Saved = serde_json::from_slice(&bytes).ok()?;
    if saved.version != VERSION {
        return None;
    }
    let held: Vec<PathBuf> = saved.files.iter().map(|(name, _)| path.join(name)).collect();
    let through = held.last()?.clone();
    let now: Vec<&PathBuf> = files.iter().filter(|file| **file <= through).collect();
    if now.len() != held.len() || now.iter().zip(&held).any(|(now, held)| *now != held) {
        return None;
    }
    for (held, (_, length)) in held.iter().zip(&saved.files) {
        if fastctx_platform::disk::metadata(held).ok()?.len() != *length {
            return None;
        }
    }
    Some((saved.state, through))
}

/// Keeps what `book` holds, having read the day files `files` and no other.
pub(super) fn save(path: &Path, book: &Book, files: &[PathBuf]) -> io::Result<()> {
    let files = files
        .iter()
        .map(|held| -> io::Result<(String, u64)> {
            let name =
                held.file_name().and_then(|name| name.to_str()).ok_or_else(|| io::Error::other("a day file name"))?;
            Ok((name.to_string(), fastctx_platform::disk::metadata(held)?.len()))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let bytes = serde_json::to_vec(&Saved { version: VERSION, files, state: state(book) }).map_err(io::Error::other)?;
    fastctx_platform::publish(&file(path), &bytes, fastctx_platform::Permissions::Private)
}

/// Forgets the checkpoint of the ledger at `path`.
pub(super) fn discard(path: &Path) -> io::Result<()> {
    match fastctx_platform::disk::remove_file(file(path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn state(book: &Book) -> State {
    State {
        totals: book.totals.iter().map(|(identity, (at, total))| (identity.clone(), *at, *total)).collect(),
        meters: book.meters.values().cloned().collect(),
        seen: book.seen.iter().map(|((identity, at), row)| (identity.clone(), *at, *row)).collect(),
        keys: book.keys.iter().map(|(key, at)| (key.clone(), *at)).collect(),
        readings: book.readings.iter().map(|(account, reading)| (account.clone(), reading.clone())).collect(),
        resets: book.resets.iter().map(|(account, resets)| (account.clone(), resets.clone())).collect(),
        week_read: book.week_read.iter().map(|(account, read)| (account.clone(), read.clone())).collect(),
        accounts: book.accounts.clone(),
        recent: book.recent.clone(),
    }
}

/// Makes `book` what reading the days `state` holds leaves at `now`: what a
/// row keeps only for a while is dropped where it is past that by now.
pub(super) fn adopt(book: &mut Book, state: State, now: Timestamp) {
    let (month, recent) = (now - MONTH, now - RECENT);
    book.totals = state
        .totals
        .into_iter()
        .filter(|(_, at, _)| *at >= month)
        .map(|(identity, at, total)| (identity, (at, total)))
        .collect();
    book.meters =
        state.meters.into_iter().map(|meter| ((meter.session.clone(), Some(meter.process.clone())), meter)).collect();
    book.seen = state
        .seen
        .into_iter()
        .filter(|(_, _, row)| *row >= month)
        .map(|(identity, at, row)| ((identity, at), row))
        .collect();
    book.keys = state.keys.into_iter().filter(|(_, at)| at.is_none_or(|at| at >= month)).collect();
    book.readings = state.readings.into_iter().collect();
    book.resets = state.resets.into_iter().collect();
    book.week_read = state.week_read.into_iter().collect();
    book.accounts = state.accounts;
    book.recent = state.recent.into_iter().filter(|charge| charge.at >= recent).collect();
    // As the rows make them: a session metered, or with a process whose
    // total is kept.
    book.modern = book
        .meters
        .keys()
        .map(|(session, _)| session.clone())
        .chain(book.totals.keys().filter(|(_, process)| process.is_some()).map(|(session, _)| session.clone()))
        .collect();
    book.reading = book.readings.get(&book.account_at(Timestamp::MAX)).cloned();
}

#[cfg(test)]
mod tests {
    use super::super::ledger::{Entry, Use, day_file};
    use super::*;
    use fastctx_types::usage::Tokens;

    const DAY: SignedDuration = SignedDuration::from_hours(24);

    fn row(path: &Path, entry: &Entry) {
        use std::io::Write as _;
        let mut file = fastctx_platform::disk::open_with(
            std::fs::OpenOptions::new().create(true).append(true),
            day_file(path, entry.usage_at()),
        )
        .unwrap();
        let mut bytes = serde_json::to_vec(entry).unwrap();
        bytes.push(b'\n');
        file.write_all(&bytes).unwrap();
    }

    /// Sixty days of rows of every kind a book keeps something of.
    fn ledger(path: &Path, now: Timestamp) {
        for day in (0..=60).rev() {
            let at = now - DAY * day + SignedDuration::from_mins(5);
            let at = at.min(now);
            let session = format!("session-{}", day % 7);
            row(
                path,
                &Entry::Spent {
                    at,
                    who: None,
                    with: Vec::new(),
                    session: session.clone(),
                    process: (day % 2 == 0).then(|| format!("process-{day}")),
                    total: Usd(1_000 * (61 - day as u64)),
                    uses: vec![Use {
                        model: "model".into(),
                        usd: Some(Usd(1_000)),
                        tokens: Tokens::default(),
                        kinds: None,
                        call: None,
                    }],
                },
            );
            row(
                path,
                &Entry::Checkpoint {
                    at,
                    session: session.clone(),
                    process: Some(format!("process-{day}")),
                    total: Usd(500),
                    reports: vec![at - DAY * 40, at - SignedDuration::from_mins(1)],
                },
            );
            row(
                path,
                &Entry::Tokens {
                    at,
                    who: None,
                    with: Vec::new(),
                    model: "model".into(),
                    tokens: Tokens::default(),
                    key: Some(if day % 3 == 0 { format!("terminal:{day}") } else { format!("key-{day}") }),
                    call: None,
                },
            );
            row(path, &Entry::Receipts { at, keys: vec![format!("receipt-{day}"), format!("terminal:r{day}")] });
            if day == 50 || day == 10 {
                row(path, &Entry::Account { at, account: format!("account-{day}") });
            }
            if day % 20 == 0 {
                row(path, &Entry::Reading { at, windows: Vec::new(), account: None, resets: Vec::new() });
            }
            // A reading of the week each day is in, weeks of seven days
            // ending two days from now: this week's lie either side of what
            // the checkpoint holds.
            let end = now + DAY * 2 - DAY * 7 * ((day + 1) / 7);
            let weekly = fastctx_types::agents::LimitWindow {
                name: "seven_day".into(),
                minutes: Some(10_080),
                used_tenths: 10 * (61 - day) as u32,
                resets_at: Some(end),
                since: None,
            };
            row(path, &Entry::Reading { at, windows: vec![weekly], account: None, resets: Vec::new() });
        }
    }

    /// What a book holds, the same however it came to hold it.
    fn held(book: &Book) -> String {
        let state = state(book);
        let sorted = |values: Vec<String>| {
            let mut values = values;
            values.sort();
            values
        };
        let each = |value: &dyn erased::Each| sorted(value.each());
        serde_json::to_string(&(
            each(&state.totals),
            each(&state.meters),
            each(&state.seen),
            each(&state.keys),
            each(&state.readings),
            each(&state.resets),
            each(&state.week_read),
            each(&state.accounts),
            each(&state.recent),
            sorted(book.modern.iter().cloned().collect()),
            serde_json::to_string(&book.reading).unwrap(),
        ))
        .unwrap()
    }

    mod erased {
        pub trait Each {
            fn each(&self) -> Vec<String>;
        }
        impl<T: serde::Serialize> Each for Vec<T> {
            fn each(&self) -> Vec<String> {
                self.iter().map(|value| serde_json::to_string(value).unwrap()).collect()
            }
        }
    }

    fn read(path: &Path, now: Timestamp) -> Book {
        let mut book = Book::default();
        book.load_at(path, now).unwrap();
        book
    }

    #[test]
    fn a_checkpoint_reads_as_the_days_it_holds() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let now = Timestamp::now();
        ledger(path, now);
        let whole = read(path, now);
        assert!(fastctx_platform::disk::is_there(file(path)), "the first load keeps a checkpoint");
        let kept = fastctx_platform::disk::read(file(path)).unwrap();
        assert_eq!(held(&read(path, now)), held(&whole), "a load from the checkpoint holds what a full read does");

        // Days on, the old checkpoint and the days after it hold what a full
        // read then does, what is past its while by then dropped.
        let later = now + DAY * 20;
        discard(path).unwrap();
        let whole = read(path, later);
        fastctx_platform::publish(&file(path), &kept, fastctx_platform::Permissions::Private).unwrap();
        assert_eq!(held(&read(path, later)), held(&whole));
    }

    #[test]
    fn a_row_for_a_day_it_holds_discards_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let now = Timestamp::now();
        ledger(path, now);
        let mut book = read(path, now);
        let at = |at| Entry::Receipts { at, keys: vec![format!("late-{at}")] };
        book.commit(path, at(now)).unwrap();
        assert!(fastctx_platform::disk::is_there(file(path)), "a row of today leaves it");
        book.commit(path, at(now - DAY * 5)).unwrap();
        assert!(!fastctx_platform::disk::is_there(file(path)), "a row of a day it holds discards it");
        assert_eq!(held(&read(path, now)), held(&book));
    }

    #[test]
    fn a_day_changed_since_is_read_again() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let now = Timestamp::now();
        ledger(path, now);
        read(path, now);
        let files = super::super::ledger::days(path, Timestamp::MIN).unwrap();
        assert!(restore(path, &files).is_some());
        row(path, &Entry::Receipts { at: now - DAY * 30, keys: vec!["written elsewhere".into()] });
        assert!(restore(path, &files).is_none(), "a day it holds that has grown");
        read(path, now);
        fastctx_platform::disk::remove_file(day_file(path, now - DAY * 59)).unwrap();
        let files = super::super::ledger::days(path, Timestamp::MIN).unwrap();
        assert!(restore(path, &files).is_none(), "a day it holds that is gone");
    }
}
