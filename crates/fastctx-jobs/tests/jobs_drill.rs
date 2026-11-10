//! Runs jobs through their queue on this machine: every piece of work asked
//! for is done, one of a unit at a time and a few of a kind at once; asking
//! again merges; failures wait longer each time and are set aside; a run
//! that overstays is dropped; what a stopped process left running runs again.
//!
//! jobs_drill --root ABSOLUTE_DIRECTORY_THAT_DOES_NOT_EXIST

use fastctx_jobs::{Job, Jobs, Kind, Operation, Outcome, Run, Standing, Taken, accept, settle, submit};
use fastctx_storage::Database;
use fastctx_storage::rusqlite::params;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Outcome_<T> = Result<T, Box<dyn std::error::Error>>;

const COUNTED: usize = 5_000;
const UNITS: usize = 100;
const AT_ONCE: usize = 8;

#[derive(Default)]
struct Seen {
    ran: Mutex<HashMap<String, Vec<Vec<u8>>>>,
    now: AtomicUsize,
    most: AtomicUsize,
    by_unit: Mutex<HashMap<String, usize>>,
    overlapped: AtomicUsize,
}

fn main() -> Outcome_<()> {
    let args: Vec<String> = std::env::args().collect();
    let root = match args.iter().position(|arg| arg == "--root").and_then(|at| args.get(at + 1)) {
        Some(root) => Path::new(root),
        None => return Err("usage: jobs_drill --root ABSOLUTE_DIRECTORY_THAT_DOES_NOT_EXIST".into()),
    };
    if !root.is_absolute() || root.exists() {
        return Err("the drill needs an absolute root that does not exist".into());
    }
    std::fs::create_dir_all(root)?;
    let database = Database::open(&root.join("jobs.sqlite3"), fastctx_jobs::SCHEMA)?;
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?.block_on(drill(database))?;
    println!("jobs drill passed");
    Ok(())
}

fn job(kind: &'static str, key: impl Into<String>, unit: impl Into<String>, payload: &[u8]) -> Job {
    Job { kind, key: key.into(), unit: unit.into(), payload: payload.to_vec() }
}

async fn waiting(database: &Database, kind: &'static str) -> Outcome_<i64> {
    Ok(database
        .read(move |db| Ok(db.query_row("SELECT COUNT(*) FROM jobs WHERE kind = ?1", [kind], |row| row.get(0))?))
        .await?)
}

async fn emptied(database: &Database, kind: &'static str, within: Duration) -> Outcome_<()> {
    let began = Instant::now();
    while waiting(database, kind).await? > 0 {
        if began.elapsed() > within {
            return Err(format!("jobs of {kind} were still there after {within:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Ok(())
}

async fn drill(database: Database) -> Outcome_<()> {
    let jobs = Jobs::new(database.clone());
    let seen = Arc::new(Seen::default());

    let counting = seen.clone();
    let count: Run = Arc::new(move |taken: Taken| {
        let seen = counting.clone();
        Box::pin(async move {
            let now = seen.now.fetch_add(1, Ordering::AcqRel) + 1;
            seen.most.fetch_max(now, Ordering::AcqRel);
            {
                let mut by_unit = seen.by_unit.lock().expect("units");
                let running = by_unit.entry(taken.unit.clone()).or_insert(0);
                *running += 1;
                if *running > 1 {
                    seen.overlapped.fetch_add(1, Ordering::AcqRel);
                }
            }
            // A timer would measure the timer: Windows wakes one every 15 ms.
            tokio::task::yield_now().await;
            seen.ran.lock().expect("ran").entry(taken.key.clone()).or_default().push(taken.payload);
            *seen.by_unit.lock().expect("units").get_mut(&taken.unit).expect("counted") -= 1;
            seen.now.fetch_sub(1, Ordering::AcqRel);
            Outcome::Done
        })
    });
    jobs.register(Kind { name: "count", at_once: AT_ONCE, within: Duration::from_secs(5), tries: 3 }, count);

    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let merged = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let (held, payloads) = (gate.clone(), merged.clone());
    let merge: Run = Arc::new(move |taken: Taken| {
        let (held, payloads) = (held.clone(), payloads.clone());
        Box::pin(async move {
            payloads.lock().expect("payloads").push(taken.payload);
            held.acquire().await.expect("the gate stays open").forget();
            Outcome::Done
        })
    });
    jobs.register(Kind { name: "merge", at_once: 1, within: Duration::from_secs(30), tries: 3 }, merge);

    let attempts = Arc::new(Mutex::new(Vec::<u32>::new()));
    let tried = attempts.clone();
    let fail: Run = Arc::new(move |taken: Taken| {
        tried.lock().expect("attempts").push(taken.attempt);
        Box::pin(async { Outcome::Failed("it cannot be done".into()) })
    });
    jobs.register(Kind { name: "fail", at_once: 1, within: Duration::from_secs(5), tries: 3 }, fail);

    let waited = Arc::new(AtomicUsize::new(0));
    let waiting_runs = waited.clone();
    let wait: Run = Arc::new(move |_: Taken| {
        // What it waits for is not there the first time it runs.
        let first = waiting_runs.fetch_add(1, Ordering::AcqRel) == 0;
        Box::pin(async move { if first { Outcome::Again { after: Duration::from_secs(3600) } } else { Outcome::Done } })
    });
    jobs.register(Kind { name: "wait", at_once: 1, within: Duration::from_secs(5), tries: 3 }, wait);

    let operating = Arc::new(tokio::sync::Semaphore::new(0));
    let operated = Arc::new(AtomicUsize::new(0));
    let (opening, counted, settling) = (operating.clone(), operated.clone(), jobs.clone());
    let settles = database.clone();
    let operate: Run = Arc::new(move |taken: Taken| {
        let (opening, counted, jobs, database) = (opening.clone(), counted.clone(), settling.clone(), settles.clone());
        Box::pin(async move {
            opening.acquire().await.expect("the gate stays open").forget();
            counted.fetch_add(1, Ordering::AcqRel);
            let operation = Operation { kind: "open", unit: taken.unit, id: taken.key };
            let settled = operation.clone();
            let wrote = database
                .write(256, move |db| {
                    settle(db, &settled, &Standing::Done(b"opened".to_vec()), Duration::from_secs(3600))
                })
                .await;
            jobs.told(&operation);
            if wrote.is_ok() { Outcome::Done } else { Outcome::Failed("its settling was not written".into()) }
        })
    });
    jobs.register(Kind { name: "operate", at_once: 1, within: Duration::from_secs(30), tries: 3 }, operate);

    let slow: Run = Arc::new(|_| {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(600)).await;
            Outcome::Done
        })
    });
    jobs.register(Kind { name: "slow", at_once: 1, within: Duration::from_millis(100), tries: 1 }, slow);

    // What a stopped process left running runs again.
    database
        .write(256, |db| {
            db.execute(
                "INSERT INTO jobs (kind, key, unit, payload, state, due) VALUES ('count', 'left-running', 'left', x'01', 1, 0)",
                [],
            )?;
            Ok(())
        })
        .await?;
    jobs.start().await?;
    emptied(&database, "count", Duration::from_secs(10)).await?;
    if seen.ran.lock().expect("ran").remove("left-running").map(|runs| runs.len()) != Some(1) {
        return Err("a job left running by a stopped process did not run once".into());
    }

    // Every job runs once, one of a unit at a time, a few of the kind at once.
    let began = Instant::now();
    for start in (0..COUNTED).step_by(100) {
        database
            .write(64 * 1024, move |db| {
                for number in start..start + 100 {
                    submit(db, &job("count", format!("k{number}"), format!("u{}", number % UNITS), b"once"))?;
                }
                Ok(())
            })
            .await?;
        jobs.stir("count");
    }
    emptied(&database, "count", Duration::from_secs(120)).await?;
    let took = began.elapsed();
    {
        let ran = seen.ran.lock().expect("ran");
        if ran.len() != COUNTED || ran.values().any(|runs| runs.len() != 1) {
            return Err(format!("{} of {COUNTED} jobs ran, or some ran more than once", ran.len()).into());
        }
    }
    let (most, overlapped) = (seen.most.load(Ordering::Acquire), seen.overlapped.load(Ordering::Acquire));
    if most > AT_ONCE || overlapped > 0 {
        return Err(
            format!("{most} ran at once of {AT_ONCE} allowed; a unit's jobs overlapped {overlapped} times").into()
        );
    }
    println!(
        "{COUNTED} jobs of {UNITS} units in {took:.2?}, {:.0} a second, {most} at once at most",
        COUNTED as f64 / took.as_secs_f64()
    );

    // The queue measures itself: each run timed, and the table counted
    // when it is asked after.
    jobs.measure().await?;
    let readings = fastctx_platform::readings::snapshot();
    let ran = readings.timings.iter().find(|timing| timing.name == "job.ran.count").map(|timing| timing.count);
    let left = readings.levels.iter().find(|level| level.name == "job.waiting.count").map(|level| level.value);
    if ran.is_none_or(|ran| ran < COUNTED as u64) || left != Some(0) {
        return Err(format!("the queue's measures say {ran:?} runs and {left:?} waiting").into());
    }

    // A job that waits for more than time is run again once it is roused,
    // not when its hour is up.
    database.write(256, |db| submit(db, &job("wait", "one", "unit", b""))).await?;
    jobs.stir("wait");
    let began = Instant::now();
    while waited.load(Ordering::Acquire) < 1 {
        if began.elapsed() > Duration::from_secs(10) {
            return Err("a submitted job did not run".into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    if waited.load(Ordering::Acquire) != 1 || waiting(&database, "wait").await? != 1 {
        return Err("a job that asked to be run again in an hour did not wait".into());
    }
    let roused = database.write(256, |db| fastctx_jobs::rouse(db, "wait")).await?;
    jobs.stir("wait");
    emptied(&database, "wait", Duration::from_secs(10)).await?;
    if roused != 1 || waited.load(Ordering::Acquire) != 2 {
        return Err(
            format!("rousing {roused} jobs ran the waiting one {} times", waited.load(Ordering::Acquire)).into()
        );
    }

    // Asked for again while it waits, a job is one job with what was asked
    // last; asked for while it runs, it runs once more.
    for payload in [b"first".as_slice(), b"second", b"third"] {
        let payload = payload.to_vec();
        database.write(256, move |db| submit(db, &job("merge", "one", "unit", &payload))).await?;
    }
    jobs.stir("merge");
    let began = Instant::now();
    while merged.lock().expect("payloads").is_empty() {
        if began.elapsed() > Duration::from_secs(5) {
            return Err("the merged job did not start".into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    database.write(256, |db| submit(db, &job("merge", "one", "unit", b"while running"))).await?;
    gate.add_permits(2);
    emptied(&database, "merge", Duration::from_secs(10)).await?;
    let payloads = merged.lock().expect("payloads").clone();
    if payloads != [b"third".to_vec(), b"while running".to_vec()] {
        return Err(format!("asking again did not merge as it should: {payloads:?}").into());
    }

    // A job that keeps failing waits longer each time, then is set aside
    // until it is asked for again.
    let began = Instant::now();
    database.write(256, |db| submit(db, &job("fail", "never", "unit", b""))).await?;
    jobs.stir("fail");
    let aside = loop {
        let state: (i64, Option<String>) = database
            .read(|db| {
                Ok(db.query_row("SELECT state, failure FROM jobs WHERE kind = 'fail'", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?)
            })
            .await?;
        if state.0 == 2 {
            break state.1;
        }
        if began.elapsed() > Duration::from_secs(10) {
            return Err("a job that keeps failing was not set aside".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let waited = began.elapsed();
    if *attempts.lock().expect("attempts") != [1, 2, 3] || waited < Duration::from_millis(700) {
        return Err(
            format!("a failing job was tried {:?} within {waited:?}", attempts.lock().expect("attempts")).into()
        );
    }
    if aside.as_deref() != Some("it cannot be done") {
        return Err("a job set aside does not say why".into());
    }
    database.write(256, |db| submit(db, &job("fail", "never", "unit", b""))).await?;
    jobs.stir("fail");
    let began = Instant::now();
    while attempts.lock().expect("attempts").len() < 4 {
        if began.elapsed() > Duration::from_secs(5) {
            return Err("a job set aside did not run when asked for again".into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // An operation is accepted once however often it is asked for, and
    // whoever waits for it learns what it came to when its job settles it.
    let operation = Operation { kind: "open", unit: "unit".into(), id: "asked-once".into() };
    for asked in 0..3 {
        let (operation, job) = (operation.clone(), job("operate", "asked-once", "unit", b""));
        let known = database
            .write(256, move |db| {
                let known = accept(db, &operation)?;
                if known.is_none() {
                    submit(db, &job)?;
                }
                Ok(known)
            })
            .await?;
        if (asked == 0) != known.is_none() {
            return Err(format!("asked {asked} times, an operation stood as {known:?}").into());
        }
    }
    let early = jobs.settled(&operation, Duration::from_millis(50)).await?;
    if early != Some(Standing::Accepted) || operated.load(Ordering::Acquire) != 0 {
        return Err(format!("an operation not done yet stood as {early:?}").into());
    }
    let waiting = {
        let (jobs, operation) = (jobs.clone(), operation.clone());
        tokio::spawn(async move { jobs.settled(&operation, Duration::from_secs(10)).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let began = Instant::now();
    operating.add_permits(1);
    jobs.stir("operate");
    let told = waiting.await??;
    if told != Some(Standing::Done(b"opened".to_vec())) || began.elapsed() > Duration::from_secs(5) {
        return Err(format!("whoever waited for an operation was told {told:?} after {:?}", began.elapsed()).into());
    }
    if operated.load(Ordering::Acquire) != 1 {
        return Err("an operation asked for three times was not done once".into());
    }

    // A run that overstays is dropped and counts as a failure.
    database.write(256, |db| submit(db, &job("slow", "long", "unit", b""))).await?;
    jobs.stir("slow");
    let began = Instant::now();
    loop {
        let failure: Option<String> = database
            .read(|db| {
                Ok(db
                    .query_row("SELECT failure FROM jobs WHERE kind = 'slow' AND state = ?1", params![2], |row| {
                        row.get(0)
                    })
                    .unwrap_or(None))
            })
            .await?;
        if let Some(failure) = failure {
            if !failure.contains("did not finish within") {
                return Err(format!("an overstaying run was recorded as: {failure}").into());
            }
            break;
        }
        if began.elapsed() > Duration::from_secs(5) {
            return Err("an overstaying run was not dropped".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}
