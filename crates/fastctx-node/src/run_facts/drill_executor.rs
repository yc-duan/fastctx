//! Executor recovery, bounded replies, and the independent journal consumer.
use super::{journal, sql, step_view};
use fastctx_platform::disk;
use fastctx_storage::Database;
use fastctx_types::{
    layout::WorldDir,
    run::JobRecord,
    run_facts::{RunStep, StepDid},
};
use std::{
    io::{self, Write},
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let database = Database::open(
        &world.path().join("node.sqlite3"),
        &format!("{}{}{}", crate::records::SCHEMA, sql::SCHEMA, crate::record_payloads::SCHEMA),
    )?;
    crate::record_database::register(&world, &database);
    let directory = world.conversation(&"codex-a1b2c3".parse()?);
    let path = directory.path().join("jobs/j-abc123.json");
    disk::create_dir_all(path.parent().unwrap())?;
    let record: JobRecord = serde_json::from_value(serde_json::json!({
        "id":"j-abc123","machine":"executor","owner":{"machine":"executor","resident":"codex-a1b2c3"},
        "script":"true","cwd":directory.path(),"output":directory.path().join("out/j-abc123.log"),
        "started_at":"2026-01-01T00:00:00.000000001Z","ends_with_node":false
    }))?;
    disk::write(&path, serde_json::to_vec(&record)?)?;
    let at = record.started_at;
    let step = |index| RunStep {
        id: format!("step-{index}"),
        job: record.id.clone(),
        machine: record.machine.clone(),
        began: at,
        ended: Some(at),
        did: StepDid::Plan { version: index },
    };
    {
        // Published executors used one unsegmented file. Recovery must also
        // handle that file exceeding the new segment size.
        let mut output = io::BufWriter::new(disk::create(path.with_extension("steps.jsonl"))?);
        for index in 0..100_000 {
            serde_json::to_writer(&mut output, &step(index))?;
            output.write_all(b"\n")?;
        }
        output.flush()?;
    }
    database.write_now(128, |db| {
        db.execute_batch("CREATE TRIGGER step_view_fault BEFORE INSERT ON run_steps WHEN NEW.id='step-50' BEGIN SELECT RAISE(ABORT,'injected recovery failure'); END;")?;
        Ok(())
    })?;
    assert!(step_view::refresh(&path).is_err());
    database.write_now(128, |db| {
        db.execute_batch("DROP TRIGGER step_view_fault")?;
        Ok(())
    })?;
    step_view::refresh(&path)?;
    let hidden = path.with_extension("steps.hidden");
    disk::rename(path.with_extension("steps.jsonl"), &hidden)?;
    let gate = super::WRITING.get(&path.with_extension("facts"));
    let held = gate.lock();
    let mut samples = Vec::new();
    let mut bytes = 0;
    for _ in 0..100 {
        let started = Instant::now();
        let (steps, folded) = step_view::read(&path)?;
        samples.push(started.elapsed());
        assert_eq!(steps.len(), 200);
        assert_eq!(folded.iter().map(|row| row.count).sum::<u64>(), 99_800);
        bytes = serde_json::to_vec(&(steps, folded))?.len();
    }
    drop(held);
    samples.sort();
    assert!(samples[98] < Duration::from_millis(100), "the executor reply exceeded its interactive budget");
    println!(
        "executor 100000-step view: 200 hot / 99800 folded, {bytes} bytes, read p99 {:?}; journal absent and writer gate held",
        samples[98]
    );
    disk::rename(&hidden, path.with_extension("steps.jsonl"))?;
    {
        let _writing = gate.lock();
        let mut bytes = serde_json::to_vec(&step(100_000))?;
        bytes.push(b'\n');
        journal::append(&path, &bytes, 0)?;
        step_view::refresh_locked(&path)?;
    }
    let (steps, folded) = step_view::read(&path)?;
    assert_eq!(steps.len(), 200);
    assert_eq!(folded.iter().map(|row| row.count).sum::<u64>(), 99_801);
    super::drill_frames::run(&database, &path)?;
    crate::job_metadata::Job::open(&path)?.delete_all()?;
    assert_eq!(
        database.read_now(|db| Ok(db.query_row("SELECT count(*) FROM run_steps", [], |row| row.get::<_, i64>(0))?))?,
        0
    );
    println!("executor interrupted recovery, resumed append and job-view collection: passed");
    Ok(())
}
