//! Source isolation and background admission through the production interfaces.
use fastctx_platform::disk;
use fastctx_types::{ResidentId, layout::WorldDir};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let database = crate::record_database::open(&world);
    if let Some(error) = database.fault() {
        return Err(error.to_string().into());
    }
    let service = crate::record_database::register(&world, &database)?;
    let id: ResidentId = "codex-ab1234".parse()?;
    let directory = world.conversation(&id);
    disk::create_dir_all(directory.book("goal-operations"))?;
    disk::create_dir_all(directory.path().join("context"))?;
    disk::create_dir_all(world.path().join("timers"))?;
    disk::write(directory.book("goal-operations").join("broken.json"), b"{")?;
    disk::write(directory.path().join("context/world.json"), b"{")?;
    disk::write(world.path().join("timers/broken.json"), b"{")?;
    let before = Instant::now();
    let unopened = crate::goals::intents::prepare(&directory).expect_err("migration ran before opening");
    assert_eq!(unopened.kind(), io::ErrorKind::WouldBlock);
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(disk::exists(directory.book("goal-operations").join("broken.json"))?);
    service.start()?;
    crate::goals::intents::prepare(&directory)?;
    crate::context::records::prepare(&world, &id)?;
    crate::timers::store::Store::open(&world.path().join("timers"))?;
    assert_eq!(super::archive::damaged_count(world.path())?, 3);
    let map = directory.path().join("legacy-map.json");
    let values =
        (0..1000).map(|index| (format!("key-{index:05}"), index)).collect::<std::collections::BTreeMap<_, _>>();
    disk::write(&map, serde_json::to_vec(&values)?)?;
    crate::records::legacy::map::<u64>(&directory, "drill-map", &map)?;
    assert!(!disk::exists(&map)?);
    let count = database.read_now(|db| {
        Ok(db.query_row(
            "SELECT count(*) FROM conversation_records WHERE conversation=?1 AND family='drill-map'",
            [id.as_str()],
            |row| row.get::<_, i64>(0),
        )?)
    })?;
    assert_eq!(count, 1000);
    crate::records::legacy::map::<u64>(&directory, "drill-map", &map)?;
    crate::records::write_at(&directory, fastctx_storage::Priority::Interactive, 64, |rows| {
        rows.put("drill-map", "after-migration", &1001)
    })?;
    let conflicting = directory.path().join("conflicting-map.json");
    crate::records::write_at(&directory, fastctx_storage::Priority::Interactive, 64, |rows| {
        rows.put("drill-conflict", "kept", &2)
    })?;
    disk::write(&conflicting, br#"{"kept":1}"#)?;
    crate::records::legacy::map::<u64>(&directory, "drill-conflict", &conflicting)?;
    assert_eq!(crate::records::read(&directory, |rows| rows.get::<u64>("drill-conflict", "kept"))?, Some(2));
    assert_eq!(super::archive::damaged_count(world.path())?, 4);
    let received = directory.book("received");
    disk::create_dir_all(&received)?;
    disk::write(received.join("missing-fields.json"), b"{}")?;
    disk::write(received.join("presence.taken"), [0xff, 0xfe])?;
    crate::records::items::prepare(&received.join(".items"))?;
    assert_eq!(super::archive::damaged_count(world.path())?, 5);
    assert!(crate::records::items::exists(&received.join("presence.taken"))?);
    let (entered, started) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    assert!(
        super::scheduler::discovery(|| super::scheduler::request(
            world.path(),
            id.as_str(),
            "background-drill",
            move || {
                entered.send(()).map_err(io::Error::other)?;
                held.recv().map_err(io::Error::other)
            }
        ))
        .is_err()
    );
    started.recv_timeout(Duration::from_secs(1))?;
    let (dependency_world, dependency_id) = (world.path().to_path_buf(), id.to_string());
    let dependency = Instant::now();
    let dependency_result = super::scheduler::request(world.path(), id.as_str(), "dependent-drill", move || {
        super::scheduler::request(&dependency_world, &dependency_id, "background-drill", || {
            panic!("running prerequisite replaced")
        })
    });
    assert!(dependency_result.is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock));
    assert!(dependency.elapsed() < Duration::from_secs(1));
    assert!(
        super::scheduler::discovery(|| super::scheduler::request(world.path(), id.as_str(), "promoted-drill", || Ok(
            ()
        )))
        .is_err()
    );
    let promoted = Instant::now();
    let promoted_result =
        super::scheduler::request(world.path(), id.as_str(), "promoted-drill", || panic!("queued decoder replaced"));
    release.send(())?;
    promoted_result?;
    assert!(promoted.elapsed() < Duration::from_secs(1));
    let before = Instant::now();
    let error = super::scheduler::request(world.path(), id.as_str(), "slow-drill", || {
        std::thread::sleep(Duration::from_secs(11));
        Ok(())
    })
    .expect_err("long migration exceeded its request deadline");
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(before.elapsed() >= Duration::from_secs(10) && before.elapsed() < Duration::from_secs(11));
    let busy = before.elapsed();
    super::scheduler::request(world.path(), id.as_str(), "slow-drill", || panic!("duplicate decoder ran"))?;
    println!(
        "migration: pre-open admission refused; 5 damaged sources isolated including a row conflict and missing fields; raw presence marker preserved; 1000 map rows verified and archived; demand deadline {busy:?}; job continued after caller left"
    );
    Ok(())
}
