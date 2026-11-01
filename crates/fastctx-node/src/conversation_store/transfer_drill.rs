//! Whole-table proofs, overlapping local cursors, replay and conflict refusal.
use super::{bundle, schema, transfer};
use fastctx_platform::disk;
use fastctx_storage::{
    Priority,
    rusqlite::{Connection, OpenFlags},
};
use fastctx_types::layout::WorldDir;
use std::path::Path;

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let source_world = WorldDir::under(&root.join("source"));
    let source = crate::record_database::open(&source_world);
    assert!(source.fault().is_none(), "{:?}", source.fault());
    let _source_service = crate::record_database::register(&source_world, &source)?;
    let id: fastctx_types::ResidentId = "codex-abcdef".parse()?;
    let source_directory = source_world.conversation(&id);
    disk::create_dir_all(source_directory.path())?;
    for start in (0..100_000).step_by(128) {
        source.write_now_at(Priority::Background, 8192, move |db| {
            for number in start..(start + 128).min(100_000) {
                db.execute(
                    "INSERT INTO session_events(conversation,sequence,identity,body) VALUES('codex-abcdef',?1,?2,?3)",
                    ((number as u64 + 1).to_be_bytes().as_slice(), format!("event-{number}"), b"{}".as_slice()),
                )?;
            }
            Ok(())
        })?;
    }
    crate::records::write_at(&source_directory, Priority::Normal, 1024, |rows| {
        rows.put("pending-inputs", "input-a", &true)?;
        rows.put("pending-inputs", "input-b", &true)?;
        rows.put("delivery-receipts", "input-a", &true)?;
        rows.put("handovers", "handover", &"kept")?;
        rows.put("goal-local", "current", &"goal")?;
        rows.put("paid", "terminal", &true)?;
        rows.put("temporary", "deleted-tail", &true)?;
        rows.delete("temporary", "deleted-tail")?;
        rows.connection()
            .execute("INSERT INTO composers(owner,revision,content) VALUES(?1,1,X'7B7D')", [rows.conversation()])?;
        rows.connection().execute(
            "INSERT INTO composer_records(owner,kind,name,content) VALUES(?1,'receipt','kept',X'7B7D')",
            [rows.conversation()],
        )?;
        rows.connection().execute("INSERT INTO node_record_state VALUES('legacy-discovery-complete',1)", [])?;
        Ok(())
    })?;
    let bundle_path = crate::export_conversation(root.join("source").as_path(), id.as_str())?;
    let manifest = {
        let exported = Connection::open_with_flags(&bundle_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let tables = source.read_now(schema::tables)?;
        let manifest = bundle::manifest(&exported, id.as_str(), &tables)?;
        for proof in &manifest.tables {
            bundle::verify(&exported, id.as_str(), proof)?;
        }
        manifest
    };
    let target_world = WorldDir::under(&root.join("target"));
    let target_directory = target_world.conversation(&id);
    disk::create_dir_all(target_directory.path())?;
    disk::copy(&bundle_path, target_directory.path().join(bundle::FILE))?;
    let target = crate::record_database::open(&target_world);
    assert!(target.fault().is_none(), "{:?}", target.fault());
    let service = crate::record_database::register(&target_world, &target)?;
    let other = target_world.conversation(&"codex-012345".parse()?);
    disk::create_dir_all(other.path())?;
    crate::records::write_at(&other, Priority::Normal, 64, |rows| rows.put("unrelated", "same-position", &true))?;
    assert!(transfer::pending(target_world.path(), id.as_str()));
    assert!(
        target
            .write_now(64, |db| {
                db.execute("INSERT INTO context_projection_dirty VALUES('codex-abcdef')", [])?;
                Ok(())
            })
            .is_err()
    );
    service.start()?;
    let began = std::time::Instant::now();
    wait(&target_world, &target, id.as_str())?;
    assert!(!disk::exists(target_directory.path().join(bundle::FILE))?);
    target.read_now(|db| {
        for proof in &manifest.tables {
            bundle::verify(db, id.as_str(), proof)?;
        }
        Ok(())
    })?;
    assert_eq!(crate::records::read(&other, |rows| rows.get::<bool>("unrelated", "same-position"))?, Some(true));
    drop(service);
    let service = crate::record_database::register(&target_world, &target)?;
    service.start()?;
    // Restoring the identical snapshot again must not replay input triggers.
    disk::copy(&bundle_path, target_directory.path().join(bundle::FILE))?;
    let preparing = target_world.clone();
    target.write_now(128, move |db| transfer::prepare(db, &preparing))?;
    wait(&target_world, &target, id.as_str())?;
    target.read_now(|db| {
        for proof in &manifest.tables {
            bundle::verify(db, id.as_str(), proof)?;
        }
        Ok(())
    })?;
    let high: i64 = target.read_now(|db| {
        Ok(db.query_row(
            "SELECT position FROM conversation_record_positions WHERE conversation=?1 AND stream='records'",
            [id.as_str()],
            |row| row.get(0),
        )?)
    })?;
    crate::records::write_at(&target_directory, Priority::Normal, 64, |rows| rows.put("new", "after-moving", &true))?;
    let position: i64 = target.read_now(|db| {
        Ok(db.query_row(
            "SELECT position FROM conversation_records WHERE conversation=?1 AND family='new'",
            [id.as_str()],
            |row| row.get(0),
        )?)
    })?;
    assert!(position > high);
    drop(service);
    let service = crate::record_database::register(&target_world, &target)?;
    service.start()?;
    disk::copy(&bundle_path, target_directory.path().join(bundle::FILE))?;
    let preparing = target_world.clone();
    target.write_now(128, move |db| transfer::prepare(db, &preparing))?;
    wait(&target_world, &target, id.as_str())?;
    let conflict =
        transfer::rejection(target_world.path(), id.as_str()).expect("different target rows must be rejected");
    assert!(conflict.contains("conflicts") || conflict.contains("row proof differs"), "{conflict}");
    assert!(!disk::exists(target_directory.path().join(bundle::FILE))?);
    assert!(!transfer::pending(target_world.path(), id.as_str()));
    crate::records::write_at(&target_directory, Priority::Normal, 64, |rows| {
        rows.put("new", "after-rejection", &true)
    })?;
    assert_eq!(crate::records::read(&target_directory, |rows| rows.get::<bool>("new", "after-moving"))?, Some(true));
    super::begin(&target_directory)?;
    super::purge(&target, id.as_str())?;
    assert!(!transfer::pending(target_world.path(), id.as_str()));
    println!(
        "conversation transfer: {} table proofs and 100000 events preserved; input revisions unchanged on replay; overlapping node cursors isolated; deleted tail not reused; conflicts quarantined without fencing existing rows; {:?}",
        manifest.tables.len(),
        began.elapsed()
    );
    Ok(())
}

fn wait(world: &WorldDir, database: &fastctx_storage::Database, conversation: &str) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let began = std::time::Instant::now();
        let result = transfer::ensure(world.path(), database, conversation);
        assert!(began.elapsed() < std::time::Duration::from_secs(11), "import admission exceeded its ten-second wait");
        match result {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {}
            result => return result,
        }
    }
}
