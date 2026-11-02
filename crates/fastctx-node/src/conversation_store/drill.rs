//! Whole-conversation deletion under concurrent late writes and file repair.
use super::*;
use fastctx_storage::{Database, Priority};
use fastctx_types::layout::WorldDir;
use std::path::Path;

pub(crate) fn run(root: &Path) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let schema = format!(
        "{}{}{}{}{}{}{}{}{}",
        crate::records::SCHEMA,
        crate::session_record::SCHEMA,
        crate::record_payloads::SCHEMA,
        crate::records::maps::SCHEMA,
        crate::run_facts::SCHEMA,
        crate::delivery::terminals::SCHEMA,
        crate::composer::SCHEMA,
        crate::inbox::presence::SCHEMA,
        crate::goals::SCHEMA
    );
    let database =
        Database::open_guarded(&world.path().join("node.sqlite3"), &schema, fences::guard, schema::initialize)?;
    let _migration = crate::record_database::register(&world, &database)?;
    _migration.start()?;
    let directory = world.conversation(&"codex-aabbcc".parse()?);
    fastctx_platform::disk::create_dir_all(directory.path())?;
    for start in (0..100_000).step_by(128) {
        database.write_now_at(Priority::Background, 8192, move |db| {
            for number in start..(start + 128).min(100_000) {
                db.execute(
                    "INSERT INTO session_events(conversation,sequence,identity,body) VALUES('codex-aabbcc',?1,?2,?3)",
                    (
                        u64::try_from(number + 1).unwrap().to_be_bytes().as_slice(),
                        format!("event-{number}"),
                        b"{}".as_slice(),
                    ),
                )?;
            }
            Ok(())
        })?;
    }
    crate::records::write_at(&directory, Priority::Normal, 128, |rows| {
        rows.put("pending-inputs", "input-a", &true)?;
        rows.put("pending-inputs", "input-b", &true)?;
        rows.put("delivery-receipts", "input-a", &true)?;
        rows.put("goal-local", "current", &"goal")?;
        rows.put("handovers", "handover-a", &"handover")?;
        rows.connection().execute("INSERT INTO context_projection_dirty VALUES(?1)", [rows.conversation()])?;
        rows.connection()
            .execute("INSERT INTO composers(owner,revision,content) VALUES(?1,1,X'7B7D')", [rows.conversation()])?;
        Ok(())
    })?;
    begin(&directory)?;
    assert!(
        crate::records::write_at(&directory, Priority::Normal, 1, |rows| rows.put("pending-inputs", "late", &true))
            .is_err()
    );
    assert!(with_live_files(&directory, || fastctx_platform::disk::create_dir_all(directory.path())).is_err());
    let started = std::time::Instant::now();
    let bytes = purge(&database, "codex-aabbcc")?;
    assert!(bytes > 1_000_000);
    fastctx_platform::disk::remove_dir_all(directory.path())?;
    database.read_now(|db| {
        let tables = schema::tables(db)?;
        for table in &tables {
            let count: i64 = db.query_row(
                &format!("SELECT count(*) FROM {} WHERE conversation='codex-aabbcc'", schema::quote(&table.name)),
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0, "{} retained deleted records", table.name);
        }
        println!(
            "conversation deletion: {} tables, 100000 events, {bytes} logical bytes, {:?}",
            tables.len(),
            started.elapsed()
        );
        Ok(())
    })?;
    drop(_migration);
    drop(database);
    let database = crate::storage_drill::reopen(&world)?;
    let _migration = crate::record_database::register(&world, &database)?;
    _migration.start()?;
    resume(&database)?;
    assert!(with_live_files(&directory, || fastctx_platform::disk::create_dir_all(directory.path())).is_err());
    assert!(!fastctx_platform::disk::is_there(directory.path()));
    assert_eq!(purge(&database, "codex-aabbcc")?, bytes);
    Ok(())
}
