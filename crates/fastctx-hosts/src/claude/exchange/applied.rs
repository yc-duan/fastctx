//! Which of a session's reports the node has applied.
//!
//! Applying a report leaves an empty marker of its name in `applied/`, so a
//! report whose file appears again is not applied twice and a report that
//! names earlier ones waits for them. The mod cannot delete what it writes
//! and the node kept every marker, so a long conversation left hundreds of
//! thousands of empty files. `applied.json` names one report through which
//! every report is settled, and the markers up to it go.
//!
//! A report's name is the mod's clock and count at a fixed width, so names
//! order the reports as they were named. A later name does not settle two
//! kinds of report. One set aside in `damaged/` may still be applied by a
//! newer decoder, so it is judged by its own marker, which is kept. One the
//! mod has named but not finished writing may still arrive, so `through`
//! moves only as far as the mod has vouched for, or to the end once the
//! mod's process is gone.

use super::Registration;
use fastctx_platform::{Permissions, publish};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io, path::Path};

#[derive(Deserialize, Serialize)]
struct Settled {
    through: String,
}

/// How far the markers of a session may be let go.
pub enum Vouched<'a> {
    /// The session's mod said every report it named before this one is whole
    /// on disk.
    Before(&'a str),
    /// The process this registration names has ended: nothing more is written
    /// under the names already there.
    Ended(&'a Registration),
}

const STEM: usize = 22;

/// Whether `stem` is a clock and a count at the width the mod writes them.
fn ordered_stem(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    bytes.len() == STEM
        && bytes[..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'-'
        && bytes[16..].iter().all(u8::is_ascii_digit)
}

/// Whether `name` is a report's file name whose text gives its order.
pub(super) fn ordered(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(ordered_stem)
}

fn through(directory: &Path) -> io::Result<Option<String>> {
    let bytes = match std::fs::read(directory.join("applied.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let settled: Settled = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("applied.json: {error}")))?;
    if !ordered(&settled.through) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "applied.json names no report"));
    }
    Ok(Some(settled.through))
}

/// Whether the report `name` was applied. The marker is looked for first and
/// `applied.json` read after it, the order `compact` writes them in, so a
/// marker being let go is never taken for a report not applied.
pub(super) fn has(directory: &Path, name: &str) -> io::Result<bool> {
    if directory.join("applied").join(name).try_exists()? {
        return Ok(true);
    }
    if !ordered(name) {
        return Ok(false);
    }
    match through(directory)? {
        Some(through) if name <= through.as_str() => Ok(!directory.join("damaged").join(name).try_exists()?),
        _ => Ok(false),
    }
}

fn names(directory: &Path) -> io::Result<Vec<String>> {
    match std::fs::read_dir(directory) {
        Ok(entries) => {
            entries.map(|entry| Ok(entry?.file_name().into_string().ok())).filter_map(Result::transpose).collect()
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// The report the mod is waiting to see applied before it hands anything
/// more over (`goal.js`): it looks for that very marker.
fn awaited(directory: &Path) -> io::Result<Result<Option<String>, ()>> {
    #[derive(Deserialize)]
    struct Stop {
        report: String,
    }
    match std::fs::read(directory.join("goal-stop.json")) {
        // The mod's file interface empties a file before it writes it.
        Ok(bytes) => Ok(serde_json::from_slice::<Stop>(&bytes).map(|stop| Some(stop.report)).map_err(|_| ())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Ok(None)),
        Err(error) => Err(error),
    }
}

/// Moves `applied.json` as far as `vouched` and what still waits on disk
/// allow, then removes the markers it stands for and the allocator
/// watermarks (`.sequence`) below the newest name. The newest name in
/// `applied/` always stays: a starting mod recovers its clock and count from
/// it. Returns how many files went.
pub fn compact(directory: &Path, vouched: Vouched<'_>) -> io::Result<usize> {
    let mut limit = match vouched {
        Vouched::Before(name) if ordered(name) => Some(name.to_string()),
        Vouched::Before(_) => return Ok(0),
        Vouched::Ended(_) => None,
    };
    // A report still waiting to be applied, whole or not, is not settled, nor
    // is any named after it.
    for waiting in ["events", "accounting"] {
        for name in names(&directory.join(waiting))? {
            if !name.ends_with(".json") {
                continue;
            }
            if !ordered(&name) {
                return Ok(0);
            }
            if !has(directory, &name)? && limit.as_deref().is_none_or(|limit| name.as_str() < limit) {
                limit = Some(name);
            }
        }
    }
    let entries = names(&directory.join("applied"))?;
    let mut markers = Vec::new();
    let mut watermarks = Vec::new();
    for name in &entries {
        if ordered(name) {
            markers.push(name.as_str());
        } else if name.strip_suffix(".sequence").is_some_and(ordered_stem) {
            watermarks.push(name.as_str());
        }
    }
    let Some(newest) = markers.iter().chain(&watermarks).map(|name| &name[..STEM]).max() else {
        return Ok(0);
    };
    if let Vouched::Ended(writer) = vouched
        && super::registration(directory)?.as_ref() != Some(writer)
    {
        // Another process registered meanwhile; what it has named but not
        // written is not known here.
        return Ok(0);
    }
    let known = through(directory)?;
    let reached = markers.iter().copied().filter(|marker| limit.as_deref().is_none_or(|limit| *marker < limit)).max();
    let through = match (known, reached) {
        (known, Some(reached)) if known.as_deref().is_none_or(|known| reached > known) => {
            let settled = serde_json::to_vec(&Settled { through: reached.to_string() }).map_err(io::Error::other)?;
            publish(&directory.join("applied.json"), &settled, Permissions::Private)?;
            Some(reached.to_string())
        }
        (known, _) => known,
    };
    let Ok(awaited) = awaited(directory)? else {
        return Ok(0);
    };
    let damaged: HashSet<String> = names(&directory.join("damaged"))?.into_iter().collect();
    let applied = directory.join("applied");
    let mut removed = 0;
    let settled = markers.into_iter().filter(|marker| {
        through.as_deref().is_some_and(|through| *marker <= through)
            && awaited.as_deref() != Some(*marker)
            && !damaged.contains(*marker)
    });
    for name in settled.chain(watermarks).filter(|name| &name[..STEM] < newest) {
        match std::fs::remove_file(applied.join(name)) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(removed)
}
