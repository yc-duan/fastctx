//! Writing files for a resident (`world-write.md`).
//!
//! The writing core is handed what the resident has seen of files on this
//! machine, from its `seen` record here, so a whole-file write never wipes
//! out a change it has not seen. After a write, what the resident wrote is
//! what it knows of those files: the new contents go on its `seen` record,
//! so writing the same file again is not refused as a change it missed.

use std::collections::BTreeMap;
use std::io::{self, BufRead as _};
use std::path::{Path, PathBuf};

use fastctx_hands::files::canonical_path;
use fastctx_hands::write::Writer;
use fastctx_types::files::{SeenFile, SeenFiles, SeenVersion};
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{Action, Seen};
use fastctx_types::write::{FileChange, WriteOp, WriteOutcome};
use fastctx_types::{Address, MachineName, TownName};
use jiff::Timestamp;

/// Does `op` on this machine for `owner`.
pub fn write_here(
    world: &WorldDir,
    here: &fastctx_types::layout::Names,
    owner: &Address,
    town: Option<&TownName>,
    op: WriteOp,
) -> io::Result<WriteOutcome> {
    write_job(world, here, owner, town, op, None)
}

pub(crate) fn write_job(
    world: &WorldDir,
    here: &fastctx_types::layout::Names,
    owner: &Address,
    town: Option<&TownName>,
    op: WriteOp,
    job: Option<&fastctx_types::JobId>,
) -> io::Result<WriteOutcome> {
    let directory = world.owner(here, owner);
    let seen = load_seen(&directory.seen())?;
    let mut writer = Writer::new(world, here.me.as_str())?;
    let mut turn = None;
    // A job an older program still runs keeps its record where that program
    // put it, and its scratch beside it.
    let record_path = job.map(|job| world.job_record(here, town, &owner.clone().into(), job));
    let job_directory =
        record_path.as_ref().and_then(|path| path.parent()?.parent()).map(fastctx_types::layout::ConversationDir::at);
    if let (Some(job), Some(path), Some(job_directory)) = (job, &record_path, &job_directory) {
        let record: fastctx_types::run::JobRecord = serde_json::from_slice(&std::fs::read(path)?)?;
        if record.owner.resident() != Some(owner) || record.end.is_some() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "write does not belong to an active job"));
        }
        turn = record.turn;
        if record.call_id.is_some() {
            writer = writer.with_facts(job_directory.job_scratch(job).join("diffs"), here.me.clone())?;
        }
    }
    let mut outcome = match op {
        WriteOp::Edit(request) => writer.edit(request, &seen),
        WriteOp::EditBlocks(request) => writer.edit_blocks(request, &seen),
        WriteOp::Write(request) => writer.write(request, &seen),
        WriteOp::ApplyPatch(request) => writer.apply_patch(request, &seen),
    };
    let changes = match &mut outcome {
        Ok(result) => &mut result.changes,
        Err(error) => &mut error.changes,
    };
    for change in changes.iter_mut() {
        if let Some(fact) = &mut change.fact {
            fact.turn = turn.clone();
        }
    }
    if let Some(path) = &record_path {
        for change in changes.iter() {
            if let Some(fact) = &change.fact {
                crate::run_facts::write(&path.with_extension("writes"), fact)?;
            }
        }
    }
    let written: Vec<Seen> =
        changes.iter().map(seen_after).collect::<io::Result<Vec<_>>>()?.into_iter().flatten().collect();
    if !written.is_empty() {
        crate::residents::record_seen(world, here, owner, &written)?;
    }
    Ok(outcome)
}

/// The latest of what a resident saw of each file, by the file's canonical
/// path, as the writing core looks files up.
pub(crate) fn load_seen(path: &Path) -> io::Result<SeenFiles> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(SeenFiles::new()),
        Err(error) => return Err(error),
    };
    let mut latest: BTreeMap<PathBuf, SeenFile> = BTreeMap::new();
    for line in io::BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let seen: Seen = serde_json::from_str(&line)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?;
        // A file seen and since removed has no identity to keep.
        let Ok(canonical) = canonical_path(Path::new(&seen.path)) else { continue };
        let version = match (seen.sha256, seen.modified) {
            (Some(sha256), _) => SeenVersion::Full { sha256 },
            (None, Some(modified)) => SeenVersion::Partial { size: seen.size, modified: modified.into() },
            (None, None) => continue,
        };
        latest.insert(canonical, SeenFile { version, encoding: seen.encoding_selection });
    }
    Ok(latest)
}

/// What the resident knows of a file after writing it: its new contents,
/// in the encoding the writer used, even if an external write changed it.
fn seen_after(change: &FileChange) -> io::Result<Option<Seen>> {
    let Some(sha256) = change.after_hash.clone() else { return Ok(None) };
    let path = canonical_path(&change.path)?;
    let size = match std::fs::metadata(&path) {
        Ok(metadata) => metadata.len(),
        // A later part of the same patch may have deleted this path.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(Seen {
        path: path.to_string_lossy().into_owned(),
        at: Timestamp::now(),
        sha256: Some(sha256),
        size,
        modified: None,
        encoding: change.encoding.clone(),
        encoding_selection: change.encoding_selection.clone(),
    }))
}

/// A write as the resident's actions record it.
pub fn action(op: &WriteOp, outcome: &WriteOutcome, machine: &MachineName, at: Timestamp) -> Action {
    let (changes, failure) = match outcome {
        Ok(result) => (&result.changes, None),
        Err(error) => (&error.changes, Some(&error.message)),
    };
    let mut paths: Vec<String> = changes.iter().map(|change| change.path.to_string_lossy().into_owned()).collect();
    paths.dedup();
    let what = match op {
        WriteOp::Edit(request) => request.file_path.to_string_lossy().into_owned(),
        WriteOp::EditBlocks(request) => request.file_path.to_string_lossy().into_owned(),
        WriteOp::Write(request) => request.file_path.to_string_lossy().into_owned(),
        WriteOp::ApplyPatch(_) if paths.is_empty() => "a patch".to_string(),
        WriteOp::ApplyPatch(_) => paths.join(", "),
    };
    let counted = match changes.len() {
        1 => "1 change".to_string(),
        n => format!("{n} changes"),
    };
    let result = match failure {
        None => counted,
        Some(message) => {
            // The message's first line is the head note; the reason follows.
            let reason = message.lines().nth(1).unwrap_or(message.as_str());
            format!("failed after {counted}: {reason}")
        }
    };
    Action { turn: None, at, machine: machine.clone(), hand: op.tool().to_string(), what, result }
}
