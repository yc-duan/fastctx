//! Delayed gathering after a completed SQL import, and stable queue/job identity.
use super::*;
use crate::records as sql;
use fastctx_storage::Priority;
use fastctx_types::{
    run::JobRecord,
    session::{Happened, SessionChange, SessionEvent},
};

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let name = "codex-a1b2c3@gpu-2";
    let room = ConversationDir::at(&world.conversations().join(name));
    fastctx_platform::disk::create_dir_all(room.path())?;
    let database = crate::record_database::open(&world);
    let service = crate::record_database::register(&world, &database)?;
    service.start()?;
    let map = room.path().join("drill-map.json");
    sql::legacy::map::<u64>(&room, "gather-map", &map)?;
    let action = fastctx_types::residents::Action {
        turn: None,
        at: "2026-01-01T00:00:00Z".parse()?,
        machine: "gpu-2".parse()?,
        hand: "run".into(),
        what: "same observation".into(),
        result: "exit 0".into(),
    };
    let line = serde_json::to_string(&action)? + "\n";
    fastctx_platform::disk::write(room.actions(), line.repeat(2))?;
    crate::residents::activity::prepare(&room, "actions")?;
    assert_eq!(sql::read(&room, |rows| rows.all::<fastctx_types::residents::Action>("actions"))?.len(), 2);

    crate::session_record::prepare(&room)?;
    crate::run_facts::storage::database(&room)?;
    sql::write_at(&room, Priority::Normal, 64, |rows| rows.put("gather-map", "present", &42u64))?;
    let counted = room.path().join("counted.json");
    fastctx_platform::disk::write(&counted, serde_json::to_vec(&vec![true; 64])?)?;
    let interrupted =
        sql::ensure_file(&room, "counted-actions", counted.clone(), |path, push| counted_rows(path, push, 32));
    assert!(interrupted.is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock));
    assert_eq!(sql::read(&room, |rows| rows.all::<bool>("counted-actions"))?.len(), 32);
    drop(service);
    drop(database);
    let old = world.earlier().directory(None, "codex-a1b2c3@GPU-2");
    fastctx_platform::disk::create_dir_all(old.join("session-events"))?;
    fastctx_platform::disk::write(old.join("drill-map.json"), br#"{"new":7}"#)?;
    let mut newer = action.clone();
    newer.what = "new observation".into();
    fastctx_platform::disk::write(
        old.join("actions"),
        format!("{}{}{}\n", line, line, serde_json::to_string(&newer)?),
    )?;
    let event = SessionEvent {
        at: "2026-01-01T00:00:00Z".parse()?,
        happened: Happened::Changed {
            turn: None,
            change: SessionChange::Metadata { metadata: serde_json::json!({"gathered":true}) },
        },
    };
    fastctx_platform::disk::write(
        old.join("session-events/new.json"),
        serde_json::to_vec(&{
            let mut value = serde_json::to_value(&event)?;
            value["sequence"] = serde_json::json!(400);
            value
        })?,
    )?;
    let gathered = gather(&world)?;
    assert!(gathered.problems.is_empty(), "{:?}", gathered.problems);
    assert!(!fastctx_platform::disk::is_there(old.join("drill-map.json")));
    let artifact = room.path().join("out/conversations/user-folder");
    fastctx_platform::disk::create_dir_all(&artifact)?;
    records::moving(&artifact.join("piece.txt"))?;
    assert!(
        !fastctx_platform::disk::is_there(artifact.join(".record-generation")),
        "an artifact name was mistaken for a conversation root"
    );
    let database = crate::storage_drill::reopen(&world)?;
    let service = crate::record_database::register(&world, &database)?;
    assert!(
        sql::legacy::map::<u64>(&room, "gather-map", &map)
            .is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock)
    );
    service.start()?;
    sql::legacy::map::<u64>(&room, "gather-map", &map)?;
    assert_eq!(sql::read(&room, |rows| rows.get::<u64>("gather-map", "new"))?, Some(7));
    assert_eq!(sql::read(&room, |rows| rows.get::<u64>("gather-map", "present"))?, Some(42));
    assert_eq!(crate::session_record::history(&room)?.len(), 1);
    assert_eq!(crate::session_record::latest(&room)?, 400);
    sql::ensure_file(&room, "counted-actions", counted, |path, push| counted_rows(path, push, usize::MAX))?;
    assert_eq!(
        sql::read(&room, |rows| rows.all::<bool>("counted-actions"))?.len(),
        64,
        "gather reset changed a partial source's occurrence sequence"
    );
    crate::residents::activity::prepare(&room, "actions")?;
    assert_eq!(
        sql::read(&room, |rows| rows.all::<fastctx_types::residents::Action>("actions"))?.len(),
        3,
        "gather lost an occurrence or duplicated an older copy"
    );
    let canonical = room.path().join("legacy/0/jobs/j-a1b2c3.json");
    let earlier = old.join("jobs/j-a1b2c3.json");
    assert_eq!(crate::record_database::item(&earlier)?.1, crate::record_database::item(&canonical)?.1);
    assert_eq!(crate::record_database::item(&earlier)?.0.path(), room.path());
    let owner = Address { machine: "gpu-2".parse()?, resident: "codex-a1b2c3".parse()? };
    let record: JobRecord = serde_json::from_value(
        serde_json::json!({"id":"j-a1b2c3","machine":"gpu-2","owner":owner,"town":null,"script":"true","cwd":"/drill","output":"/drill/out","started_at":"2026-01-01T00:00:00Z","supervisor":null,"pid":null,"exit":null,"output_ended_at":null,"remaining":null,"output_lost":null,"end":null,"ends_with_node":false}),
    )?;
    let first = crate::job_metadata::Job::of(&earlier, &record)?;
    let second = crate::job_metadata::Job::of(&canonical, &record)?;
    assert_eq!(first.family("notice"), second.family("notice"));
    assert_eq!(first.next_sequence(0)?, 1);
    assert_eq!(second.next_sequence(0)?, 2);
    drop(service);
    drop(database);
    // A pending gather plan supplies the alias before a live job can move.
    fastctx_platform::disk::write(
        room.path().join(PLAN),
        json(&Plan {
            sources: vec![Source { directory: old.clone(), records: old.clone() }],
            places: None,
            removed: None,
            keeps_launch: false,
        })?,
    )?;
    fastctx_platform::disk::remove_file(room.aliases())?;
    let database = crate::storage_drill::reopen(&world)?;
    let service = crate::record_database::register(&world, &database)?;
    service.start()?;
    assert_eq!(crate::job_metadata::Job::of(&earlier, &record)?.family("notice"), second.family("notice"));
    assert_eq!(crate::job_metadata::Job::of(&earlier, &record)?.next_sequence(0)?, 3);
    drop(service);
    drop(database);
    fastctx_platform::disk::remove_file(room.path().join(PLAN))?;
    // Envelope origins preserve exact legacy request IDs through all sidecars.
    let home = world.conversation(&"codex-abc123".parse()?);
    let root_queue = world.path().join("received-controls");
    fastctx_platform::disk::create_dir_all(&root_queue)?;
    let machine = crate::machine::names(&world)?.name;
    let envelope = serde_json::json!({"scope":{"hub":"w-0000000000000001","account":"alice"},"order":4,"from":"sender","request":"request-abc123","command":{"kind":"stop_run","owner":{"machine":machine,"resident":"codex-abc123"},"dispatch":"dispatch-abc123"}});
    fastctx_platform::disk::write(root_queue.join("message.part.json"), serde_json::to_vec(&envelope)?)?;
    fastctx_platform::disk::write(root_queue.join("message.part.retry"), b"retry canary")?;
    fastctx_platform::disk::write(root_queue.join("sql-conflict.json"), serde_json::to_vec(&envelope)?)?;
    fastctx_platform::disk::write(root_queue.join("sql-conflict.reply"), b"must not attach to another SQL envelope")?;
    let mut gathered = Gathered::default();
    queues::gather(&world, &BTreeSet::new(), &mut gathered)?;
    assert!(gathered.problems.is_empty(), "{:?}", gathered.problems);
    fastctx_platform::disk::create_dir_all(&root_queue)?;
    let conflict = root_queue.join("conflict.json");
    fastctx_platform::disk::write(&conflict, serde_json::to_vec(&envelope)?)?;
    fastctx_platform::disk::write(conflict.with_extension("retry"), b"must stay with old envelope")?;
    fastctx_platform::disk::write(
        home.book("received-controls").join("conflict.json"),
        b"different existing envelope",
    )?;
    let mut conflict_result = Gathered::default();
    queues::gather(&world, &BTreeSet::new(), &mut conflict_result)?;
    assert_eq!(conflict_result.problems.len(), 1);
    assert!(
        !fastctx_platform::disk::is_there(&conflict)
            && !fastctx_platform::disk::is_there(conflict.with_extension("retry"))
    );
    assert!(fastctx_platform::disk::is_dir(home.path().join("recovered/queues/received-controls")));
    assert!(!fastctx_platform::disk::is_there(home.book("received-controls").join("conflict.retry")));
    let database = crate::storage_drill::reopen(&world)?;
    let service = crate::record_database::register(&world, &database)?;
    service.start()?;
    assert_eq!(crate::record_database::item(&conflict)?.1, "items/received-controls");
    let mut present = envelope.clone();
    present["order"] = serde_json::json!(100);
    let present_body = serde_json::to_vec(&present)?;
    let kept = present_body.clone();
    sql::write_at(&home, Priority::Normal, kept.len(), move |rows| {
        rows.put_raw("items/books/received-controls", "sql-conflict.json", &kept)
    })?;
    queues::prepare_conflicts(&home)?;
    let migrated = home.book("received-controls").join("message.part.retry");
    let original_body = serde_json::to_vec(&envelope)?;
    let old_path = root_queue.join("message.part.json");
    let old_id = crate::control_letters::id(&old_path, &original_body);
    assert_eq!(sql::items::read(&old_path)?, original_body);
    let page = sql::items::entries(&home.book("received-controls"), Some("json"))?;
    assert!(page.contains(&old_path));
    assert_eq!(crate::control_letters::id(&old_path, &original_body), old_id);
    let conflict_target = home.book("received-controls").join("sql-conflict.json");
    assert!(page.contains(&conflict_target));
    assert_eq!(sql::items::read(&conflict_target)?, present_body);
    assert!(!sql::items::exists(&conflict_target.with_extension("reply"))?);
    assert_eq!(crate::record_database::item(&root_queue.join("sql-conflict.json"))?.1, "items/received-controls");
    assert_eq!(crate::record_database::original(&migrated), root_queue.join("message.part.retry"));
    assert_eq!(sql::items::read(&root_queue.join("message.part.retry"))?, b"retry canary");
    assert_eq!(crate::record_database::item(&root_queue.join("message.part.reply"))?.0.path(), home.path());
    assert_eq!(
        sql::items::entries(&home.book("received-controls"), Some("retry"))?,
        vec![root_queue.join("message.part.retry")]
    );
    drop(service);
    drop(database);
    let database = crate::storage_drill::reopen(&world)?;
    let service = crate::record_database::register(&world, &database)?;
    service.start()?;
    assert!(crate::record_database::rejected_origin(&conflict_target));
    assert_eq!(crate::record_database::original(&conflict_target), conflict_target);
    println!(
        "gather: completed imports reopened; late rows and old sequence retained; uppercase guest and live/finished aliases share job families and sequence; queue origin and sibling routing preserved"
    );
    Ok(())
}

fn counted_rows(path: &Path, push: &mut dyn FnMut(sql::Source) -> io::Result<()>, stop: usize) -> io::Result<()> {
    let values: Vec<bool> = serde_json::from_slice(&fastctx_platform::disk::read(path)?)?;
    for (index, value) in values.into_iter().enumerate() {
        push(sql::Source::new("same-action".into(), path.to_path_buf(), serde_json::to_vec(&value)?)?.counted())?;
        if index + 1 == stop {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "interrupted after a committed source batch"));
        }
    }
    Ok(())
}
