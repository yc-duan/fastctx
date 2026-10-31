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
    let slow: u64 = stats.write_micros.iter().skip(12).sum();
    println!("100000 events: first page p99 {:?}; SQL operations {}, operations >=2048 us {}", pages[197], count, slow);
    assert!(slow * 100 <= count, "more than 1% of SQL operations took >=2048 us");
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
