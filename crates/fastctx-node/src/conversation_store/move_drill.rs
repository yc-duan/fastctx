//! A conversation moved by hand and back: exported, imported on another
//! machine, let go here by the fingerprint that machine keeps, used there and
//! taken back whole. Letting it go leaves none of its rows; a release stopped
//! before its directory moved finishes when run again; once deleted, an
//! offer of it is refused rather than bringing it back.
use super::{bundle, schema, transfer, transfer_drill::wait};
use fastctx_platform::disk;
use fastctx_storage::{Priority, rusqlite::Connection};
use fastctx_types::layout::WorldDir;
use std::path::Path;

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let id: fastctx_types::ResidentId = "claude-5e1f2a3b".parse()?;
    let session = "0b6c3a52-7e0d-4c0e-9a51-3f2a6d1e8c47";
    let (home_root, away_root) = (root.join("home"), root.join("away"));
    let (home, away) = (WorldDir::under(&home_root), WorldDir::under(&away_root));
    let directory = home.conversation(&id);

    let database = crate::record_database::open(&home);
    assert!(database.fault().is_none(), "{:?}", database.fault());
    let service = crate::record_database::register(&home, &database)?;
    disk::create_dir_all(directory.path())?;
    crate::records::write_at(&directory, Priority::Normal, 1024, |rows| {
        rows.put("pending-inputs", "input-a", &true)?;
        rows.put("goal-local", "current", &"goal")?;
        rows.connection()
            .execute("INSERT OR REPLACE INTO node_record_state VALUES('legacy-discovery-complete',1)", [])?;
        Ok(())
    })?;
    database.write_now_at(Priority::Background, 64 * 1024, |db| {
        for number in 0..300u64 {
            db.execute(
                "INSERT INTO session_events(conversation,sequence,identity,body) VALUES('claude-5e1f2a3b',?1,?2,?3)",
                ((number + 1).to_be_bytes().as_slice(), format!("event-{number}"), b"{}".as_slice()),
            )?;
        }
        Ok(())
    })?;
    let registration = fastctx_hosts::claude::exchange::sessions(home.path()).join(session);
    disk::create_dir_all(directory.claude_exchange(session))?;
    fastctx_platform::directory_alias::create(&directory.claude_exchange(session), &registration)?;
    drop(service);
    drop(database);

    let exported = crate::export_conversation(&home_root, id.as_str())?;
    let arriving = away.conversation(&id);
    disk::create_dir_all(arriving.path())?;
    disk::copy(&exported.bundle, arriving.path().join(bundle::FILE))?;
    let away_database = crate::record_database::open(&away);
    assert!(away_database.fault().is_none(), "{:?}", away_database.fault());
    let service = crate::record_database::register(&away, &away_database)?;
    service.start()?;
    wait(&away, &away_database, id.as_str())?;
    let taken = imported(&away_root, id.as_str())?;
    assert_eq!(taken.as_deref(), Some(exported.fingerprint.as_str()));

    // Refused while the node runs, for another export, and once its rows
    // changed, each leaving every row.
    let raw = Connection::open(home.path().join("node.sqlite3"))?;
    let before = rows(&raw, id.as_str())?;
    let owner = fastctx_platform::FileLock::try_acquire(&home.node_lock())?.expect("the stopped node's lock");
    let running = crate::release_conversation(&home_root, id.as_str(), &exported.fingerprint).unwrap_err();
    assert_eq!(running.kind(), std::io::ErrorKind::WouldBlock, "{running}");
    drop(owner);
    let other = crate::release_conversation(&home_root, id.as_str(), &"0".repeat(64)).unwrap_err();
    assert!(other.to_string().contains("not the one taken"), "{other}");
    raw.execute(
        "UPDATE session_events SET body=X'7B7D20' WHERE conversation=?1 AND identity='event-7'",
        [id.as_str()],
    )?;
    let changed = crate::release_conversation(&home_root, id.as_str(), &exported.fingerprint).unwrap_err();
    assert!(changed.to_string().contains("changed since"), "{changed}");
    raw.execute("UPDATE session_events SET body=X'7B7D' WHERE conversation=?1 AND identity='event-7'", [id.as_str()])?;
    assert!(before > 300);
    assert_eq!(rows(&raw, id.as_str())?, before);

    let kept = crate::release_conversation(&home_root, id.as_str(), &exported.fingerprint)?;
    assert!(kept.starts_with(home.released()) && disk::exists(kept.join(bundle::FILE))?);
    assert!(!disk::exists(directory.path())?);
    assert!(disk::symlink_metadata(&registration).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound));
    assert_eq!(rows(&raw, id.as_str())?, 0);
    // A release stopped before its directory moved finishes the same way.
    disk::rename(&kept, directory.path())?;
    assert_eq!(crate::release_conversation(&home_root, id.as_str(), &exported.fingerprint)?, kept);
    assert!(!disk::exists(directory.path())?);
    drop(raw);

    crate::records::write_at(&arriving, Priority::Normal, 64, |rows| {
        rows.put("new", "away", &true)?;
        rows.connection()
            .execute("INSERT OR REPLACE INTO node_record_state VALUES('legacy-discovery-complete',1)", [])?;
        Ok(())
    })?;
    drop(service);
    drop(away_database);
    let back = crate::export_conversation(&away_root, id.as_str())?;
    assert_ne!(back.fingerprint, exported.fingerprint);
    disk::create_dir_all(directory.path())?;
    disk::copy(&back.bundle, directory.path().join(bundle::FILE))?;
    let database = crate::storage_drill::reopen(&home)?;
    let service = crate::record_database::register(&home, &database)?;
    service.start()?;
    wait(&home, &database, id.as_str())?;
    assert_eq!(imported(&home_root, id.as_str())?.as_deref(), Some(back.fingerprint.as_str()));
    let (goal, used) = crate::records::read(&directory, |rows| {
        Ok((rows.get::<String>("goal-local", "current")?, rows.get::<bool>("new", "away")?))
    })?;
    assert_eq!((goal.as_deref(), used), (Some("goal"), Some(true)));
    let left = crate::release_conversation(&away_root, id.as_str(), &back.fingerprint)?;
    assert_eq!(rows(&Connection::open(away.path().join("node.sqlite3"))?, id.as_str())?, 0);

    super::begin(&directory)?;
    super::purge(&database, id.as_str())?;
    assert_eq!(database.read_now(|db| rows(db, id.as_str()))?, 0);
    disk::copy(left.join(bundle::FILE), directory.path().join(bundle::FILE))?;
    let preparing = home.clone();
    database.write_now(128, move |db| transfer::prepare(db, &preparing))?;
    wait(&home, &database, id.as_str())?;
    assert!(transfer::rejection(home.path(), id.as_str()).is_some(), "an offer of a deleted conversation is refused");
    assert_eq!(database.read_now(|db| rows(db, id.as_str()))?, 0);
    drop(service);
    println!(
        "conversation moves: taken and let go by fingerprint both ways, refused while running, for another export or changed rows; an interrupted release finished; none left behind; a deleted one not brought back"
    );
    Ok(())
}

/// The fingerprint of the export the world under `root` took of `conversation`.
fn imported(root: &Path, conversation: &str) -> std::io::Result<Option<String>> {
    Ok(transfer::status::list(root)?
        .into_iter()
        .find(|import| import.conversation == conversation && import.state == "imported")
        .and_then(|import| import.fingerprint))
}

/// The rows `conversation` has in every table that keeps conversations' rows.
fn rows(db: &Connection, conversation: &str) -> fastctx_storage::Result<i64> {
    let mut count = 0;
    for table in schema::tables(db)? {
        count += db.query_row(
            &format!("SELECT count(*) FROM {} WHERE conversation=?1", schema::quote(&table.name)),
            [conversation],
            |row| row.get::<_, i64>(0),
        )?;
    }
    Ok(count)
}
