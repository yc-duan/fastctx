//! Indexed event paging, migration order, idempotency and local corruption isolation.

use super::*;
use fastctx_types::{
    ResidentId,
    layout::WorldDir,
    session::{Happened, SessionChange},
};
use std::time::{Duration, Instant};

fn event(index: u64) -> SessionEvent {
    SessionEvent {
        at: "2026-01-01T00:00:00Z".parse().expect("drill timestamp"),
        happened: Happened::Changed {
            turn: None,
            change: SessionChange::Metadata { metadata: serde_json::json!({"index":index}) },
        },
    }
}

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let database = Database::open_guarded(
        &world.path().join("node.sqlite3"),
        SCHEMA,
        crate::conversation_store::fences::guard,
        crate::conversation_store::schema::initialize,
    )?;
    let _migration = crate::record_database::register(&world, &database)?;
    _migration.start()?;
    let id: ResidentId = "codex-a1b2c3".parse()?;
    let directory = world.conversation(&id);
    assert!(append(&directory, "first", &event(1))?);
    let scope = id.to_string();
    for start in (2..=100000u64).step_by(128) {
        let scope = scope.clone();
        database.write_now(128 * 256, move |db| {
            let end = (start + 127).min(100000);
            for index in start..=end {
                let body = serde_json::to_vec(&event(index))
                    .map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
                db.execute(
                    "INSERT INTO session_events(conversation,sequence,identity,body) VALUES(?1,?2,?3,?4)",
                    params![scope, index.to_be_bytes().as_slice(), format!("event-{index}"), body],
                )?;
            }
            sql::set_head(db, &scope, end, end, true)
        })?;
    }
    let mut times = Vec::new();
    for _ in 0..200 {
        let start = Instant::now();
        let loaded = page(&directory, 99900, None, 100)?;
        assert_eq!(loaded.events.len(), 100);
        assert_eq!(loaded.next, 100000);
        assert_eq!(loaded.latest, 100000);
        times.push(start.elapsed());
    }
    times.sort();
    assert!(times[197] < Duration::from_millis(300));
    assert!(!append(&directory, "first", &event(1))?);
    assert_eq!(latest(&directory)?, 100000);
    assert!(append(&directory, "last", &event(100001))?);
    assert_eq!(latest(&directory)?, 100001);
    let fenced = page(&directory, 99900, Some(99950), 512)?;
    assert_eq!(fenced.events.len(), 50);
    assert_eq!(fenced.next, 99950);
    println!(
        "100000 session events: actual page API p99 {:?}; range fence and duplicate sequence allocation passed",
        times[197]
    );

    let legacy_id: ResidentId = "codex-d4e5f6".parse()?;
    let legacy = world.conversation(&legacy_id);
    fastctx_platform::disk::create_dir_all(legacy.session_events())?;
    let mut expected = Vec::new();
    for index in 0..257u64 {
        let mut event = event(index);
        event.at = event.at.checked_add(jiff::SignedDuration::from_nanos((257 - index) as i64))?;
        let sequence = (index >= 100).then_some(index * 10);
        let key = format!("legacy-{index}");
        fastctx_platform::disk::write(
            legacy.session_events().join(format!("{key}.json")),
            serde_json::to_vec(&Recorded { sequence, event: event.clone() })?,
        )?;
        expected.push((sequence, event.at, key, event));
    }
    fastctx_platform::disk::write(legacy.session_sequence(), b"5000")?;
    expected.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let actual = history(&legacy)?;
    assert_eq!(
        serde_json::to_vec(&actual)?,
        serde_json::to_vec(&expected.into_iter().map(|row| row.3).collect::<Vec<_>>())?
    );
    let original_sequence = database.read_now(|db| {
        sql::number(db.query_row(
            "SELECT sequence FROM session_events WHERE conversation=?1 AND identity='legacy-100'",
            [legacy_id.as_str()],
            |row| row.get(0),
        )?)
    })?;
    assert_eq!(original_sequence, 1000);
    assert_eq!(fastctx_platform::disk::read_dir(legacy.session_events())?.count(), 0);
    let crowded_id: ResidentId = "codex-aabb11".parse()?;
    let crowded = world.conversation(&crowded_id);
    fastctx_platform::disk::create_dir_all(crowded.session_events())?;
    for (key, sequence, index) in [("first", Some(1), 1), ("second", None, 2), ("third", None, 3)] {
        fastctx_platform::disk::write(
            crowded.session_events().join(format!("{key}.json")),
            serde_json::to_vec(&Recorded { sequence, event: event(index) })?,
        )?;
    }
    fastctx_platform::disk::write(crowded.session_events().join("broken.json"), b"{")?;
    let crowded_page = super::page(&crowded, 0, None, 100)?;
    assert_eq!(crowded_page.events.iter().map(|(sequence, _)| *sequence).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(crate::record_migration::archive::damaged_count(world.path())?, 1);
    assert!(append(&crowded, "after-damage", &event(4))?);
    assert_eq!(crate::record_migration::archive::collect(world.path(), Duration::ZERO)?, 261);
    assert_eq!(crate::record_migration::archive::damaged_count(world.path())?, 1);
    assert!(append(&legacy, "after-migration", &event(999))?);
    assert_eq!(latest(&legacy)?, 5001);
    let broken = legacy_id.to_string();
    database.write_now(32, move |db| {
        db.execute("UPDATE session_events SET body=x'7b' WHERE conversation=?1", [broken])?;
        Ok(())
    })?;
    assert!(page(&legacy, 0, None, 100).is_err());
    assert_eq!(page(&directory, 100000, None, 100)?.events.len(), 1);
    println!(
        "legacy events: 257 mixed sequence/timestamp rows verified; reserved sequence preserved; corrupt conversation did not affect another"
    );
    Ok(())
}
