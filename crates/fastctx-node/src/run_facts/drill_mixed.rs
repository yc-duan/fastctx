//! Mixed historical facts exercise the read path, not only the step counter.
use super::{acceptance, projections, sql};
use crate::record_payloads;
use fastctx_storage::{Database, Priority};
use fastctx_types::{run_facts::*, write::ChangeKind};
use std::time::{Duration, Instant};

pub(super) fn run(database: &Database, template: &JobFacts) -> Result<(), Box<dyn std::error::Error>> {
    let mut mixed = template.clone();
    mixed.sequence += 1;
    mixed.steps.clear();
    mixed.visited.clear();
    mixed.writes.clear();
    let notes: Vec<_> = (0..20000)
        .map(|i| OutputNoteMatch {
            note: OutputNote { command: NoteCommand::Write, write: Some(format!("write-{}", i % 2196)) },
            from: i * 128,
            to: i * 128 + 100,
        })
        .collect();
    mixed.notes = Some(notes.clone());
    for i in 0..2196 {
        mixed.visited.push((mixed.machine.clone(), format!("j-{i:06x}").parse()?));
        mixed.writes.push(WriteFact {
            turn: Some("turn".into()),
            id: format!("write-{i}"),
            machine: mixed.machine.clone(),
            path: format!("/work/file-{i}.txt"),
            kind: ChangeKind::Added,
            added: Some(1),
            removed: Some(0),
            unavailable: None,
            note: None,
            diff: Some(DiffRef {
                content: None,
                machine: mixed.machine.clone(),
                path: format!("/artifacts/diff-{i}"),
                bytes: Some(700),
                lines: Some(vec![DiffLine {
                    kind: DiffLineKind::Added,
                    before: None,
                    after: Some(1),
                    text: "x".repeat(512),
                    ending: "\n".into(),
                }]),
            }),
        });
    }
    acceptance::receive(database, "conversation", &mixed, |_| {})?;
    let spans = vec![OutputSpan {
        note: None,
        piece: DisplayPiece::Text { first_line: 1, text: "selected output\n".repeat(100000) },
    }];
    let mut stored = database.read_now(|db| sql::head(db, "conversation", "call"))?.ok_or("missing mixed head")?;
    stored.facts.notes = notes.clone();
    stored.facts.spans = spans.clone();
    let prepared = projections::Call::prepare(database, "conversation", &mut stored, Priority::Normal)?;
    database.write_now(4096, move |db| {
        prepared.install(db, "conversation", "call", false)?;
        sql::save(db, "conversation", &stored)
    })?;
    record_payloads::collect(database, "conversation")?;
    let mut reads = Vec::new();
    for _ in 0..200 {
        let started = Instant::now();
        let stored = database.read_now(|db| sql::load(db, "conversation", "call"))?.ok_or("mixed facts missing")?;
        reads.push(started.elapsed());
        assert_eq!(stored.facts.visited.len(), 2196);
        assert_eq!(stored.facts.writes.len(), 2196);
        assert_eq!(stored.facts.notes, notes);
        assert_eq!(stored.facts.spans, spans);
        assert!(
            stored
                .facts
                .writes
                .iter()
                .filter_map(|fact| fact.diff.as_ref())
                .filter(|diff| diff.lines.is_some())
                .count()
                < 2196
        );
    }
    reads.sort();
    assert!(reads[197] < Duration::from_millis(300), "mixed facts read exceeded 300 ms");
    let payload_before = database.read_now(|db| {
        Ok(db.query_row("SELECT count(*),sum(length(body)) FROM record_payload_chunks", [], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?)
    })?;
    let before = database.statistics();
    for i in 1..=100 {
        let mut progress = template.clone();
        progress.sequence = mixed.sequence + i;
        progress.steps.clear();
        progress.writes.clear();
        progress.visited.clear();
        progress.notes = None;
        acceptance::receive(database, "conversation", &progress, |_| {})?;
    }
    let after = database.statistics();
    let payload_after = database.read_now(|db| {
        Ok(db.query_row("SELECT count(*),sum(length(body)) FROM record_payload_chunks", [], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?)
    })?;
    assert_eq!(payload_before, payload_after, "progress rewrote selected output history");
    assert_eq!(database.read_now(|db| sql::load(db, "conversation", "call"))?.unwrap().facts.notes, notes);
    println!(
        "mixed 100k-step call: 2196 visits + 2196 writes + 20000 note matches + 1.6 MiB selected output; read p99 {:?}; 100 metadata updates: {} SQL operations, {} over 2 ms; immutable payload {:?}",
        reads[197],
        after.operations - before.operations,
        after.writes_over_budget - before.writes_over_budget,
        payload_after
    );
    Ok(())
}
