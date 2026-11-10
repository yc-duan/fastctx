//! Handover lifecycle and indexed-read health at a long conversation's scale.
use super::{Handover, handovers};
use crate::records;
use fastctx_storage::Priority;
use fastctx_types::{
    layout::WorldDir,
    residents::{Delivery, Notice, Pending, PendingState, WorldFact},
    session::{TurnEnd, TurnHow},
};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

fn input(index: usize) -> Pending {
    Pending {
        delivery: Delivery {
            id: format!("input-{index}").parse().unwrap(),
            at: jiff::Timestamp::now(),
            notice: Notice::World { fact: WorldFact::DeskOutside, text: format!("input {index}") },
        },
        readers: Vec::new(),
        state: PendingState::Waiting,
        working: false,
        session: None,
        since: None,
    }
}
pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let database = crate::record_database::open(&world);
    if let Some(error) = database.fault() {
        return Err(error.clone().into());
    }
    let migration = crate::record_database::register(&world, &database)?;
    migration.start()?;
    let directory = world.conversation(&"codex-a1b2c3".parse()?);
    let pending = input(0);
    crate::inbox::keep(&directory, &pending)?;
    let mut handover = super::begin(&directory, vec![pending.clone()])?;
    crate::inbox::set_state(&directory, &pending.delivery.id, PendingState::Sending)?;
    super::refused(&directory, &mut handover)?;
    assert_eq!(crate::inbox::read(&directory, &pending.delivery.id)?.unwrap().state, PendingState::Waiting);
    assert!(!handovers::has_active(&directory)?);

    let mut replacement = pending.clone();
    replacement.session = Some("new-session".into());
    replacement.since = Some(jiff::Timestamp::now());
    replacement.working = true;
    let mut retried = super::begin(&directory, vec![replacement.clone()])?;
    assert_eq!(retried.inputs, vec![replacement]);
    assert_eq!(retried.id, handover.id);
    let at = handovers::modified(&directory, &retried.id)?;
    super::accept(&directory, &mut retried, "known-turn")?;
    assert_eq!(at, handovers::modified(&directory, &retried.id)?);
    super::refused(&directory, &mut handover)?;
    assert_eq!(handover.turn.as_deref(), Some("known-turn"));
    assert_eq!(handover.inputs, retried.inputs);
    assert!(super::accept(&directory, &mut handover, "different-turn").is_err());
    let keys = handovers::accepted_inputs(&directory, &[pending.clone(), input(1)], true)?;
    assert!(keys.contains(&pending.delivery.id));
    assert!(!keys.contains(&input(1).delivery.id));
    let mut unknown = super::begin(&directory, vec![input(2)])?;
    let end = TurnEnd {
        how: TurnHow::Interrupted,
        reason: Some("submission was interrupted".into()),
        error: None,
        facts: None,
        duration_ms: None,
        last_message: None,
    };
    super::unresolved(&directory, &mut unknown, end.clone())?;
    super::accept(&directory, &mut unknown, "late-turn")?;
    assert_eq!(unknown.unresolved, Some(end));
    assert_eq!(unknown.turn.as_deref(), Some("late-turn"));

    // Abort the second half of a refusal inside SQLite. Neither half may commit.
    let mut interrupted = super::begin(&directory, vec![input(3)])?;
    crate::inbox::keep(&directory, &input(3))?;
    crate::inbox::set_state(&directory, &input(3).delivery.id, PendingState::Sending)?;
    crate::storage_drill::outside(&world.path().join("node.sqlite3"), "CREATE TRIGGER refusal_fault BEFORE UPDATE OF attempting ON handover_heads BEGIN SELECT RAISE(ABORT,'injected refusal failure'); END;")?;
    assert!(super::refused(&directory, &mut interrupted).is_err());
    assert_eq!(crate::inbox::read(&directory, &input(3).delivery.id)?.unwrap().state, PendingState::Sending);
    assert!(super::handover(&directory, &interrupted.id)?.unwrap().attempting);
    crate::storage_drill::outside(&world.path().join("node.sqlite3"), "DROP TRIGGER refusal_fault")?;
    super::refused(&directory, &mut interrupted)?;
    assert_eq!(crate::inbox::read(&directory, &input(3).delivery.id)?.unwrap().state, PendingState::Waiting);

    // A late acceptance must stay recoverable until its own binding finishes,
    // even when the turn's original effects have already finished.
    let mut late = super::begin(&directory, vec![input(42)])?;
    super::finish_at(&directory, "already-ended", unknown.unresolved.clone().unwrap(), &[], None, false)?;
    records::write_at(&directory, Priority::Normal, 256, |rows| {
        super::terminals::effects_complete(rows, "already-ended")
    })?;
    super::accept(&directory, &mut late, "already-ended")?;
    records::write_at(&directory, Priority::Normal, 256, |rows| handovers::complete_turn(rows, "already-ended"))?;
    assert!(handovers::active(&directory)?.iter().any(|row| row.id == late.id));
    handovers::bound(&directory, &late.id)?;
    assert!(!handovers::active(&directory)?.iter().any(|row| row.id == late.id));

    let legacy_directory = world.conversation(&"codex-b2c3d4".parse()?);
    fastctx_platform::disk::create_dir_all(legacy_directory.handovers())?;
    let old = Handover {
        id: "legacy-handover".parse()?,
        inputs: vec![input(4)],
        context: None,
        context_input: None,
        turn: None,
        unresolved: None,
        attempting: true,
        revision: String::new(),
    };
    let path = legacy_directory.handovers().join("legacy-handover.json");
    fastctx_platform::disk::write(&path, serde_json::to_vec(&old)?)?;
    fastctx_platform::disk::write(path.with_extension("accepted"), b"\"legacy-turn\"")?;
    // One its host never accepted has no receipt beside it.
    let unaccepted = Handover { id: "legacy-unaccepted".parse()?, inputs: vec![input(5)], ..old.clone() };
    fastctx_platform::disk::write(
        legacy_directory.handovers().join("legacy-unaccepted.json"),
        serde_json::to_vec(&unaccepted)?,
    )?;
    let restored = super::handover(&legacy_directory, &old.id)?.unwrap();
    assert_eq!(restored.inputs, old.inputs);
    assert_eq!(restored.turn.as_deref(), Some("legacy-turn"));
    let restored = super::handover(&legacy_directory, &unaccepted.id)?.unwrap();
    assert_eq!(restored.inputs, unaccepted.inputs);
    assert_eq!(restored.turn, None);

    let short = measure(&directory, &pending)?;
    // Seed settled history in bounded setup transactions; timing below measures
    // the production input, turn and recovery queries, not the fixture loader.
    for start in (0..100_000).step_by(64) {
        records::write_at(&directory, Priority::Background, 64 * 1024, move |rows| {
            let mut heads = rows.connection().prepare_cached("INSERT INTO handover_heads(conversation,identity,revision,context,unresolved,turn,attempting,active,modified,modified_millis) SELECT conversation,?2,revision,context,NULL,?3,0,0,modified,modified_millis FROM handover_heads WHERE conversation=?1 AND identity=?4")?;
            let mut inputs = rows.connection().prepare_cached("INSERT INTO handover_inputs SELECT conversation,?2,revision,ordinal,?3,body FROM handover_inputs WHERE conversation=?1 AND handover=?4")?;
            let template: String = rows.connection().query_row(
                "SELECT identity FROM handover_heads WHERE conversation=?1 AND turn='known-turn'",
                [rows.conversation()],
                |row| row.get(0),
            )?;
            for n in start..(start + 64).min(100_000) {
                let id = format!("history-{n:06}");
                heads.execute((rows.conversation(), &id, format!("turn-{n}"), &template))?;
                inputs.execute((rows.conversation(), &id, format!("old-input-{n}"), &template))?;
            }
            Ok(())
        })?;
    }
    let long = measure(&directory, &pending)?;
    println!("handover queries p99: short {short:?}; 100000 settled {long:?}");
    assert!(long < Duration::from_millis(100), "indexed handover reads exceeded the interactive budget");
    assert!(long <= short * 10 + Duration::from_millis(5), "settled history leaked into hot reads");
    println!("handover refusal atomicity, retry snapshots, late acceptance and legacy receipts: passed");
    Ok(())
}
fn measure(directory: &fastctx_types::layout::ConversationDir, pending: &Pending) -> io::Result<Duration> {
    let mut samples = Vec::new();
    for _ in 0..100 {
        let started = Instant::now();
        assert_eq!(super::input_turn(directory, &pending.delivery.id)?.as_deref(), Some("known-turn"));
        assert_eq!(handovers::for_turn(directory, "known-turn")?.len(), 1);
        assert!(handovers::has_active(directory)?);
        assert_eq!(handovers::active(directory)?.len(), 2);
        samples.push(started.elapsed());
    }
    samples.sort();
    Ok(samples[98])
}
