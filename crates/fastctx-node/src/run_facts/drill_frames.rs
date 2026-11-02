//! Publication failure and transport retry over a page larger than one letter.
use super::{frame_queue, updates::Emitted};
use crate::job_metadata::Job;
use fastctx_storage::Database;
use fastctx_types::{run::RunState, run_facts::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) fn run(database: &Database, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let record = fastctx_hands::run::record::read(path)?;
    let update = JobFacts {
        record: Some(path.to_string_lossy().into()),
        script: Some(record.script.clone()),
        started_at: Some(record.started_at),
        finished_at: Some(record.started_at),
        turn: None,
        call_id: "large-frame".into(),
        primary: true,
        output: None,
        notes: Some(
            (0..20_000)
                .map(|index| OutputNoteMatch {
                    note: OutputNote { command: NoteCommand::View, write: None },
                    from: index * 128,
                    to: index * 128 + 100,
                })
                .collect(),
        ),
        visited: Vec::new(),
        machine: record.machine.clone(),
        job: record.id.clone(),
        sequence: 0,
        state: Some(RunState::Running),
        steps: Vec::new(),
        writes: (0..128)
            .map(|index| WriteFact {
                turn: None,
                id: format!("{index:x}"),
                machine: record.machine.clone(),
                path: "/work/file".into(),
                kind: fastctx_types::write::ChangeKind::Modified,
                added: Some(1),
                removed: Some(1),
                unavailable: None,
                diff: None,
                note: Some(WriteNote::Text { command: NoteCommand::Write, text: "x".repeat(192 * 1024) }),
            })
            .collect(),
    };
    let original = serde_json::to_vec(&update)?.len();
    assert!(original > 16 * 1024 * 1024, "the fixture must reproduce the transport rejection");
    let job = Job::open(path)?;
    let mut emitted = Emitted::default();
    database.write_now(512, |db| {
        db.execute_batch("CREATE TRIGGER fact_queue_fault BEFORE INSERT ON conversation_records WHEN NEW.family LIKE '%/facts-queue' BEGIN SELECT RAISE(ABORT,'injected manifest failure'); END;")?;
        Ok(())
    })?;
    assert!(frame_queue::publish(&job, update.clone(), &mut emitted).is_err());
    assert!(job.get::<Emitted>("facts-state")?.is_none(), "failed publication advanced the source cursor");
    database.write_now(128, |db| {
        db.execute_batch("DROP TRIGGER fact_queue_fault")?;
        Ok(())
    })?;
    job.put("facts-pending", &update)?;
    frame_queue::publish(&job, update.clone(), &mut emitted)?;
    assert!(job.raw("facts-pending")?.is_none(), "legacy source must retire with the manifest");
    assert!(frame_queue::publish(&job, update, &mut emitted).is_err(), "a published page cannot be staging garbage");
    let mut frames = BTreeMap::new();
    let mut attempts = 0;
    let first = frame_queue::drain(&job, |frame| {
        frames.insert(frame.sequence, serde_json::to_vec(frame)?);
        attempts += 1;
        if attempts == 2 {
            return Err(std::io::Error::other("connection failed after receiver commit"));
        }
        Ok(())
    });
    assert!(first.is_err());
    let retry = *frames.last_key_value().unwrap().0;
    let mut seen_retry = false;
    let mut identities = BTreeSet::new();
    for bytes in frames.values() {
        let frame: JobFacts = serde_json::from_slice(bytes)?;
        identities.extend(frame.writes.into_iter().map(|fact| fact.id));
    }
    let job = Job::open(path)?;
    frame_queue::drain(&job, |frame| {
        let bytes = serde_json::to_vec(frame)?;
        assert!(bytes.len() < 16 * 1024 * 1024);
        if frame.sequence == retry {
            assert_eq!(frames.get(&retry), Some(&bytes));
            seen_retry = true;
        }
        identities.extend(frame.writes.iter().map(|fact| fact.id.clone()));
        frames.insert(frame.sequence, bytes);
        Ok(())
    })?;
    assert!(seen_retry);
    assert_eq!(identities.len(), 128);
    let last = frames.last_key_value().unwrap().0;
    for (sequence, bytes) in &frames {
        let frame: JobFacts = serde_json::from_slice(bytes)?;
        assert_eq!(frame.state.is_some(), sequence == last);
        assert_eq!(frame.script.as_deref(), Some(record.script.as_str()));
        assert_eq!(frame.started_at, Some(record.started_at));
        assert!(frame.record.is_some(), "legacy receivers retain the last frame as their job source");
        if sequence == last {
            assert_eq!(frame.notes.unwrap().len(), 4096);
        }
    }
    frame_queue::drain(&job, |_| panic!("a drained queue sent again"))?;
    println!(
        "oversize fact page {original} bytes: {} frames, max {} bytes; interrupted publication and lost acknowledgment replay passed",
        frames.len(),
        frames.values().map(Vec::len).max().unwrap()
    );
    Ok(())
}
