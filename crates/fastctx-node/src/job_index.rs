//! The jobs here that are running, or ended within the last day, by their
//! record's path: readers that list running or recent jobs read those
//! records, not every job record ever kept here. The index lives in memory
//! for the node's run. It is made by reading every record once, the first
//! time it is asked for. A job comes in as it starts (`started`); one goes
//! when a reader finds its record gone or ended more than a day ago.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use fastctx_types::layout::WorldDir;
use fastctx_types::run::JobRecord;

/// How long a job that ended stays in the index.
const KEPT: jiff::SignedDuration = jiff::SignedDuration::from_hours(24);

/// How long after a job ended when it ended is kept beside its path: by
/// then its supervisor has said how its output drained, and its record is
/// written no more but as its files go.
const SETTLED: jiff::SignedDuration = jiff::SignedDuration::from_mins(10);

/// What a record's file is told from another writing of it by: how long it
/// is, and when it was written.
type Written = (u64, Option<std::time::SystemTime>);

#[derive(Default)]
struct Index {
    /// None until made.
    paths: Option<BTreeSet<PathBuf>>,
    /// Jobs that started while it was being made.
    started: BTreeSet<PathBuf>,
    /// When the jobs that ended a while ago ended, each with what its
    /// record's file was as that was read. A reader that asks for the jobs
    /// running, or ended since a time after it, passes such a record by
    /// while its file is the same: every record of the day was read and
    /// parsed at each asking who is at work here, some thousands on a
    /// machine that runs many jobs.
    ended: HashMap<PathBuf, (Written, jiff::Timestamp)>,
}

static INDEXES: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<Index>>>>> = LazyLock::new(Default::default);

fn of(world: &WorldDir) -> Arc<Mutex<Index>> {
    INDEXES.lock().expect("never poisoned").entry(world.path().to_path_buf()).or_default().clone()
}

/// The job whose record is at `path` started: it is listed from now on.
pub(crate) fn started(world: &WorldDir, path: &Path) {
    let index = of(world);
    let mut index = index.lock().expect("never poisoned");
    match &mut index.paths {
        Some(paths) => paths.insert(path.to_path_buf()),
        None => index.started.insert(path.to_path_buf()),
    };
}

/// The records of the jobs here that are running, or ended since `since`
/// (no earlier than a day ago), each with where it is kept.
pub(crate) fn recent(world: &WorldDir, since: jiff::Timestamp) -> io::Result<Vec<(JobRecord, PathBuf)>> {
    let index = of(world);
    let made = index.lock().expect("never poisoned").paths.is_some();
    if !made {
        let found = every(world)?;
        let mut index = index.lock().expect("never poisoned");
        let started = std::mem::take(&mut index.started);
        let paths = index.paths.get_or_insert_with(BTreeSet::new);
        paths.extend(found);
        paths.extend(started);
    }
    let paths: Vec<PathBuf> = index.lock().expect("never poisoned").paths.iter().flatten().cloned().collect();
    let now = jiff::Timestamp::now();
    let kept_since = now - KEPT;
    let mut records = Vec::new();
    let mut gone = Vec::new();
    let mut settled = Vec::new();
    for path in paths {
        let written: Written = match fastctx_platform::disk::metadata(&path) {
            Ok(metadata) => (metadata.len(), metadata.modified().ok()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                gone.push(path);
                continue;
            }
            Err(error) => return Err(error),
        };
        let ended = index
            .lock()
            .expect("never poisoned")
            .ended
            .get(&path)
            .filter(|(was, _)| *was == written)
            .map(|(_, at)| *at);
        match ended {
            Some(at) if at < kept_since => {
                gone.push(path);
                continue;
            }
            Some(at) if at < since => continue,
            _ => {}
        }
        let record = match fastctx_hands::run::record::read(&path) {
            Ok(record) => record,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                gone.push(path);
                continue;
            }
            Err(error) => return Err(error),
        };
        match record.end.as_ref().map(|end| end.at) {
            Some(at) if at < kept_since => gone.push(path),
            Some(at) => {
                if ended.is_none() && at < now - SETTLED {
                    settled.push((path.clone(), (written, at)));
                }
                if at >= since {
                    records.push((record, path));
                }
            }
            None => records.push((record, path)),
        }
    }
    if !gone.is_empty() || !settled.is_empty() {
        let mut index = index.lock().expect("never poisoned");
        index.ended.extend(settled);
        for path in &gone {
            index.ended.remove(path);
        }
        if let Some(paths) = &mut index.paths {
            for path in &gone {
                paths.remove(path);
            }
        }
    }
    Ok(records)
}

/// The records of the jobs running here.
pub(crate) fn running(world: &WorldDir) -> io::Result<Vec<(JobRecord, PathBuf)>> {
    let mut records = recent(world, jiff::Timestamp::now())?;
    records.retain(|(record, _)| record.end.is_none());
    Ok(records)
}

/// Every job record here running or ended within a day, read once.
/// The records of every job here that is running, or ended since `since`,
/// however long ago: read from where they are kept, past the index's day
/// (M07). Read whole each time; for a look that takes in everything.
pub(crate) fn ended_since(world: &WorldDir, since: jiff::Timestamp) -> io::Result<Vec<(JobRecord, PathBuf)>> {
    let mut found = Vec::new();
    for parent in world.job_parents()? {
        for owner in entries(&parent)? {
            for path in entries(&owner.join("jobs"))? {
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                match fastctx_hands::run::record::read(&path) {
                    Ok(record) if record.end.as_ref().is_none_or(|end| end.at >= since) => found.push((record, path)),
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(found)
}

fn every(world: &WorldDir) -> io::Result<Vec<PathBuf>> {
    let since = jiff::Timestamp::now() - KEPT;
    let mut found = Vec::new();
    for parent in world.job_parents()? {
        for owner in entries(&parent)? {
            for path in entries(&owner.join("jobs"))? {
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                match fastctx_hands::run::record::read(&path) {
                    Ok(record) if record.end.as_ref().is_none_or(|end| end.at >= since) => found.push(path),
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    // One record that cannot be read is not counted, as
                    // listing all jobs before passed over none: it is kept
                    // and read again by each reader, which tells its error.
                    Err(_) => found.push(path),
                }
            }
        }
    }
    Ok(found)
}

fn entries(path: &Path) -> io::Result<Vec<PathBuf>> {
    match fastctx_platform::disk::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
