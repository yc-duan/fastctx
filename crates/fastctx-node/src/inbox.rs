//! What waits to be told to a resident, and the end of a job owed to the
//! resident who started it.
//!
//! When a `run` call answers before its job has ended, the resident is owed
//! word of the end: a marker beside the job's record says so. When the job
//! ends, the word goes to the resident (`crate::speech::tell`), and only
//! then does the marker go; a node that stopped in between finds the marker
//! when it starts and finishes the telling.
//!
//! A resident whose host gives the World no way in finds what waits for it
//! in its inbox, one file per notice, named by the notice's id: telling the
//! same job's end twice writes nothing new. The World's tools hand the
//! inbox over with their next answer, each notice once.

use std::io;
use std::path::Path;
use std::sync::Arc;

use fastctx_hands::run::record;
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::run::JobRecord;
use fastctx_types::{JobId, MachineName, NoticeId, ResidentId, TownName};

use crate::Node;
use crate::speech::{Owed, tell};

/// Marks that the resident who started `job` is owed word of its end.
/// Returns the job's record as it stands after the marker is there: if it
/// has ended by then, nobody else will tell, and the caller does.
pub fn owe(world: &WorldDir, here: &MachineName, job: &JobId, record_path: &Path) -> io::Result<JobRecord> {
    let job_record = record::read(record_path)?;
    let directory = world.owner(here, job_record.town.as_ref(), &job_record.owner);
    match publish_new(&directory.job_owed(job), b"", Permissions::Private) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    record::read(record_path)
}

/// Tells the resident that started the job of `record` that it ended, if it
/// has and the resident is owed that word.
pub async fn tell_if_ended(node: &Arc<Node>, record: &JobRecord) -> io::Result<()> {
    let Some(end) = &record.end else {
        return Ok(());
    };
    let owed = node.world.owner(&node.machine(), record.town.as_ref(), &record.owner).job_owed(&record.id);
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
    let id = NoticeId::try_from(format!("{}-{}", record.machine, record.id)).expect("a machine and a job make an id");
    tell(node, &record.owner, id, notice, Owed::Working).await.map_err(|failure| io::Error::other(failure.message))?;
    match std::fs::remove_file(&owed) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Finishes telling what a node that stopped left owed.
pub async fn settle(node: &Arc<Node>) -> io::Result<Vec<(std::path::PathBuf, io::Error)>> {
    let mut problems = Vec::new();
    let mut owed = Vec::new();
    for parent in node.world.resident_parents()? {
        for resident in entries(&parent)? {
            for path in entries(&resident.join("jobs"))? {
                if path.extension().is_some_and(|extension| extension == "owed") {
                    owed.push(path);
                }
            }
        }
    }
    for path in owed {
        let told = match record::read(&path.with_extension("json")) {
            Ok(job) => tell_if_ended(node, &job).await,
            Err(error) => Err(error),
        };
        if let Err(error) = told {
            problems.push((path, error));
        }
    }
    Ok(problems)
}

/// Puts a notice in the inbox of a resident living on this machine.
pub fn put(world: &WorldDir, town: Option<&TownName>, resident: &ResidentId, delivery: &Delivery) -> io::Result<()> {
    let inbox = world.resident(town, resident).inbox();
    ensure_private_dir(&inbox)?;
    let mut bytes = serde_json::to_vec_pretty(delivery).map_err(io::Error::other)?;
    bytes.push(b'\n');
    match publish_new(&inbox.join(format!("{}.json", delivery.id)), &bytes, Permissions::Private) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

/// Takes everything in a resident's inbox, oldest first.
pub fn collect(world: &WorldDir, resident: &ResidentId, town: Option<&TownName>) -> io::Result<Vec<Delivery>> {
    let inbox = world.resident(town, resident).inbox();
    let mut deliveries = Vec::new();
    for path in entries(&inbox)? {
        // A notice being written sits under a hidden temporary name until
        // it is whole.
        let hidden = path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden || path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let delivery: Delivery = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?;
        std::fs::remove_file(&path)?;
        deliveries.push(delivery);
    }
    deliveries.sort_by_key(|delivery| delivery.at);
    Ok(deliveries)
}

fn entries(directory: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    match std::fs::read_dir(directory) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
