//! A fact update is a bounded sequence of independently acknowledged letters.
//! Member identity is unchanged; only the final frame publishes completion.
use fastctx_types::run_facts::{JobFacts, OutputNoteMatch};
use serde::Serialize;
use std::io;

static OMITTED_NOTES: std::sync::LazyLock<&'static fastctx_platform::readings::Counter> =
    std::sync::LazyLock::new(|| fastctx_platform::readings::counter("node.facts.notes.folded"));
static CLIPPED_SCRIPTS: std::sync::LazyLock<&'static fastctx_platform::readings::Counter> =
    std::sync::LazyLock::new(|| fastctx_platform::readings::counter("node.facts.scripts.clipped"));

const TARGET: usize = 1024 * 1024;
const MAXIMUM: usize = 12 * 1024 * 1024;
const SCRIPT: usize = 128 * 1024;
const NOTES: usize = 4096;
const NOTE_BYTES: usize = 1024 * 1024;

/// Notes are a display index, not the capture. Keep its recent bounded suffix;
/// complete output and exact writer notes remain on the executor.
pub(super) fn notes(notes: &mut Vec<OutputNoteMatch>) -> io::Result<()> {
    let mut bytes = 2;
    let mut keep = 0;
    for note in notes.iter().rev().take(NOTES) {
        bytes += serde_json::to_vec(note)?.len() + 1;
        if bytes > NOTE_BYTES {
            break;
        }
        keep += 1;
    }
    OMITTED_NOTES.add((notes.len() - keep) as u64);
    notes.drain(..notes.len() - keep);
    Ok(())
}

pub(super) fn split(mut update: JobFacts) -> io::Result<Vec<JobFacts>> {
    if let Some(script) = &mut update.script
        && script.len() > SCRIPT
    {
        let mut end = SCRIPT;
        while !script.is_char_boundary(end) {
            end -= 1;
        }
        CLIPPED_SCRIPTS.add(1);
        script.truncate(end);
        script.push_str("\n… (complete script in the job record)");
    }
    if let Some(notes) = &mut update.notes {
        self::notes(notes)?;
    }
    let writes = std::mem::take(&mut update.writes);
    let steps = std::mem::take(&mut update.steps);
    let visited = std::mem::take(&mut update.visited);
    let mut frame = update.clone();
    frame.state = None;
    frame.finished_at = None;
    frame.output = None;
    frame.notes = None;
    // Published receivers replace the whole job snapshot at each sequence.
    // Repeat its small source fields so an intermediate or final frame cannot
    // erase the source information they received with the first one.
    let empty = frame.clone();
    let mut frames = Vec::new();
    let mut bytes = serde_json::to_vec(&frame)?.len();
    let base = serde_json::to_vec(&empty)?.len();
    if base > MAXIMUM / 2 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "job fact metadata exceeds its frame budget"));
    }
    fn add<T: Serialize>(
        item: T,
        frames: &mut Vec<JobFacts>,
        frame: &mut JobFacts,
        empty: &JobFacts,
        bytes: &mut usize,
        base: usize,
        put: impl FnOnce(&mut JobFacts, T),
    ) -> io::Result<()> {
        let size = serde_json::to_vec(&item)?.len() + 1;
        if size + base > MAXIMUM {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "one job fact exceeds its 12 MiB frame budget"));
        }
        if *bytes > base && *bytes + size > base + TARGET {
            frames.push(std::mem::replace(frame, empty.clone()));
            *bytes = base;
        }
        put(frame, item);
        *bytes += size;
        Ok(())
    }
    for item in writes {
        add(item, &mut frames, &mut frame, &empty, &mut bytes, base, |frame, item| frame.writes.push(item))?;
    }
    for item in steps {
        add(item, &mut frames, &mut frame, &empty, &mut bytes, base, |frame, item| frame.steps.push(item))?;
    }
    for item in visited {
        add(item, &mut frames, &mut frame, &empty, &mut bytes, base, |frame, item| frame.visited.push(item))?;
    }
    if frame != empty {
        frames.push(frame);
    }

    frames.push(update);
    for frame in &frames {
        // Include headroom for the letter envelope, maximum sequence and escaping.
        if serde_json::to_vec(frame)?.len() > MAXIMUM {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "job fact frame exceeds its budget"));
        }
    }
    Ok(frames)
}
