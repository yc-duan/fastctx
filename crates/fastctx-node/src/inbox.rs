//! What waits to be told to a resident.
//!
//! When a `run` call answers before its job has ended, the resident is owed
//! word of the end: a marker beside the job's record says so. When the job
//! ends, the word goes into the resident's inbox, a file named for the job,
//! so telling it twice writes nothing new; then the marker goes. A node that
//! stopped in between finds the marker when it starts and finishes the
//! telling. Whatever carries words to the resident takes them from the
//! inbox; each is handed over once.

use std::io;
use std::path::Path;

use fastctx_hands::run::record;
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::layout::{ResidentDir, WorldDir};
use fastctx_types::residents::Notice;
use fastctx_types::run::JobRecord;
use fastctx_types::{JobId, ResidentId, TownName};

/// Marks that the resident who started `job` is owed word of its end, and
/// tells at once if it has already ended.
pub fn owe(world: &WorldDir, job: &JobId, record_path: &Path) -> io::Result<()> {
    let job_record = record::read(record_path)?;
    let resident = world.resident(job_record.town.as_ref(), &job_record.resident);
    match publish_new(&resident.job_owed(job), b"", Permissions::Private) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    // It may have ended between the answer and the marker; nobody else
    // would tell then.
    tell_if_ended(world, &record::read(record_path)?)
}

/// Tells the resident that started the job of `record` that it ended, if it
/// has and the resident is owed that word.
pub fn tell_if_ended(world: &WorldDir, record: &JobRecord) -> io::Result<()> {
    let Some(end) = &record.end else { return Ok(()) };
    let resident = world.resident(record.town.as_ref(), &record.resident);
    let owed = resident.job_owed(&record.id);
    if !owed.exists() {
        return Ok(());
    }
    let notice = Notice::JobEnded {
        job: record.id.clone(),
        machine: record.machine.clone(),
        how: end.how.clone(),
        status: record.exit.map(|exit| exit.status),
        at: end.at,
        output: record.output.clone(),
    };
    put(&resident, &format!("{}.json", record.id), &notice)?;
    match std::fs::remove_file(&owed) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Finishes telling what a node that stopped left owed.
pub fn settle(world: &WorldDir) -> io::Result<Vec<(std::path::PathBuf, io::Error)>> {
    let mut problems = Vec::new();
    for parent in world.resident_parents()? {
        for resident in entries(&parent)? {
            for path in entries(&resident.join("jobs"))? {
                if path.extension().is_none_or(|extension| extension != "owed") {
                    continue;
                }
                let told = record::read(&path.with_extension("json")).and_then(|job| tell_if_ended(world, &job));
                if let Err(error) = told {
                    problems.push((path, error));
                }
            }
        }
    }
    Ok(problems)
}

/// Takes everything in a resident's inbox, oldest first.
pub fn collect(world: &WorldDir, resident: &ResidentId, town: Option<&TownName>) -> io::Result<Vec<Notice>> {
    let inbox = world.resident(town, resident).inbox();
    let mut notices = Vec::new();
    for path in entries(&inbox)? {
        // A word being written sits under a hidden temporary name until it
        // is whole.
        let hidden = path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden || path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let notice: Notice = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?;
        std::fs::remove_file(&path)?;
        notices.push(notice);
    }
    notices.sort_by_key(|notice| match notice {
        Notice::JobEnded { at, .. } => *at,
    });
    Ok(notices)
}

fn put(resident: &ResidentDir, name: &str, notice: &Notice) -> io::Result<()> {
    ensure_private_dir(&resident.inbox())?;
    let mut bytes = serde_json::to_vec_pretty(notice).map_err(io::Error::other)?;
    bytes.push(b'\n');
    match publish_new(&resident.inbox().join(name), &bytes, Permissions::Private) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

fn entries(directory: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    match std::fs::read_dir(directory) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
