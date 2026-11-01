//! Storage performance and process-death drill. All state lives below --root.

use fastctx_storage::{
    Database, Error, ErrorKind, Priority,
    versions::{self, Migration},
};
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, body TEXT NOT NULL);";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || !matches!(args[1].as_str(), "--root" | "--writer") {
        return Err("usage: storage_drill --root ABSOLUTE_EMPTY_DIRECTORY".into());
    }
    let root = Path::new(&args[2]);
    if !root.is_absolute() {
        return Err("the drill needs an absolute root".into());
    }
    if args[1] == "--writer" {
        return worker(root);
    }
    if root.exists() {
        return Err("the drill root must not exist".into());
    }
    std::fs::create_dir_all(root)?;
    performance(root)?;
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(concurrency(root))?;
    crashes(root)?;
    corruption(root)?;
    println!("storage drill passed");
    Ok(())
}

fn performance(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let database = Database::open(&root.join("performance.sqlite3"), SCHEMA)?;
    for start in (0..100_000i64).step_by(128) {
        database.write_now_at(Priority::Background, 128 * 128, move |db| {
            let mut insert = db.prepare_cached("INSERT INTO events(id,body) VALUES(?1,?2)")?;
            for id in start..(start + 128).min(100_000) {
                insert.execute((id, format!("event {id}")))?;
            }
            Ok(())
        })?;
    }
    let mut pages = Vec::new();
    for _ in 0..200 {
        let start = Instant::now();
        let rows: Vec<(i64, String)> = database.read_now(|db| {
            Ok(db
                .prepare_cached("SELECT id,body FROM events WHERE id>=99900 ORDER BY id LIMIT 100")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?)
        })?;
        assert_eq!(rows.len(), 100);
        assert_eq!(rows[99], (99999, "event 99999".into()));
        pages.push(start.elapsed());
    }
    pages.sort();
    assert!(pages[197] < Duration::from_millis(300), "first page p99 exceeded 300 ms");
    let stats = database.statistics();
    let count: u64 = stats.write_micros.iter().sum();
    let slow = stats.writes_over_budget;
    println!("100000 events: first page p99 {:?}; SQL operations {}, operations >2 ms {}", pages[197], count, slow);
    assert!(slow * 100 <= count, "more than 1% of SQL operations took >2 ms");
    let bad = database.write_now(32, |db| {
        db.execute("INSERT INTO events(id,body) VALUES(100001,'must roll back')", [])?;
        Err::<(), _>(Error::new("injected operation failure"))
    });
    assert!(bad.is_err());
    assert_eq!(
        database.read_now(|db| Ok(db.query_row("SELECT count(*) FROM events", [], |row| row.get::<_, i64>(0))?))?,
        100000
    );
    database.write_now(0, |db| {
        versions::apply(
            db,
            "drill",
            &[
                Migration { version: 1, sql: "CREATE TABLE schema_one (value INTEGER);" },
                Migration { version: 2, sql: "CREATE TABLE schema_two (value INTEGER);" },
            ],
        )
    })?;
    let rejected = database.write_now(0, |db| {
        versions::apply(
            db,
            "drill",
            &[
                Migration { version: 1, sql: "" },
                Migration { version: 2, sql: "" },
                Migration { version: 3, sql: "INVALID SQL" },
            ],
        )
    });
    assert!(rejected.is_err());
    assert_eq!(
        database.read_now(|db| Ok(db.query_row(
            "SELECT version FROM storage_versions WHERE component='drill'",
            [],
            |row| row.get::<_, u32>(0)
        )?))?,
        2
    );
    Ok(())
}

async fn concurrency(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let database = Database::open(&root.join("concurrency.sqlite3"), SCHEMA)?;
    let (started, waiting) = std::sync::mpsc::channel();
    let (release, hold) = std::sync::mpsc::channel();
    let first = database.clone();
    let blocker = tokio::spawn(async move {
        first
            .write(0, move |_| {
                started.send(()).expect("drill observer");
                hold.recv().expect("drill release");
                Ok(())
            })
            .await
    });
    while waiting.try_recv().is_err() {
        tokio::task::yield_now().await;
    }
    let mut background = Vec::new();
    for id in 0..100i64 {
        let db = database.clone();
        background.push(tokio::spawn(async move {
            db.write_at(Priority::Background, 32, move |db| {
                db.execute("INSERT INTO events(id,body) VALUES(?1,'background')", [id])?;
                Ok(())
            })
            .await
        }));
    }
    let db = database.clone();
    let important = tokio::spawn(async move {
        db.write_at(Priority::Interactive, 0, |db| {
            Ok(db.query_row("SELECT count(*) FROM events", [], |row| row.get::<_, i64>(0))?)
        })
        .await
    });
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
    release.send(())?;
    blocker.await??;
    let preceding = important.await??;
    assert!(preceding < 10, "interactive write waited behind {preceding} background writes");
    for job in background {
        job.await??;
    }
    let (notified, received) = std::sync::mpsc::channel();
    let db = database.clone();
    let cancelled = tokio::spawn(async move {
        db.write_then(
            16,
            |db| {
                db.execute("INSERT INTO events(id,body) VALUES(1000,'retained')", [])?;
                Ok(())
            },
            move |_| {
                notified.send(()).expect("commit observer");
            },
        )
        .await
    });
    while database.statistics().queued == 0 {
        tokio::task::yield_now().await;
        if cancelled.is_finished() {
            break;
        }
    }
    cancelled.abort();
    drop(database);
    tokio::task::spawn_blocking(move || received.recv_timeout(Duration::from_secs(5))).await??;
    let database = Database::open(&root.join("concurrency.sqlite3"), SCHEMA)?;
    assert_eq!(
        database
            .read(|db| Ok(db.query_row("SELECT body FROM events WHERE id=1000", [], |row| row.get::<_, String>(0))?))
            .await?,
        "retained"
    );
    let invalid = database.write(usize::MAX, |_| Ok(())).await.unwrap_err();
    assert_eq!(invalid.kind(), ErrorKind::Invalid);
    println!(
        "priority/cancellation: interactive followed {preceding} background writes; cancelled caller retained its committed row and notification"
    );
    Ok(())
}

fn worker(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let database = Database::open(path, SCHEMA)?;
    let mut out = std::io::stdout().lock();
    for id in 0..1_000_000i64 {
        database.write_now(64, move |db| {
            db.execute("INSERT OR IGNORE INTO events(id,body) VALUES(?1,?2)", (id, format!("durable {id}")))?;
            Ok(())
        })?;
        writeln!(out, "{id}")?;
        out.flush()?;
    }
    Err("worker unexpectedly completed before being killed".into())
}

fn crashes(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    for round in 0..16u64 {
        let path = root.join(format!("crash-{round}.sqlite3"));
        let mut child = Command::new(&executable).arg("--writer").arg(&path).stdout(Stdio::piped()).spawn()?;
        let target = 32 + (round * 7919) % 193;
        let mut lines = std::io::BufReader::new(child.stdout.take().expect("worker stdout")).lines();
        let mut acknowledged = 0i64;
        for _ in 0..target {
            acknowledged = lines.next().ok_or("worker stopped before acknowledgement")??.parse::<i64>()? + 1;
        }
        child.kill()?;
        assert!(!child.wait()?.success(), "fault injection failed: worker exited successfully");
        drop(lines);
        let database = Database::open(&path, SCHEMA)?;
        let count = database.read_now(move |db| {
            Ok(db.query_row(
                "SELECT count(*) FROM events WHERE id<?1 AND body='durable '||id",
                [acknowledged],
                |row| row.get::<_, i64>(0),
            )?)
        })?;
        assert_eq!(count, acknowledged, "lost acknowledged rows in kill round {round}");
    }
    println!("process death: 16 observed forced terminations, zero acknowledged rows lost");
    Ok(())
}

fn corruption(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // Unreadable files are set aside whole and an empty database opens in
    // their place, saying so: the files stay as they were.
    let path = root.join("broken.sqlite3");
    for (suffix, bytes) in [
        ("", b"broken acknowledged database".as_slice()),
        ("-wal", b"uncheckpointed committed bytes".as_slice()),
        ("-shm", b"shared memory".as_slice()),
    ] {
        std::fs::write(format!("{}{suffix}", path.display()), bytes)?;
    }
    let salvaged = Database::open_or_unavailable(&path, SCHEMA, |_| Ok(()));
    assert!(salvaged.fault().is_none(), "a damaged database left no database: {:?}", salvaged.fault());
    assert!(salvaged.salvaged().is_some_and(|report| report.contains("nothing could be read")));
    salvaged.write_now(0, |db| {
        db.execute("INSERT INTO events(id,body) VALUES(1,'after salvage')", [])?;
        Ok(())
    })?;
    let directory =
        std::fs::read_dir(format!("{}.quarantine", path.display()))?.next().ok_or("no quarantine directory")??.path();
    assert_eq!(std::fs::read(directory.join("broken.sqlite3"))?, b"broken acknowledged database");
    assert_eq!(std::fs::read(directory.join("broken.sqlite3-wal"))?, b"uncheckpointed committed bytes");
    assert!(!Path::new(&format!("{}.unavailable", path.display())).exists());
    assert!(fastctx_platform::health::summary().damaged.is_some(), "the health does not tell of the damage");
    drop(salvaged);

    // A start cut short after moving the database but before its log: the
    // next one moves the log too, so it never meets the new database.
    let cut = root.join("cut.sqlite3");
    std::fs::write(&cut, b"broken database")?;
    std::fs::write(format!("{}-wal", cut.display()), b"stale log")?;
    let set_aside = root.join("cut.sqlite3.quarantine").join("0a-1-1");
    std::fs::create_dir_all(&set_aside)?;
    std::fs::rename(&cut, set_aside.join("cut.sqlite3"))?;
    std::fs::write(format!("{}.unavailable", cut.display()), "fastctx-sqlite-quarantine-v1\n0a-1-1\ninjected")?;
    let resumed = Database::open_or_unavailable(&cut, SCHEMA, |_| Ok(()));
    assert!(resumed.fault().is_none(), "the interrupted salvage did not finish: {:?}", resumed.fault());
    assert_eq!(std::fs::read(set_aside.join("cut.sqlite3-wal"))?, b"stale log");
    assert!(
        resumed.read_now(|db| Ok(db.query_row("SELECT count(*) FROM events", [], |row| row.get::<_, i64>(0))?))? == 0
    );
    drop(resumed);

    // An empty file without a log never held a commit: it opens as new.
    let empty = root.join("empty.sqlite3");
    std::fs::write(&empty, [])?;
    let opened = Database::open_or_unavailable(&empty, SCHEMA, |_| Ok(()));
    assert!(opened.fault().is_none() && opened.salvaged().is_none());
    drop(opened);

    // A page broken in the middle of a table: the cheap checks let the
    // database open, the first read through it marks it damaged, and the
    // next open salvages every row before that page.
    let paged = root.join("paged.sqlite3");
    {
        let database = Database::open(&paged, SCHEMA)?;
        for start in (0..20_000i64).step_by(500) {
            database.write_now(500 * 64, move |db| {
                let mut insert = db.prepare_cached("INSERT INTO events(id,body) VALUES(?1,?2)")?;
                for id in start..start + 500 {
                    insert.execute((id, format!("row {id:08}")))?;
                }
                Ok(())
            })?;
        }
    }
    closed(&paged)?;
    let length = std::fs::metadata(&paged)?.len();
    let page = 4096u64;
    let broken = (length / page).saturating_sub(3) * page + 512;
    {
        use std::io::{Seek, SeekFrom};
        let mut file = std::fs::OpenOptions::new().write(true).open(&paged)?;
        file.seek(SeekFrom::Start(broken))?;
        file.write_all(&[0xA5; 2048])?;
        file.sync_all()?;
    }
    {
        let database = Database::open_or_unavailable(&paged, SCHEMA, |_| Ok(()));
        assert!(database.fault().is_none() && database.salvaged().is_none());
        let read = database.read_now(|db| {
            Ok(db.query_row("SELECT count(*), sum(length(body)) FROM events", [], |row| row.get::<_, i64>(0))?)
        });
        let error = read.expect_err("a broken page read without error");
        assert!(error.is_database_damage(), "not taken for damage: {error}");
        assert!(Path::new(&format!("{}.damaged", paged.display())).exists());
    }
    closed(&paged)?;
    let database = Database::open_or_unavailable(&paged, SCHEMA, |_| Ok(()));
    let report = database.salvaged().ok_or("the damaged database was not salvaged")?.to_string();
    let (rows, intact): (i64, i64) = database.read_now(|db| {
        Ok(db.query_row("SELECT count(*), coalesce(sum(body = printf('row %08d', id)), 0) FROM events", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?)
    })?;
    assert!(rows > 0 && rows < 20_000, "salvaged {rows} of 20000 rows");
    assert_eq!(rows, intact, "a salvaged row came over changed");
    assert!(!Path::new(&format!("{}.damaged", paged.display())).exists());
    drop(database);

    // A record a service cannot read is the service's, not the database's.
    let logical = root.join("logical.sqlite3");
    let database = Database::open(&logical, SCHEMA)?;
    let error = database
        .read_now(|_| Err::<(), _>(Error::with_kind(ErrorKind::Corrupt, "one record is malformed")))
        .unwrap_err();
    assert!(!error.is_database_damage());
    assert!(!Path::new(&format!("{}.damaged", logical.display())).exists());
    database.write_now(0, |db| {
        db.execute("INSERT INTO events(id,body) VALUES(1,'healthy')", [])?;
        Ok(())
    })?;

    // A writer that waits for a write would wait for itself: refused at
    // once, not after the time an answer may take.
    let inner = database.clone();
    let started = Instant::now();
    let nested = database.write_now(0, move |_| inner.write_now(0, |_| Ok(())));
    assert!(nested.is_err() && started.elapsed() < Duration::from_secs(5));
    println!(
        "corruption: unreadable files kept and an empty database opened in their place; an interrupted salvage finished without its stale log; an empty file opened as new; a broken page salvaged {rows} of 20000 rows unchanged ({}); a record error left storage healthy; a writer refused to wait for itself",
        report.lines().filter(|line| line.contains("could not be read")).count()
    );
    Ok(())
}

/// Waits until the database at `path` was closed: its writer emptied its log
/// into the file when it fell idle (and took it away as it closed, where the
/// system let it), and stopped within the second it waits for more work.
fn closed(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let log = format!("{}-wal", path.display());
    let deadline = Instant::now() + Duration::from_secs(20);
    while std::fs::metadata(&log).is_ok_and(|log| log.len() > 0) {
        if Instant::now() > deadline {
            return Err("the database did not empty its log".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_secs(2));
    Ok(())
}
