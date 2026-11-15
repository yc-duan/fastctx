//! §18's first import and large single value: a Claude Code record read by
//! the hosts' follower from its start — the reading a conversation's first
//! import makes — and its events committed to a library. The first import
//! is a record of 532 lines, 2,163,892 bytes as quick-1 is, read warm 20
//! times; the large value is a record whose one line holds 287 MB, read once
//! with this process's resident memory sampled every 5 ms for how much it
//! grew. What the node does besides (its watching, mapping events to its
//! items) is not in either.

use crate::fixture::Seeded;
use crate::stats::Samples;
use crate::storage::{SCOPE, Scale, error, three};
use fastctx_content::v2::{Commit, EVENT, Kind as _, KindRecord, OVERSIZED, Timeline};
use fastctx_content::{Record, State};
use fastctx_hosts::claude::transcript::follow;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The most a record's body holds; an event larger is an object.
const RECORD_BYTES: usize = 64 << 10;
/// The most records, and bytes of them, the node commits of a timeline at
/// once (`timeline/assets.rs`).
const COMMIT_RECORDS: usize = 1_024;
const COMMIT_BYTES: usize = 4 << 20;

fn row(n: u64, text: String) -> Value {
    let ms = n * 10;
    let at = format!("2026-11-12T{:02}:{:02}:{:02}.{:03}Z", ms / 3_600_000 % 24, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if n.is_multiple_of(2) {
        json!({"type": "user", "uuid": format!("u{n}"), "parentUuid": null, "sessionId": "bench", "timestamp": at,
               "message": {"role": "user", "content": text}})
    } else {
        json!({"type": "assistant", "uuid": format!("a{n}"), "parentUuid": format!("u{}", n - 1), "sessionId": "bench",
               "timestamp": at, "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
    }
}

/// Writes `lines` rows whose JSON lines come to `bytes` in all, the last
/// one taking up what the others leave.
fn record(path: &Path, lines: u64, bytes: u64, seeded: &mut Seeded) -> Result<(), String> {
    let mut file = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path).map_err(error)?);
    let mut written = 0u64;
    for n in 0..lines {
        let shell = serde_json::to_vec(&row(n, String::new())).map_err(error)?.len() as u64 + 1;
        let left = bytes.saturating_sub(written);
        let share = if n + 1 == lines { left } else { left / (lines - n) };
        let text = seeded.text(share.saturating_sub(shell) as usize);
        let mut line = serde_json::to_vec(&row(n, text)).map_err(error)?;
        line.push(b'\n');
        written += line.len() as u64;
        file.write_all(&line).map_err(error)?;
    }
    file.flush().map_err(error)
}

/// Reads `path` from its start and commits what it shows; how many events.
fn import(three: &crate::storage::Three, path: &Path, number: &mut u64) -> Result<usize, String> {
    imported(three, path, number).map(|(events, _)| events)
}

/// As `import`, with whether the reading stopped before a line larger than
/// World reads (design §28.49).
fn imported(three: &crate::storage::Three, path: &Path, number: &mut u64) -> Result<(usize, bool), String> {
    let holding = three.producer.holding(SCOPE).map_err(error)?.ok_or("not held")?;
    let read = follow::tail(path, &follow::Seam::default(), &HashSet::new(), &|_| true, false, &|_| false)
        .map_err(error)?
        .map_err(|why| format!("the reading restarted: {why}"))?;
    let events = read.transcript.events.len();
    let refused = read.transcript.problems.iter().any(|problem| problem.contains("more than World reads"));
    let (mut records, mut leases) = (Vec::new(), Vec::new());
    for event in &read.transcript.events {
        *number += 1;
        let encoded = serde_json::to_vec(event).map_err(error)?;
        // As the node keeps an event too large for a record: what stands in
        // its place, the whole of it an object (`timeline/assets.rs` body).
        let (body, objects) = if encoded.len() < RECORD_BYTES {
            let mut body = vec![EVENT];
            body.extend_from_slice(&encoded);
            (body, Vec::new())
        } else {
            let lease = three.producer.put(&mut encoded.as_slice(), "bench", Duration::from_secs(600)).map_err(error)?;
            let mut body = vec![OVERSIZED];
            body.extend_from_slice(&serde_json::to_vec(&json!({ "bytes": encoded.len(), "slim": null })).map_err(error)?);
            let objects = vec![lease.object];
            leases.push(lease);
            (body, objects)
        };
        let key = number.to_be_bytes().to_vec();
        records.push(KindRecord {
            kind: Timeline::KIND,
            schema: Timeline::SCHEMA,
            record: Record { item: key.clone(), order: key, state: State::Is { body, objects } },
        });
    }
    let mut leases = leases.into_iter();
    // As the node commits a timeline it writes (`timeline/assets.rs`): up
    // to 1,024 records or 4 MiB of them a commit.
    let mut parts: Vec<Vec<KindRecord>> = Vec::new();
    let mut bytes = 0;
    for record in records {
        let size = match &record.record.state {
            State::Is { body, .. } => body.len(),
            State::Gone => 0,
        };
        if parts.last().is_none_or(|part| part.len() >= COMMIT_RECORDS || bytes + size > COMMIT_BYTES) {
            parts.push(Vec::new());
            bytes = 0;
        }
        bytes += size;
        parts.last_mut().expect("a part").push(record);
    }
    for part in parts {
        let leasing = part.iter().filter(|record| matches!(&record.record.state, State::Is { objects, .. } if !objects.is_empty())).count();
        let commit = Commit {
            scope: SCOPE.into(),
            stream: Timeline::STREAM.into(),
            records: part,
            leases: leases.by_ref().take(leasing).collect(),
            durable: false,
            op: None,
            holding,
        };
        three.producer.raw_commit(commit, three.device.clone()).map_err(error)?;
    }
    Ok((events, refused))
}

pub(crate) fn first_import(dir: &Path, _scale: Scale) -> Result<bool, String> {
    let path = dir.join("quick-1.jsonl");
    record(&path, 532, 2_163_892, &mut Seeded::new(24))?;
    let three = three(dir)?;
    let (mut number, mut samples, mut events) = (0u64, Samples::default(), 0);
    for round in 0..25 {
        let started = Instant::now();
        events = import(&three, &path, &mut number)?;
        if round >= 5 {
            samples.push(started.elapsed());
        }
    }
    Ok(samples.report("first-import", "a record of 532 lines, 2,163,892 bytes, read from its start and committed, warm", Some(40.0), json!({ "events": events })))
}

/// The most this process held resident while `work` ran, above what it held
/// before, sampled every 5 ms.
fn grown<T>(work: impl FnOnce() -> T) -> Result<(T, u64), String> {
    let before = fastctx_platform::memory::resident().map_err(error)?;
    let (most, done) = (Arc::new(AtomicU64::new(before)), Arc::new(AtomicBool::new(false)));
    let sampler = {
        let (most, done) = (most.clone(), done.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                if let Ok(now) = fastctx_platform::memory::resident() {
                    most.fetch_max(now, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    let result = work();
    done.store(true, Ordering::Relaxed);
    let _ = sampler.join();
    Ok((result, most.load(Ordering::Relaxed).saturating_sub(before)))
}

pub(crate) fn large_value(dir: &Path, scale: Scale) -> Result<bool, String> {
    let bytes: u64 = match scale {
        Scale::Full => 287_000_000,
        Scale::Small => 28_700_000,
    };
    let path = dir.join("large.jsonl");
    {
        let mut seeded = Seeded::new(25);
        let mut file = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&path).map_err(error)?);
        for n in 0..4u64 {
            let mut line = serde_json::to_vec(&row(n, seeded.text(2000))).map_err(error)?;
            line.push(b'\n');
            file.write_all(&line).map_err(error)?;
        }
        let mut line = serde_json::to_vec(&row(4, seeded.text(bytes as usize))).map_err(error)?;
        line.push(b'\n');
        file.write_all(&line).map_err(error)?;
        file.flush().map_err(error)?;
    }
    let three = three(dir)?;
    let mut number = 0u64;
    let started = Instant::now();
    let (read, grew) = grown(|| imported(&three, &path, &mut number))?;
    let took = started.elapsed();
    let (events, refused) = read?;
    let mib = grew as f64 / f64::from(1 << 20);
    // A line larger than World reads is not read: the reading stops before
    // it and says so, the rows before it committed (design §28.49); one
    // within the bound is read whole.
    let larger = bytes as usize > fastctx_hosts::bounded::LINE_MOST;
    // The four rows before the large line, each one event; with it, five.
    let met = took <= Duration::from_secs(5) && mib <= 64.0 && refused == larger && events == if larger { 4 } else { 5 };
    let measure = if larger {
        "a record whose last line holds 287 MB, more than World reads: read from its start up to that line, which is refused and told, the rows before it committed"
    } else {
        "a record whose last line holds 28.7 MB, within what World reads: read from its start and committed"
    };
    println!(
        "{}",
        json!({ "row": "large-value", "measure": measure, "seconds": took.as_secs_f64(), "grew_mib": mib, "events": events,
                "refused": refused, "target_seconds": 5.0, "target_mib": 64.0, "met": met })
    );
    Ok(met)
}
