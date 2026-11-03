//! A released writer published the job snapshot before appending its journal.
//! The snapshot alone must preserve that crash window's unjournaled members.
use super::{snapshot::StoredCall, sql};
use fastctx_platform::disk;
use fastctx_types::{
    layout::WorldDir,
    run::RunState,
    run_facts::{JobFacts, RunFacts, RunStep, StepDid, WriteFact},
};
use std::path::Path;

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let database = crate::record_database::open(&world);
    let service = crate::record_database::register(&world, &database)?;
    service.start()?;
    let id = "codex-fac123".parse()?;
    let directory = world.conversation(&id);
    let call = fastctx_hands::files::content_hash(b"call");
    let job_path = directory
        .calls()
        .join(format!("{call}.jobs"))
        .join(format!("{}.json", fastctx_hands::files::content_hash(b"executor:j-123456")));
    disk::create_dir_all(job_path.parent().expect("job directory"))?;
    let machine = "executor".parse()?;
    let job = "j-123456".parse()?;
    let at = "2026-01-01T00:00:00Z".parse()?;
    let mut original = StoredCall {
        call_id: "call".into(),
        sequence: 0,
        launched: false,
        script: "echo test".into(),
        unavailable: None,
        facts: RunFacts {
            machine,
            job: Some(job),
            turn: None,
            record: None,
            state: RunState::Running,
            output: None,
            visited: Vec::new(),
            writes: Vec::new(),
            spans: Vec::new(),
            notes: Vec::new(),
            steps: Vec::new(),
            folded_steps: Vec::new(),
        },
    };
    disk::write(directory.calls().join(format!("{call}.json")), serde_json::to_vec(&original)?)?;
    let update = JobFacts {
        call_id: "call".into(),
        machine: original.facts.machine.clone(),
        job: original.facts.job.clone().expect("job"),
        sequence: 1,
        turn: None,
        primary: true,
        state: Some(RunState::Running),
        record: None,
        script: None,
        started_at: Some(at),
        finished_at: None,
        output: None,
        notes: None,
        visited: vec![("remote".parse()?, "j-654321".parse()?)],
        writes: vec![WriteFact {
            turn: None,
            id: "write".into(),
            machine: original.facts.machine.clone(),
            path: "/work/result".into(),
            kind: fastctx_types::write::ChangeKind::Added,
            added: Some(1),
            removed: Some(0),
            unavailable: None,
            diff: None,
            note: None,
        }],
        steps: vec![RunStep {
            id: "step".into(),
            job: original.facts.job.clone().expect("job"),
            machine: original.facts.machine.clone(),
            began: at,
            ended: Some(at),
            did: StepDid::Plan { version: 1 },
        }],
    };
    disk::write(&job_path, serde_json::to_vec(&update)?)?;
    // A complete JSON frame without its newline was never committed by the old writer.
    disk::write(directory.calls().join(format!("{call}.updates")), serde_json::to_vec(&update)?)?;
    super::storage::database(&directory)?;
    let loaded = database.read_now(|db| sql::load(db, id.as_str(), "call"))?.expect("imported call");
    original.facts.writes = update.writes;
    original.facts.steps = update.steps;
    original.facts.visited = update.visited;
    assert_eq!(loaded.facts, original.facts);
    assert_eq!(loaded.sequence, 1);
    assert_eq!(crate::record_migration::archive::damaged_count(world.path())?, 0);
    assert_eq!(disk::read_dir(job_path.parent().expect("job directory"))?.count(), 0);
    println!(
        "legacy call crash window: job-only writes, steps and visits preserved; uncommitted journal tail retained in archive"
    );
    Ok(())
}
