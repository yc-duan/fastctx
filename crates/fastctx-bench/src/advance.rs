//! §18's advance rows: a Claude Code record grows by one line of 4 KiB, and
//! the follower takes it to a commit — the prefix checked by its xxh3 blocks
//! (whole up to 32 MiB, two blocks past that), the tail past the seam read,
//! read again and projected, its events committed to a library. The record
//! is 32 MiB for the small prefix and 1 GiB for the large one, prompts and
//! answers of fixed-seed printable ASCII. Timed from the line on the disk to
//! the commit returning: what the follower and the storage core do of it,
//! not the node's watching for the change nor its mapping of events to its
//! own items.

use crate::fixture::Seeded;
use crate::stats::Samples;
use crate::storage::{SCOPE, Scale, error, three};
use fastctx_content::v2::{Commit, EVENT, Kind as _, KindRecord, Timeline};
use fastctx_content::{Record, State};
use fastctx_hosts::claude::transcript::follow;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

/// One row of the record: the `n`th, a user's words when `n` is even, else
/// an answer to them, its text `bytes` long.
fn row(n: u64, bytes: usize, seeded: &mut Seeded) -> Value {
    // Ten milliseconds a row, onward through a day: a record of a GiB has
    // about half a million rows, and its times never go back.
    let ms = n * 10;
    let at = format!("2026-11-12T{:02}:{:02}:{:02}.{:03}Z", ms / 3_600_000 % 24, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    let text = seeded.text(bytes);
    if n.is_multiple_of(2) {
        json!({"type": "user", "uuid": format!("u{n}"), "parentUuid": null, "sessionId": "bench", "timestamp": at,
               "message": {"role": "user", "content": text}})
    } else {
        json!({"type": "assistant", "uuid": format!("a{n}"), "parentUuid": format!("u{}", n - 1), "sessionId": "bench",
               "timestamp": at, "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
    }
}

fn line(value: &Value) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(value).map_err(error)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) fn advance(dir: &Path, scale: Scale, prefix: u64, row_name: &str) -> Result<bool, String> {
    let prefix = match scale {
        Scale::Full => prefix,
        Scale::Small => prefix / 10,
    };
    let path = dir.join("record.jsonl");
    let mut seeded = Seeded::new(23);
    let mut n = 0u64;
    {
        let mut file = BufWriter::with_capacity(1 << 20, std::fs::File::create(&path).map_err(error)?);
        let mut written = 0u64;
        while written < prefix {
            let bytes = if n.is_multiple_of(2) { 200 } else { 4000 };
            let line = line(&row(n, bytes, &mut seeded))?;
            file.write_all(&line).map_err(error)?;
            written += line.len() as u64;
            n += 1;
        }
        file.flush().map_err(error)?;
    }
    let three = three(dir)?;
    let holding = three.producer.holding(SCOPE).map_err(error)?.ok_or("not held")?;
    let earlier = HashSet::new();
    let accept = |_| true;
    // The first reading is the whole record, as a follower's first is, and
    // as the background check of a whole history reads it (§18, the whole
    // prefix checked: its reading and parsing, one thread, so its time is
    // its CPU's; what the check then compares with the library and writes is
    // not in it).
    let mut seen: HashSet<String> = HashSet::new();
    let started = Instant::now();
    let first = follow::tail(&path, &follow::Seam::default(), &earlier, &accept, false, &|_| false)
        .map_err(error)?
        .map_err(|why| format!("the first reading restarted: {why}"))?;
    crate::stats::rate(row_name, "the whole record read and parsed once", prefix as f64 / f64::from(1 << 20), "MiB", started.elapsed(), 0.0, json!({ "events": first.transcript.events.len() }));
    seen.extend(first.prefix_ids.iter().cloned());
    let mut seam = first.seam.ok_or("no seam after the first reading")?;
    let mut number = 0u64;
    let (mut samples, mut followed, mut committed) = (Samples::default(), Samples::default(), Samples::default());
    for round in 0..1020 {
        let appended = line(&row(n, 4000, &mut seeded))?;
        n += 1;
        {
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).map_err(error)?;
            file.write_all(&appended).map_err(error)?;
        }
        let started = Instant::now();
        let read = follow::tail(&path, &seam, &earlier, &accept, false, &|id: &str| seen.contains(id)).map_err(error)?;
        let Ok(tail) = read else {
            samples.failed += 1;
            continue;
        };
        let read_at = started.elapsed();
        let records: Vec<KindRecord> = tail
            .transcript
            .events
            .iter()
            .map(|event| {
                number += 1;
                let mut body = vec![EVENT];
                body.extend_from_slice(&serde_json::to_vec(event).unwrap_or_default());
                let key = number.to_be_bytes().to_vec();
                KindRecord {
                    kind: Timeline::KIND,
                    schema: Timeline::SCHEMA,
                    record: Record { item: key.clone(), order: key, state: State::Is { body, objects: Vec::new() } },
                }
            })
            .collect();
        if !records.is_empty() {
            let commit = Commit { scope: SCOPE.into(), stream: Timeline::STREAM.into(), records, leases: Vec::new(), durable: false, op: None, holding };
            three.producer.raw_commit(commit, three.device.clone()).map_err(error)?;
        }
        if round >= 20 {
            let took = started.elapsed();
            samples.push(took);
            followed.push(read_at);
            committed.push(took - read_at);
        }
        seen.extend(tail.prefix_ids.iter().cloned());
        if let Some(moved) = tail.seam {
            seam = moved;
        }
    }
    let shape = json!({ "record_bytes": prefix, "line_bytes": 4096 });
    followed.report(row_name, "of it, the follower's reading", None, shape.clone());
    committed.report(row_name, "of it, the commit", None, shape.clone());
    Ok(samples.report(row_name, "a line of 4 KiB appended, followed to its commit", Some(20.0), shape))
}
