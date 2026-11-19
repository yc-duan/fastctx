//! A database this process has open keeps its locks however else the
//! process looks at the file. Closing a descriptor of the file anywhere in
//! the process drops them on Unix (SQLite, "How To Corrupt An SQLite
//! Database File", 2.2): another process that then opens the database and
//! closes it takes itself for its last user and folds and deletes the log
//! under it, as an outside backup did to the hub's on 2026-11-19. Linux
//! shows each process's locks in `/proc/locks`.

#![cfg(target_os = "linux")]

use fastctx_storage::Database;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, body TEXT NOT NULL);";

/// This process's locks on the file at `path`.
fn held(path: &Path) -> Vec<String> {
    let ino = std::fs::metadata(path).expect("the database").ino();
    let pid = format!(" {} ", std::process::id());
    std::fs::read_to_string("/proc/locks")
        .expect("the system's locks")
        .lines()
        .filter(|line| line.contains(&pid) && line.split_whitespace().any(|field| field.ends_with(&format!(":{ino}"))))
        .map(str::to_string)
        .collect()
}

#[test]
fn looking_at_an_open_database_keeps_its_locks() {
    let root = std::env::temp_dir().join(format!("fastctx-locks-kept-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a root");
    let path = root.join("kept.sqlite3");
    // A database there already, opened again as a start opens it: its
    // connections take their shared lock on the file as they first read.
    drop(Database::open(&path, SCHEMA).expect("made"));
    let database = Database::open(&path, SCHEMA).expect("opened");
    database
        .write_now(64, |db| {
            db.execute("INSERT INTO events(id,body) VALUES(1,'kept')", [])?;
            Ok(())
        })
        .expect("written");
    let rows = database
        .read_now(|db| Ok(db.query_row("SELECT count(*) FROM events", [], |row| row.get::<_, i64>(0))?))
        .expect("read");
    assert_eq!(rows, 1);
    let before = held(&path);
    assert!(!before.is_empty(), "the open database holds no lock on its file");
    // What looks at the file's identity while the database is open: the
    // format upgrade's registry does as every start ends. Asked to stop a
    // backup of a database still open, it says so, having looked first.
    let _ = fastctx_storage::format_upgrade::stop_format_backup(&path, Duration::from_millis(10));
    assert_eq!(held(&path), before, "looking at the open database's file dropped this process's locks on it");
    drop(database);
    let _ = std::fs::remove_dir_all(&root);
}
