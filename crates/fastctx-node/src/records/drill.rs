//! Real record-family transitions, independent of a model or network account.

use super::*;
use fastctx_types::{
    Address, ResidentId,
    layout::WorldDir,
    residents::{Delivery, MessageKind, MessageLine, Notice, Pending, PendingState, Seen, WorldFact},
    session::{TurnEnd, TurnHow},
};
use std::{
    path::Path,
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

fn pending(index: usize) -> Pending {
    let id = format!("input-{index:06}").parse().expect("drill input id");
    Pending {
        delivery: Delivery {
            id,
            at: "2026-01-01T00:00:00Z".parse().expect("drill time"),
            notice: Notice::World { fact: WorldFact::DeskOutside, text: format!("input {index}") },
        },
        readers: Vec::new(),
        state: PendingState::Waiting,
        working: false,
        session: None,
        since: None,
    }
}
fn end() -> TurnEnd {
    TurnEnd {
        how: TurnHow::Completed,
        reason: None,
        error: None,
        duration_ms: Some(1),
        facts: None,
        last_message: Some("done".into()),
    }
}

pub(crate) fn run(root: &Path) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let database = crate::record_database::open(&world);
    if let Some(error) = database.fault() {
        return Err(error.clone().into());
    }
    let _migration = crate::record_database::register(&world, &database)?;
    _migration.start()?;
    let id: ResidentId = "codex-a1b2c3".parse()?;
    let directory = world.conversation(&id);
    fastctx_platform::disk::create_dir_all(directory.path())?;
    let mut first = pending(0);
    let mut later = pending(1);
    first.delivery.at = "2026-01-01T00:00:00Z".parse()?;
    later.delivery.at = "2026-01-01T00:00:00.000000001Z".parse()?;
    assert!(crate::inbox::keep(&directory, &later)?);
    assert!(crate::inbox::keep(&directory, &first)?);
    let rows = crate::inbox::pending(&directory)?;
    assert_eq!(rows[0].delivery.id, first.delivery.id, "nanosecond arrival order changed");
    assert!(!crate::inbox::keep(&directory, &first)?);
    crate::inbox::retain_uncertain(&directory, &first)?;
    assert_eq!(crate::inbox::pending(&directory)?.len(), 1);
    crate::inbox::reclaim(&directory, &[first.delivery.id.to_string()])?;
    assert_eq!(crate::inbox::pending(&directory)?.len(), 2);
    crate::inbox::consume(&directory, &first.delivery.id)?;
    assert!(!crate::inbox::keep(&directory, &first)?);
    assert!(crate::inbox::heard_at(&directory, &first.delivery.id)?.is_none());

    let mut original = crate::delivery::begin(&directory, vec![later.clone()])?;
    let mut stale = original.clone();
    let before = crate::delivery::handovers::modified(&directory, &original.id)?;
    crate::delivery::accept(&directory, &mut original, "turn-a")?;
    crate::delivery::refused(&directory, &mut stale)?;
    assert_eq!(stale.turn.as_deref(), Some("turn-a"), "stale refusal erased accepted turn");
    assert!(crate::delivery::accept(&directory, &mut stale, "turn-b").is_err());
    assert_eq!(crate::delivery::input_turn(&directory, &later.delivery.id)?.as_deref(), Some("turn-a"));
    let mut same = crate::delivery::begin(&directory, vec![pending(2)])?;
    let modified = crate::delivery::handovers::modified(&directory, &same.id)?;
    crate::delivery::accept(&directory, &mut same, "turn-c")?;
    assert_eq!(modified, crate::delivery::handovers::modified(&directory, &same.id)?);
    assert!(before.is_some());

    let barrier = Arc::new(Barrier::new(4));
    let mut workers = Vec::new();
    for worker in 0..4 {
        let (directory, barrier) = (directory.clone(), barrier.clone());
        workers.push(std::thread::spawn(move || -> io::Result<()> {
            barrier.wait();
            for index in 0..32 {
                let turn = format!("parallel-{index}");
                let reader = Address {
                    machine: format!("reader-{worker}").parse().expect("reader"),
                    resident: "codex-d4e5f6".parse().expect("resident"),
                };
                crate::delivery::finish_at(&directory, &turn, end(), &[reader], None, true)?;
            }
            Ok(())
        }));
    }
    for worker in workers {
        worker.join().map_err(|_| "record worker panicked")??;
    }
    let terminals = crate::delivery::terminals(&directory)?;
    assert_eq!(terminals.len(), 32);
    let mut orders = std::collections::HashSet::new();
    for terminal in terminals {
        assert_eq!(terminal.readers.len(), 4);
        assert!(orders.insert(terminal.queue_order));
    }
    let message = MessageLine {
        at: first.delivery.at,
        what: MessageKind::Ended { turn: "once".into(), end: end(), owed: Vec::new() },
    };
    crate::residents::append_message(&directory, &message)?;
    let mut repeat = message.clone();
    repeat.at = repeat.at.checked_add(Duration::from_secs(1))?;
    crate::residents::append_message(&directory, &repeat)?;
    assert_eq!(crate::residents::activity::lines::<MessageLine>(&directory.messages())?.len(), 1);

    let legacy = world.conversation(&"codex-abc123".parse()?);
    fastctx_platform::disk::create_dir_all(legacy.books())?;
    fastctx_platform::disk::write(legacy.delivered(), b"input-000007\n")?;
    assert!(crate::inbox::delivered_here(&legacy, &pending(7).delivery.id)?);
    assert!(
        crate::inbox::heard_at(&legacy, &pending(7).delivery.id)?.is_none(),
        "old duplicate marker became consumption proof"
    );
    let file = root.join("seen-file.txt");
    fastctx_platform::disk::write(&file, b"one")?;
    let mut seen = Seen {
        path: file.to_string_lossy().into_owned(),
        at: first.delivery.at,
        sha256: Some("old".into()),
        size: 3,
        modified: Some(first.delivery.at),
        encoding: Some("utf-8".into()),
        encoding_selection: None,
    };
    let mut bytes = serde_json::to_vec(&seen)?;
    bytes.push(b'\n');
    seen.sha256 = Some("new".into());
    bytes.extend(serde_json::to_vec(&seen)?);
    bytes.push(b'\n');
    fastctx_platform::disk::write(legacy.seen(), bytes)?;
    assert_eq!(
        crate::residents::activity::get_seen(&legacy, &file)?.and_then(|row| row.sha256).as_deref(),
        Some("new")
    );
    queues_and_jobs(&world, &directory)?;
    let start = Instant::now();
    for _ in 0..100 {
        assert!(crate::residents::activity::get_seen(&legacy, &file)?.is_some());
    }
    println!(
        "record families: 4 concurrent writers, 32 unique terminal orders, all readers retained; input order, handover claims, duplicate messages, seen migration and receipt provenance passed; 100 indexed seen reads {:?}",
        start.elapsed()
    );
    Ok(())
}

fn queues_and_jobs(
    world: &WorldDir,
    directory: &ConversationDir,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let folder = directory.book("received");
    fastctx_platform::disk::create_dir_all(&folder)?;
    fastctx_platform::disk::write(
        folder.join("first.json"),
        serde_json::to_vec(
            &serde_json::json!({"order":5,"to":"codex-a1b2c3","delivery":pending(5).delivery,"owed":crate::speech::Owed::Nobody}),
        )?,
    )?;
    fastctx_platform::disk::write(folder.join("sequence"), b"6")?;
    assert!(matches!(items::enqueue(&folder.join("second.json"), b"{}".to_vec(), None)?, items::Enqueued::Inserted));
    let queued = items::queued(&folder, &[], "", &items::queue_end(&folder)?)?;
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0].path.file_name().unwrap(), "first.json");
    assert!(queued[0].order < queued[1].order);
    items::write(&folder.join("second.taken"), b"")?;
    items::write(&folder.join("diagnostic.retry"), b"plain bytes, not JSON")?;
    assert_eq!(items::read(&folder.join("diagnostic.retry"))?, b"plain bytes, not JSON");
    let (scope, family, key, _) = crate::record_database::item(&folder.join("second.taken"))?;
    let expires: Option<i64> = read(&scope, |rows| {
        Ok(rows.connection().query_row(
            "SELECT expires FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
            (rows.conversation(), family, key),
            |row| row.get(0),
        )?)
    })?;
    assert!(expires.is_some());
    let path = directory.path().join("jobs/j-abc123.json");
    fastctx_platform::disk::create_dir_all(path.parent().unwrap())?;
    let record: fastctx_types::run::JobRecord = serde_json::from_value(serde_json::json!({
        "id":"j-abc123","machine":"worker","owner":{"machine":"worker","resident":"codex-a1b2c3"},
        "script":"true","cwd":directory.path(),"output":directory.path().join("out/j-abc123.log"),
        "started_at":"2026-01-01T00:00:00.000000001Z","ends_with_node":false
    }))?;
    fastctx_platform::disk::write(&path, serde_json::to_vec(&record)?)?;
    let fact = |index| fastctx_types::run_facts::WriteFact {
        turn: None,
        id: format!("{index:08x}"),
        machine: "worker".parse().unwrap(),
        path: format!("file-{index}"),
        kind: fastctx_types::write::ChangeKind::Added,
        added: Some(1),
        removed: Some(0),
        unavailable: None,
        diff: None,
        note: None,
    };
    fastctx_platform::disk::create_dir_all(path.with_extension("writes"))?;
    fastctx_platform::disk::write(path.with_extension("writes").join("00000000.json"), serde_json::to_vec(&fact(0))?)?;
    for index in 1..260 {
        crate::run_facts::write(&path.with_extension("writes"), &fact(index))?;
    }
    crate::run_facts::write(&path.with_extension("writes"), &fact(1))?;
    let mut cursor = 0;
    let first = crate::run_facts::read_since(&path, &mut cursor)?;
    assert_eq!(first.len(), 128);
    let mut count = first.len();
    loop {
        let page = crate::run_facts::read_since(&path, &mut cursor)?;
        if page.is_empty() {
            break;
        }
        count += page.len();
    }
    assert_eq!(count, 260);
    let job = crate::job_metadata::Job::open(&path)?;
    job.put_raw("owed", b"answer")?;
    let sequence = job.next_sequence(100)?;
    assert_eq!(sequence, 101);
    let mut next = record.clone();
    next.started_at = next.started_at.checked_add(Duration::from_nanos(1))?;
    let next = crate::job_metadata::Job::of(&path, &next)?;
    assert!(next.raw("owed")?.is_none(), "short job ID reused prior incarnation metadata");
    assert_eq!(next.next_sequence(0)?, 102);
    let timers = Arc::new(crate::timers::store::Store::open(&world.path().join("timers"))?);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
    runtime.block_on(async {
        use fastctx_types::timers::{Action, Owner};
        let now = jiff::Timestamp::now();
        let owner = Owner::Conversation { resident: "codex-a1b2c3".parse().unwrap() };
        let action = Action::Tell { resident: "codex-a1b2c3".parse().unwrap(), text: "wake".into() };
        let timer =
            timers.set("timer-one".into(), owner.clone(), now, action.clone(), "delivery-one".parse().unwrap()).await?;
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let timers = timers.clone();
            tasks.spawn(async move { timers.claim(now).await });
        }
        let mut claimed = 0;
        while let Some(result) = tasks.join_next().await {
            claimed += usize::from(result.map_err(io::Error::other)??.is_some());
        }
        assert_eq!(claimed, 1);
        let begun = timers.begin(&timer, now).await?.unwrap();
        let repeated = timers.begin(&timer, now.checked_add(Duration::from_secs(2)).unwrap()).await?.unwrap();
        assert_eq!(begun.fired_at, repeated.fired_at);
        assert_eq!(repeated.attempts, 2);
        let replacement = timers.set("timer-one".into(), owner, now, action, "delivery-two".parse().unwrap()).await?;
        assert!(!timers.complete(&timer).await?);
        timers.unclaim(&timer);
        assert_eq!(timers.claim(now).await?.unwrap().delivery, replacement.delivery);
        timers.release(&replacement, now);
        assert!(timers.claim(now).await?.is_none());
        let reopened = Arc::new(crate::timers::store::Store::open(&world.path().join("timers"))?);
        assert_eq!(reopened.claim(now).await?.unwrap().delivery, replacement.delivery);
        assert!(reopened.complete(&replacement).await?);
        Ok::<_, io::Error>(())
    })?;
    println!(
        "SQL queue ordering/raw receipts/expiry, 260-write source pagination, nanosecond job incarnation isolation and concurrent timer generations passed"
    );
    Ok(())
}
