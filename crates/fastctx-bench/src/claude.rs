//! §28.51's Claude rows: a long record of Claude Code's read whole as the
//! node reads a session's history (`transcript::read_continuing`), with this
//! process's resident memory as it reads.
//!
//! - `claude-relist`: a record of small rows — about a million, 200 MB at
//!   full scale — words, answers, and tool calls with their results.
//! - `claude-relist-large`: a record whose bytes are in large tool results:
//!   2,000 results of 64 KiB among small rows.
//!
//! Each is read three ways: whole, as the node reads it now (the row's own
//! name); as a history whose events are handed on and let go
//! (`…-history`, `transcript::read_history`); and so with its rows kept in
//! a work store on disk, the events kept there as they are made
//! (`…-kept`, `transcript::read_history_kept`, §28.51 ⑤-3).

use crate::codex::Sampled;
use crate::fixture::Seeded;
use crate::storage::{Scale, error};
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Writes a record of `rounds` exchanges, each a prompt, an answer that calls
/// a tool, the tool's result (`result` bytes long every `every`th round, else
/// short) and a closing answer.
fn record(dir: &Path, rounds: usize, result: usize, every: usize) -> Result<(PathBuf, u64), String> {
    std::fs::create_dir_all(dir).map_err(error)?;
    let path = dir.join("record.jsonl");
    let mut file = std::io::BufWriter::new(std::fs::File::create(&path).map_err(error)?);
    let mut seeded = Seeded::new(28);
    let mut parent = serde_json::Value::Null;
    let mut bytes = 0u64;
    let mut write = |row: serde_json::Value, file: &mut std::io::BufWriter<std::fs::File>| -> Result<(), String> {
        let mut line = serde_json::to_vec(&row).map_err(error)?;
        line.push(b'\n');
        bytes += line.len() as u64;
        file.write_all(&line).map_err(error)
    };
    let base = jiff::Timestamp::from_second(1_800_000_000).map_err(error)?;
    for round in 0..rounds {
        let at = |step: i64| (base + jiff::SignedDuration::from_millis(round as i64 * 4_000 + step * 900)).to_string();
        let (prompt, answer, result_id, closing) = (format!("u{round}"), format!("a{round}"), format!("r{round}"), format!("c{round}"));
        let call = format!("toolu_{round}");
        let mut words = |low, high| {
            let length = seeded.between(low, high);
            seeded.text(length)
        };
        let (asked, said, closed) = (words(20, 80), words(10, 40), words(10, 40));
        write(
            json!({"type": "user", "uuid": prompt, "parentUuid": parent, "sessionId": "bench", "timestamp": at(0),
                   "message": {"role": "user", "content": asked}}),
            &mut file,
        )?;
        write(
            json!({"type": "assistant", "uuid": answer, "parentUuid": prompt, "sessionId": "bench", "timestamp": at(1),
                   "message": {"id": format!("msg_{round}"), "role": "assistant", "model": "claude-bench",
                               "content": [{"type": "text", "text": said},
                                           {"type": "tool_use", "id": call, "name": "Bash", "input": {"command": "ls"}}],
                               "usage": {"input_tokens": 10, "output_tokens": 5}}}),
            &mut file,
        )?;
        let length = if every > 0 && round % every == every - 1 { result } else { seeded.between(10, 60) };
        let output = seeded.text(length);
        write(
            json!({"type": "user", "uuid": result_id, "parentUuid": answer, "sessionId": "bench", "timestamp": at(2),
                   "sourceToolAssistantUUID": answer, "toolUseResult": {"stdout": "", "stderr": ""},
                   "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": call, "content": output}]}}),
            &mut file,
        )?;
        write(
            json!({"type": "assistant", "uuid": closing, "parentUuid": result_id, "sessionId": "bench", "timestamp": at(3),
                   "message": {"id": format!("msg_{round}b"), "role": "assistant", "model": "claude-bench",
                               "content": [{"type": "text", "text": closed}],
                               "usage": {"input_tokens": 10, "output_tokens": 5}}}),
            &mut file,
        )?;
        parent = json!(closing);
    }
    file.flush().map_err(error)?;
    Ok((path, bytes))
}

/// How a record is read (the module's three ways).
#[derive(Clone, Copy)]
enum Way {
    Whole,
    History,
    Kept,
}

fn read(row: &str, measure: &str, path: &Path, bytes: u64, way: Way) -> Result<bool, String> {
    let sampled = Sampled::start()?;
    let started = Instant::now();
    let earlier = std::collections::HashSet::new();
    let (events, problems) = match way {
        Way::Whole => {
            let read = fastctx_hosts::claude::transcript::read_continuing(path, &[], &earlier).map_err(error)?;
            (read.events.len(), read.problems.len())
        }
        Way::History => {
            let mut events = 0;
            let read = fastctx_hosts::claude::transcript::read_history(path, &[], &earlier, &mut |_| {
                events += 1;
                Ok(())
            })
            .map_err(error)?;
            (events, read.problems.len())
        }
        Way::Kept => {
            let root = path.parent().ok_or("a record with no directory")?.join("spill");
            fastctx_node::bench::Follower::claude_kept(root, path)?
        }
    };
    let took = started.elapsed();
    let grew = sampled.grew_mib();
    let met = grew <= 64.0;
    println!(
        "{}",
        json!({ "row": row, "measure": measure, "record_mib": bytes as f64 / f64::from(1 << 20), "seconds": took.as_secs_f64(),
                "grew_mib": grew, "events": events, "problems": problems, "target_mib": 64.0, "met": met })
    );
    Ok(met)
}

/// `claude-place` and `claude-place-kept`: a record of small rows placed in
/// the timeline as the node places a Claude resident's history with no
/// window to follow (`Follower::claude_place`) — held whole and written, or
/// read into a work store and written from there (§28.51 ⑤-4) — the whole
/// way to the library, with this process's resident memory as it goes.
pub(crate) fn place(dir: &Path, scale: Scale) -> Result<bool, String> {
    let rounds = match scale {
        Scale::Full => 25_000,
        Scale::Small => 2_500,
    };
    let (path, bytes) = record(&dir.join("claude"), rounds, 0, 0)?;
    let began = jiff::Timestamp::from_second(1_800_000_000).map_err(error)?;
    let mut met = true;
    for (row, kept) in [("claude-place", false), ("claude-place-kept", true)] {
        let three = crate::storage::three(&dir.join(row))?;
        let follower = fastctx_node::bench::Follower::new(three.producer.clone(), three.device.clone(), "claude-place")?;
        let sampled = Sampled::start()?;
        let started = Instant::now();
        let written = follower.claude_place(&path, began, kept.then(|| dir.join(row).join("spill")))?;
        let took = started.elapsed();
        let grew = sampled.grew_mib();
        met &= !kept || grew <= 64.0;
        let phases: serde_json::Map<String, serde_json::Value> =
            if kept { follower.phases().into_iter().map(|(name, seconds)| (name.to_string(), json!(seconds))).collect() } else { serde_json::Map::new() };
        println!(
            "{}",
            json!({ "row": row, "measure": "a record of small rows placed in the timeline whole, written to the library",
                    "record_mib": bytes as f64 / f64::from(1 << 20), "seconds": took.as_secs_f64(), "grew_mib": grew, "written": written,
                    "phases": phases, "target_mib": 64.0, "met": !kept || grew <= 64.0 })
        );
    }
    Ok(met)
}

/// `claude-place-large`: as `claude-place-kept`, a record of about a
/// million rows (a tenth at the small scale) — read into a work store and
/// written from there; held whole it would take gigabytes, and is not run.
pub(crate) fn place_large(dir: &Path, scale: Scale) -> Result<bool, String> {
    let rounds = match scale {
        Scale::Full => 250_000,
        Scale::Small => 25_000,
    };
    let (path, bytes) = record(&dir.join("claude"), rounds, 0, 0)?;
    let began = jiff::Timestamp::from_second(1_800_000_000).map_err(error)?;
    let three = crate::storage::three(&dir.join("large"))?;
    let follower = fastctx_node::bench::Follower::new(three.producer.clone(), three.device.clone(), "claude-place")?;
    let sampled = Sampled::start()?;
    let started = Instant::now();
    let written = follower.claude_place(&path, began, Some(dir.join("large").join("spill")))?;
    let took = started.elapsed();
    let grew = sampled.grew_mib();
    let phases: serde_json::Map<String, serde_json::Value> = follower.phases().into_iter().map(|(name, seconds)| (name.to_string(), json!(seconds))).collect();
    println!(
        "{}",
        json!({ "row": "claude-place-large", "measure": "a record of a million small rows placed in the timeline, read into a work store",
                "record_mib": bytes as f64 / f64::from(1 << 20), "seconds": took.as_secs_f64(), "grew_mib": grew, "written": written,
                "phases": phases, "target_mib": 64.0, "met": grew <= 64.0 })
    );
    Ok(grew <= 64.0)
}

/// `claude-follow` and `claude-follow-kept`: the record of `claude-place`
/// followed from its start as the node follows a resident's record read
/// whole (`Follower::claude_follow`) — held whole and written, or read into
/// a work store and written from there (§28.51 ⑤-5) — the whole way to the
/// library; `claude-follow-large`, a record of about a million rows, only
/// read into a work store.
pub(crate) fn follow(dir: &Path, scale: Scale) -> Result<bool, String> {
    let (rounds, large) = match scale {
        Scale::Full => (25_000, 250_000),
        Scale::Small => (2_500, 25_000),
    };
    let began = jiff::Timestamp::from_second(1_800_000_000).map_err(error)?;
    let mut met = true;
    for (row, rounds, kept) in [("claude-follow", rounds, false), ("claude-follow-kept", rounds, true), ("claude-follow-large", large, true)] {
        let (path, bytes) = record(&dir.join(format!("claude-{rounds}")), rounds, 0, 0)?;
        let three = crate::storage::three(&dir.join(row))?;
        let follower = fastctx_node::bench::Follower::new(three.producer.clone(), three.device.clone(), "claude-follow")?;
        let sampled = Sampled::start()?;
        let started = Instant::now();
        let (written, moved) = follower.claude_follow(&path, began, kept.then(|| dir.join(row).join("spill")), &mut || Box::new(std::iter::empty()))?;
        let took = started.elapsed();
        let grew = sampled.grew_mib();
        met &= !kept || grew <= 64.0;
        let phases: serde_json::Map<String, serde_json::Value> =
            if kept { follower.phases().into_iter().map(|(name, seconds)| (name.to_string(), json!(seconds))).collect() } else { serde_json::Map::new() };
        println!(
            "{}",
            json!({ "row": row, "measure": "a record of small rows followed from its start, written to the library",
                    "record_mib": bytes as f64 / f64::from(1 << 20), "seconds": took.as_secs_f64(), "grew_mib": grew, "written": written,
                    "moved_past": moved, "phases": phases, "target_mib": 64.0, "met": !kept || grew <= 64.0 })
        );
    }
    Ok(met)
}

/// `claude-follow-world` (and `-large`): a record of one long piece of work
/// (`autonomous`), a hundred thousand rows (a million), followed from its
/// start into a work store as `claude-follow-large` is — with the World's
/// record of it beside, as a hosted conversation's is (`world_of`): the
/// World's events about three for every four of the host's rows, as the
/// longest conversation measured had them. The World's record is made once
/// the sampler runs, as `produce` reads it; each part's most memory apart.
pub(crate) fn follow_world(dir: &Path, scale: Scale) -> Result<bool, String> {
    let (small, large) = match scale {
        Scale::Full => (100_000, 1_000_000),
        Scale::Small => (10_000, 100_000),
    };
    let began = jiff::Timestamp::from_second(1_800_000_000).map_err(error)?;
    let mut met = true;
    for (row, rows) in [("claude-follow-world", small), ("claude-follow-world-large", large)] {
        let (path, bytes) = autonomous(&dir.join(format!("autonomous-{rows}")), rows, 0, false)?;
        let three = crate::storage::three(&dir.join(row))?;
        let follower = fastctx_node::bench::Follower::new(three.producer.clone(), three.device.clone(), "claude-follow")?;
        let sampled = Sampled::start()?;
        let before = sampled.before();
        let started = Instant::now();
        let events = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let counted = events.clone();
        let (written, _) = follower.claude_follow(&path, began, Some(dir.join(row).join("spill")), &mut || {
            let counted = counted.clone();
            Box::new(world_of(rows).inspect(move |_| counted.set(counted.get() + 1)))
        })?;
        let events = events.get();
        let took = started.elapsed();
        let grew = sampled.grew_mib();
        met &= grew <= 64.0;
        let mib = |bytes: u64| bytes.saturating_sub(before) as f64 / f64::from(1 << 20);
        let phases: serde_json::Map<String, serde_json::Value> = follower.phases().into_iter().map(|(name, seconds)| (name.to_string(), json!(seconds))).collect();
        let peaks: serde_json::Map<String, serde_json::Value> = follower.phase_peaks().into_iter().map(|(name, most)| (name.to_string(), json!(mib(most)))).collect();
        println!(
            "{}",
            json!({ "row": row, "measure": "one long piece of work followed from its start with the World's record of it, written to the library",
                    "record_mib": bytes as f64 / f64::from(1 << 20), "world_events": events, "seconds": took.as_secs_f64(), "grew_mib": grew,
                    "written": written, "phases": phases, "phase_peak_mib": peaks, "target_mib": 64.0, "met": grew <= 64.0 })
        );
    }
    Ok(met)
}

/// `claude-world-alone`: the World's record of a million rows' work
/// (`world_of`, about 750,000 events) written as the history of a
/// conversation whose host is not read (§28.51 ⑥-4) — read into a store as
/// it is made, ordered there, written a batch at a time — with each part's
/// most memory apart.
pub(crate) fn world_alone(dir: &Path, scale: Scale) -> Result<bool, String> {
    let rows = match scale {
        Scale::Full => 1_000_000,
        Scale::Small => 100_000,
    };
    let row = "claude-world-alone";
    let three = crate::storage::three(&dir.join(row))?;
    let follower = fastctx_node::bench::Follower::new(three.producer.clone(), three.device.clone(), "claude-follow")?;
    let sampled = Sampled::start()?;
    let before = sampled.before();
    let started = Instant::now();
    let written = follower.world_alone(Box::new(world_of(rows)), dir.join(row).join("spill"))?;
    let took = started.elapsed();
    let grew = sampled.grew_mib();
    let mib = |bytes: u64| bytes.saturating_sub(before) as f64 / f64::from(1 << 20);
    let phases: serde_json::Map<String, serde_json::Value> = follower.phases().into_iter().map(|(name, seconds)| (name.to_string(), json!(seconds))).collect();
    let peaks: serde_json::Map<String, serde_json::Value> = follower.phase_peaks().into_iter().map(|(name, most)| (name.to_string(), json!(mib(most)))).collect();
    println!(
        "{}",
        json!({ "row": row, "measure": "the World's record alone of a long piece of work written as a history whose host is not read",
                "seconds": took.as_secs_f64(), "grew_mib": grew, "written": written, "phases": phases, "phase_peak_mib": peaks,
                "target_mib": 64.0, "met": grew <= 64.0 })
    );
    Ok(grew <= 64.0)
}

/// The World's record of `autonomous`'s `rows` as a hosted conversation's
/// World keeps it, made as it is read: the turn's start and the words that
/// began it, then for every two tool calls a model request, its answer and
/// the usage told after it (with no turn, as the World tells usage), a
/// context reading every hundred calls, and the turn's end after the last.
fn world_of(rows: usize) -> impl Iterator<Item = fastctx_types::session::SessionEvent> + 'static {
    fn at(millis: i64) -> String {
        (jiff::Timestamp::from_second(1_800_000_000).expect("a time") + jiff::SignedDuration::from_millis(millis)).to_string()
    }
    fn event(value: serde_json::Value) -> fastctx_types::session::SessionEvent {
        serde_json::from_value(value).expect("an event of the World's record")
    }
    const TURN: &str = "turn-bench";
    let begun = [
        json!({"at": at(-2), "event": "turn_started", "turn": TURN}),
        json!({"at": at(-1), "event": "heard", "delivery": "talk-bench", "turn": TURN, "from": {"kind": "user", "machine": "bench"}, "text": "work through it"}),
    ];
    let steps = (0..rows).step_by(4).enumerate().flat_map(|(step, row)| {
        let call = 1_000 + row as i64 * 500;
        let mut values = vec![
            json!({"at": at(call - 400), "event": "changed", "turn": TURN,
                   "change": {"kind": "model_request", "step": step, "model": "claude-bench", "effort": "high"}}),
            json!({"at": at(call - 1), "event": "changed", "turn": TURN,
                   "change": {"kind": "model_response", "step": step, "usage": {"model": "claude-bench", "input_tokens": 2,
                              "output_tokens": 238, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 39437}}}),
            json!({"at": at(call + 1), "event": "changed", "turn": null, "change": {"kind": "usage", "usage": {}}}),
        ];
        if step % 50 == 49 {
            values.push(json!({"at": at(call + 2), "event": "context", "used": 188_126, "window": 1_000_000}));
        }
        values
    });
    let end = 1_000 + rows as i64 * 500 + 1_000;
    let ended = json!({"at": at(end), "event": "turn_ended", "turn": TURN, "end": {"how": "completed", "reason": null, "duration_ms": end, "last_message": "done"}});
    begun.into_iter().chain(steps).chain(std::iter::once(ended)).map(event)
}

/// A record whose seam can never move: the user's words once, at its start,
/// then `rows` rows of a long piece of work alone — tool calls and their
/// results, each answered — as an agent working through one turn writes.
fn autonomous(dir: &Path, rows: usize, from: usize, append: bool) -> Result<(PathBuf, u64), String> {
    std::fs::create_dir_all(dir).map_err(error)?;
    let path = dir.join("record.jsonl");
    let mut file = std::io::BufWriter::new(std::fs::OpenOptions::new().create(true).append(append).write(true).truncate(!append).open(&path).map_err(error)?);
    let base = jiff::Timestamp::from_second(1_800_000_000).map_err(error)?;
    let mut bytes = 0u64;
    let mut write = |row: serde_json::Value, file: &mut std::io::BufWriter<std::fs::File>| -> Result<(), String> {
        let mut line = serde_json::to_vec(&row).map_err(error)?;
        line.push(b'\n');
        bytes += line.len() as u64;
        file.write_all(&line).map_err(error)
    };
    if from == 0 {
        write(json!({"type": "user", "uuid": "start", "parentUuid": null, "sessionId": "bench", "timestamp": base.to_string(),
                     "message": {"role": "user", "content": "work through it"}}), &mut file)?;
    }
    for row in from..from + rows {
        let at = (base + jiff::SignedDuration::from_millis(1_000 + row as i64 * 500)).to_string();
        let call = format!("toolu_{}", row / 2);
        let value = if row % 2 == 0 {
            json!({"type": "assistant", "uuid": format!("a{row}"), "sessionId": "bench", "timestamp": at,
                   "message": {"id": format!("msg_{row}"), "role": "assistant", "model": "claude-bench",
                               "content": [{"type": "tool_use", "id": call, "name": "Bash", "input": {"command": "ls"}}]}})
        } else {
            json!({"type": "user", "uuid": format!("r{row}"), "sessionId": "bench", "timestamp": at,
                   "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": call, "content": "out"}]}})
        };
        write(value, &mut file)?;
    }
    file.flush().map_err(error)?;
    Ok((path, bytes))
}

/// `claude-follow-window`: a record of about a million rows whose seam never
/// moves (`autonomous`) followed twice into a work store (§28.51 ⑤-7): from
/// its start, then grown and read on — a window as long as the record,
/// written past the floor the first left, the items it held kept beside the
/// checkpoint. Each followed measured apart.
pub(crate) fn follow_window(dir: &Path, scale: Scale) -> Result<bool, String> {
    let rows = match scale {
        Scale::Full => 1_000_000,
        Scale::Small => 100_000,
    };
    let (path, bytes) = autonomous(&dir.join("claude"), rows, 0, false)?;
    let began = jiff::Timestamp::from_second(1_800_000_000).map_err(error)?;
    let three = crate::storage::three(&dir.join("window"))?;
    let follower = fastctx_node::bench::Follower::new(three.producer.clone(), three.device.clone(), "claude-follow")?;
    let mut first = Some(Sampled::start()?);
    let started = Instant::now();
    let mut second: Option<(Sampled, Instant, f64, f64)> = None;
    let record = dir.join("claude");
    let (written_first, written_second, window) = follower.claude_follow_twice(&path, began, dir.join("window").join("spill"), &mut || {
        let (grew, took) = (first.take().ok_or("grown twice")?.grew_mib(), started.elapsed().as_secs_f64());
        autonomous(&record, 20, rows, true)?;
        second = Some((Sampled::start()?, Instant::now(), grew, took));
        Ok(())
    })?;
    let (sampled, began_second, grew_first, took_first) = second.ok_or("the record was never grown")?;
    let (grew_second, took_second) = (sampled.grew_mib(), began_second.elapsed().as_secs_f64());
    let met = grew_first <= 64.0 && grew_second <= 64.0 && window;
    println!(
        "{}",
        json!({ "row": "claude-follow-window", "measure": "a record whose seam never moves followed from its start, then read on as a window as long as itself",
                "record_mib": bytes as f64 / f64::from(1 << 20), "first_seconds": took_first, "first_grew_mib": grew_first, "first_written": written_first,
                "second_seconds": took_second, "second_grew_mib": grew_second, "second_written": written_second, "window": window,
                "target_mib": 64.0, "met": met })
    );
    Ok(met)
}

/// `claude-follow-on`: the record of `claude-follow-window` followed from
/// its start, then grown a tool call and its result at a time and its
/// timeline written on from the place it was read through at each of many
/// looks (design §10: what did not change is not read again): each look
/// reads the rows past that place and writes what they show, whatever the
/// length of the window before it. Each look measured; the slowest is to
/// take no longer than `target_seconds`.
pub(crate) fn follow_on(dir: &Path, scale: Scale) -> Result<bool, String> {
    let rows = match scale {
        Scale::Full => 1_000_000,
        Scale::Small => 100_000,
    };
    const LOOKS: usize = 200;
    const TARGET_SECONDS: f64 = 0.25;
    let (path, bytes) = autonomous(&dir.join("claude"), rows, 0, false)?;
    let began = jiff::Timestamp::from_second(1_800_000_000).map_err(error)?;
    let three = crate::storage::three(&dir.join("window"))?;
    let follower = fastctx_node::bench::Follower::new(three.producer.clone(), three.device.clone(), "claude-follow")?;
    let record = dir.join("claude");
    let started = Instant::now();
    let mut first_seconds = None;
    let (written_first, looks) = follower.claude_follow_on(&path, began, dir.join("window").join("spill"), LOOKS, &mut |look| {
        first_seconds.get_or_insert_with(|| started.elapsed().as_secs_f64());
        autonomous(&record, 2, rows + look * 2, true).map(|_| ())
    })?;
    let mut seconds: Vec<f64> = looks.iter().map(|(seconds, _)| *seconds).collect();
    seconds.sort_by(f64::total_cmp);
    let (median, slowest) = (seconds[seconds.len() / 2], seconds[seconds.len() - 1]);
    let written: usize = looks.iter().map(|(_, written)| *written).sum();
    let met = slowest <= TARGET_SECONDS && written >= LOOKS;
    println!(
        "{}",
        json!({ "row": "claude-follow-on", "measure": "a record whose seam never moves followed from its start, then written on from inside the turn as it grows a call and its result at a time",
                "record_mib": bytes as f64 / f64::from(1 << 20), "first_seconds": first_seconds, "first_written": written_first,
                "looks": LOOKS, "look_median_seconds": median, "look_slowest_seconds": slowest, "looks_written": written,
                "target_seconds": TARGET_SECONDS, "met": met })
    );
    Ok(met)
}

/// `claude-place-stopped`: the record of `claude-place-large` placed whole,
/// then grown by a line larger than World reads (§28.49) and placed again:
/// the reading stops before it, and what the record shows is added only
/// where the timeline has none of it (`Span::Added`), decided in the work
/// store. Only the second placing is measured.
pub(crate) fn place_stopped(dir: &Path, scale: Scale) -> Result<bool, String> {
    let rounds = match scale {
        Scale::Full => 250_000,
        Scale::Small => 25_000,
    };
    let (path, bytes) = record(&dir.join("claude"), rounds, 0, 0)?;
    let began = jiff::Timestamp::from_second(1_800_000_000).map_err(error)?;
    let three = crate::storage::three(&dir.join("stopped"))?;
    let follower = fastctx_node::bench::Follower::new(three.producer.clone(), three.device.clone(), "claude-place")?;
    let first = follower.claude_place(&path, began, Some(dir.join("stopped").join("spill")))?;
    // A line of more values than World reads, at the record's end.
    {
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).map_err(error)?;
        let values = vec![0u8; fastctx_hosts::bounded::VALUES_MOST + 1];
        let row = json!({"type": "user", "uuid": "too-many", "sessionId": "bench", "timestamp": "2027-01-01T00:00:00Z",
                         "message": {"role": "user", "content": [{"type": "text", "text": "x"}]}, "values": values});
        let mut line = serde_json::to_vec(&row).map_err(error)?;
        line.push(b'\n');
        file.write_all(&line).map_err(error)?;
    }
    let sampled = Sampled::start()?;
    let started = Instant::now();
    let written = follower.claude_place(&path, began, Some(dir.join("stopped").join("spill")))?;
    let took = started.elapsed();
    let grew = sampled.grew_mib();
    let phases: serde_json::Map<String, serde_json::Value> = follower.phases().into_iter().map(|(name, seconds)| (name.to_string(), json!(seconds))).collect();
    println!(
        "{}",
        json!({ "row": "claude-place-stopped", "measure": "a record of a million small rows stopped short, added to the timeline that had it, read into a work store",
                "record_mib": bytes as f64 / f64::from(1 << 20), "seconds": took.as_secs_f64(), "grew_mib": grew, "first_written": first, "written": written,
                "phases": phases, "target_mib": 64.0, "met": grew <= 64.0 })
    );
    Ok(grew <= 64.0)
}

pub(crate) fn relist(dir: &Path, scale: Scale) -> Result<bool, String> {
    let rounds = match scale {
        Scale::Full => 250_000,
        Scale::Small => 25_000,
    };
    let (path, bytes) = record(&dir.join("claude"), rounds, 0, 0)?;
    let mut met = true;
    for (row, way) in [("claude-relist", Way::Whole), ("claude-relist-history", Way::History), ("claude-relist-kept", Way::Kept)] {
        met &= read(row, "a record of small rows read whole as a session's history", &path, bytes, way)?;
    }
    Ok(met)
}

pub(crate) fn relist_large(dir: &Path, scale: Scale) -> Result<bool, String> {
    let rounds = match scale {
        Scale::Full => 20_000,
        Scale::Small => 2_000,
    };
    let (path, bytes) = record(&dir.join("claude"), rounds, 64 << 10, rounds / 2_000)?;
    let mut met = true;
    for (row, way) in [("claude-relist-large", Way::Whole), ("claude-relist-large-history", Way::History), ("claude-relist-large-kept", Way::Kept)] {
        met &= read(row, "a record of 2,000 tool results of 64 KiB among small rows, read whole", &path, bytes, way)?;
    }
    Ok(met)
}
