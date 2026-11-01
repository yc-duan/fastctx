//! Performance floor for indexed call facts and old-executor duplicate updates.

use super::{snapshot::StoredCall, sql};
use fastctx_storage::Database;
use fastctx_types::{
    JobId, MachineName,
    run::RunState,
    run_facts::{JobFacts, RunFacts, RunStep, StepDid},
};
use std::path::Path;
use std::time::{Duration, Instant};

pub fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !root.is_absolute() || root.exists() {
        return Err("the drill needs an absolute, absent root".into());
    }
    std::fs::create_dir_all(root)?;
    let database = Database::open_initialized(
        &root.join("node.sqlite3"),
        &format!("{}{}", sql::SCHEMA, crate::record_payloads::SCHEMA),
        crate::conversation_store::schema::initialize,
    )?;
    let machine: MachineName = "executor".parse()?;
    let job: JobId = "j-123456".parse()?;
    let at: jiff::Timestamp = "2026-01-01T00:00:00Z".parse()?;
    let stored = StoredCall {
        call_id: "call".into(),
        sequence: 0,
        launched: false,
        script: "x".repeat(128 * 1024),
        unavailable: None,
        facts: RunFacts {
            machine: machine.clone(),
            job: Some(job.clone()),
            turn: Some("turn".into()),
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
    database.write_now(256 * 1024, move |db| sql::save(db, "conversation", &stored))?;
    let template = JobFacts {
        call_id: "call".into(),
        machine: machine.clone(),
        job: job.clone(),
        sequence: 0,
        turn: Some("turn".into()),
        primary: true,
        state: Some(RunState::Running),
        record: None,
        script: None,
        started_at: Some(at),
        finished_at: None,
        output: None,
        notes: None,
        visited: Vec::new(),
        writes: Vec::new(),
        steps: Vec::new(),
    };
    let mut costs = Vec::new();
    for batch in 0..1000u64 {
        let mut update = template.clone();
        update.sequence = batch + 1;
        for index in batch * 100..(batch + 1) * 100 {
            update.steps.push(RunStep {
                id: format!("step-{index}"),
                job: job.clone(),
                machine: machine.clone(),
                began: at,
                ended: Some(at),
                did: StepDid::Plan { version: index },
            });
        }
        let before = Instant::now();
        super::acceptance::receive(&database, "conversation", &update, |_| {})?;
        costs.push(before.elapsed());
    }
    let mut reads = Vec::new();
    for _ in 0..200 {
        let start = Instant::now();
        let stored = database.read_now(|db| sql::load(db, "conversation", "call"))?.ok_or("missing call")?;
        assert_eq!(stored.facts.steps.len(), 200);
        assert_eq!(stored.facts.folded_steps.iter().map(|part| part.count).sum::<u64>(), 99800);
        assert_eq!(stored.script.len(), 128 * 1024);
        reads.push(start.elapsed());
    }
    reads.sort();
    assert!(reads[197] < Duration::from_millis(300), "call read exceeded 300 ms");
    let mut duplicate = template;
    duplicate.sequence = 1001;
    for index in 0..2196 {
        duplicate.steps.push(RunStep {
            id: format!("step-{index}"),
            job: job.clone(),
            machine: machine.clone(),
            began: at,
            ended: Some(at),
            did: StepDid::Plan { version: index },
        });
        duplicate.visited.push((machine.clone(), format!("j-{index:06x}").parse()?));
    }
    let baseline =
        database.read_now(|db| Ok(db.query_row("SELECT count(*) FROM run_steps", [], |row| row.get::<_, i64>(0))?))?;
    let repeated = duplicate.clone();
    super::acceptance::receive(&database, "conversation", &repeated, |_| {})?;
    let repeat = Instant::now();
    for _ in 0..100 {
        let repeated = duplicate.clone();
        assert!(!super::acceptance::receive(&database, "conversation", &repeated, |_| {})?);
    }
    let repeated_in = repeat.elapsed();
    let count =
        database.read_now(|db| Ok(db.query_row("SELECT count(*) FROM run_steps", [], |row| row.get::<_, i64>(0))?))?;
    assert_eq!(count, baseline);
    let mut late = duplicate.clone();
    late.sequence = 2;
    late.visited.clear();
    late.writes.clear();
    late.steps.clear();
    late.steps.push(RunStep {
        id: "arrived-late".into(),
        job: job.clone(),
        machine: machine.clone(),
        began: at,
        ended: Some(at),
        did: StepDid::Plan { version: 100001 },
    });
    late.record = Some("/jobs/record.json".into());
    late.script = Some("initial metadata delivered after newer progress".into());
    assert!(super::acceptance::receive(&database, "conversation", &late, |_| {})?);
    let mut latest = database.read_now(|db| sql::job(db, "conversation", &late))?.ok_or("missing job")?;
    assert_eq!(latest.sequence, 1001);
    database.read_now(|db| {
        let tx = db.unchecked_transaction()?;
        sql::job_source(&tx, "conversation", &mut latest)
    })?;
    assert_eq!(latest.record.as_deref(), Some("/jobs/record.json"));
    assert!(!super::acceptance::receive(&database, "conversation", &late, |_| {})?);
    let count =
        database.read_now(|db| Ok(db.query_row("SELECT count(*) FROM run_steps", [], |row| row.get::<_, i64>(0))?))?;
    assert_eq!(count, 100001, "older progress lost its unique step");
    let mut invalid = late.clone();
    invalid.turn = Some("another-turn".into());
    invalid.steps[0].id = "rejected-step".into();
    assert!(
        super::acceptance::receive(&database, "conversation", &invalid, |_| panic!("rejected frame was published"))
            .is_err()
    );
    assert_eq!(
        database.read_now(|db| Ok(db.query_row(
            "SELECT count(*) FROM run_steps WHERE id='rejected-step'",
            [],
            |row| row.get::<_, i64>(0)
        )?))?,
        0
    );
    let mut ended = late.clone();
    ended.sequence = 1002;
    ended.state = Some(RunState::Failed { error: "drill terminal".into() });
    ended.steps[0].id = "terminal-step".into();
    let mut shown = RunState::Running;
    let mut sequence = 1001;
    super::acceptance::receive(&database, "conversation", &ended, |frame| {
        // The shipped interface only updates metadata for a newer sequence.
        if frame.sequence > sequence {
            sequence = frame.sequence;
            if let Some(state) = frame.state {
                shown = state;
            }
        }
    })?;
    assert_eq!(Some(shown), ended.state, "member notification consumed the terminal metadata sequence");
    super::drill_mixed::run(&database, &ended)?;
    let stats = database.statistics();
    println!("SQL operations {}, over 2 ms {}", stats.operations, stats.writes_over_budget);
    assert!(stats.writes_over_budget * 100 <= stats.operations, "SQL operation p99 exceeded 2 ms");
    println!(
        "100000 steps: 200 hot, 99800 folded; call read p99 {:?}; 100 duplicates {:?}; slow operations {}",
        reads[197], repeated_in, stats.slow_operations
    );
    let head_size = database.read_now(|db| {
        Ok(db.query_row("SELECT length(body) FROM run_calls WHERE call='call'", [], |row| row.get::<_, i64>(0))?)
    })?;
    assert!(head_size < 4096, "stable script or collections leaked into the mutable head");
    let first = costs[..100].iter().map(Duration::as_micros).sum::<u128>() / 100;
    let last = costs[900..].iter().map(Duration::as_micros).sum::<u128>() / 100;
    println!("100-step commit mean: first {} us, last {} us; mutable head {} bytes", first, last, head_size);
    database.write_now(64, |db| {
        db.execute("INSERT INTO node_conversation_deletions(name,started) VALUES('conversation',0)", [])?;
        Ok(())
    })?;
    let deleted = crate::conversation_store::purge(&database, "conversation")?;
    let mut after_deletion = ended.clone();
    after_deletion.sequence += 1;
    assert!(
        super::acceptance::receive(&database, "conversation", &after_deletion, |_| panic!(
            "a deleted job was published"
        ))
        .is_err()
    );
    database.read_now(|db| {
        for table in crate::conversation_store::schema::tables(db)? {
            let count: i64 = db.query_row(
                &format!(
                    "SELECT count(*) FROM {} WHERE conversation='conversation'",
                    crate::conversation_store::schema::quote(&table.name)
                ),
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0, "{} retained job facts", table.name);
        }
        Ok(())
    })?;
    println!("100000-step conversation deleted: {deleted} logical bytes; late job facts rejected");
    crate::session_record::drill::run(&root.join("events"))?;
    Ok(())
}
