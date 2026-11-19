//! A database opened to be kept as it is when damaged (`Shape::keep_damaged`)
//! is not opened when it is found damaged, and nothing of it is set aside,
//! salvaged or written: whoever keeps it puts it back from a backup.

use fastctx_storage::{Database, Shape};
use std::path::Path;
use std::time::Duration;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, body TEXT NOT NULL);";

fn kept() -> Shape {
    Shape { keep_damaged: true, ..Default::default() }
}

/// What lies in `dir`, by name, after the database's own closing is done.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_damaged_database_kept_as_it_is_is_not_opened_nor_salvaged() {
    let root = std::env::temp_dir().join(format!("fastctx-keep-damaged-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    // One that cannot be read at all.
    let broken = root.join("broken.sqlite3");
    std::fs::write(&broken, b"broken acknowledged database").unwrap();
    let refused = Database::open_shaped(&broken, SCHEMA, kept()).expect_err("a damaged database was opened");
    assert!(refused.is_database_damage(), "{refused}");
    assert_eq!(std::fs::read(&broken).unwrap(), b"broken acknowledged database");
    assert_eq!(names(&root), vec!["broken.sqlite3", "broken.sqlite3-keeper"]);

    // One marked damaged as it was read: left as it is, its rows all there.
    let marked = root.join("marked.sqlite3");
    {
        let database = Database::open_shaped(&marked, SCHEMA, kept()).unwrap();
        database
            .write_now(64, |db| {
                db.execute("INSERT INTO events(id,body) VALUES(1,'kept')", [])?;
                Ok(())
            })
            .unwrap();
    }
    std::thread::sleep(Duration::from_millis(500));
    let before = std::fs::read(&marked).unwrap();
    std::fs::write(root.join("marked.sqlite3.damaged"), "a page SQLite could not read").unwrap();
    let refused = Database::open_shaped(&marked, SCHEMA, kept()).expect_err("a database marked damaged was opened");
    assert!(refused.is_database_damage(), "{refused}");
    assert_eq!(std::fs::read(&marked).unwrap(), before);
    assert!(!root.join("marked.sqlite3.quarantine").exists(), "a database kept as it is was set aside");

    // The mark taken away by whoever put it right, it opens as it was.
    std::fs::remove_file(root.join("marked.sqlite3.damaged")).unwrap();
    let database = Database::open_shaped(&marked, SCHEMA, kept()).unwrap();
    let rows = database
        .read_now(|db| Ok(db.query_row("SELECT count(*) FROM events", [], |row| row.get::<_, i64>(0))?))
        .unwrap();
    assert_eq!(rows, 1);
    drop(database);
    let _ = std::fs::remove_dir_all(&root);
}
