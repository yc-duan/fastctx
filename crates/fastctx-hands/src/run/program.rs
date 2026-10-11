//! Executables reserved by jobs, including time spent only in descendants.
//!
//! The node writes the reservation before starting the supervisor. Only a
//! finished process tree releases it: a shell's exit, closed output, or a
//! dead Linux supervisor does not prove that its descendants have ended.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::identity::is_from_earlier_boot;
use fastctx_platform::is_alive;
use fastctx_types::layout::WorldDir;
use fastctx_types::run::{EndedHow, JobRecord};

use super::record;

/// A job whose program is still needed, or whose descendants' fate is not
/// known. An old record without a program reserves every older copy.
pub struct ProgramUse {
    directory: Option<PathBuf>,
    record: PathBuf,
    reason: &'static str,
}

impl ProgramUse {
    /// The recorded reason the job's tree is still reserved or unproven.
    pub fn reason(&self) -> String {
        format!("{}: {}", self.record.display(), self.reason)
    }

    /// Whether this use prevents deleting `directory`, and why.
    pub fn reason_for(&self, directory: &Path) -> io::Result<Option<String>> {
        if let Some(held) = &self.directory
            && *held != fs::canonicalize(directory)?
        {
            return Ok(None);
        }
        Ok(Some(format!("{}: {}", self.record.display(), self.reason)))
    }
}

/// Reads the jobs that still reserve a program. The caller holds the World
/// node lock, so a previous node cannot start another job while this snapshot
/// is used to remove copies. Existing jobs can only finish in the meantime.
pub fn uses(world: &WorldDir) -> io::Result<Vec<ProgramUse>> {
    let mut uses = Vec::new();
    for residents in world.job_parents()? {
        for resident in entries(&residents)? {
            for path in entries(&resident.join("jobs"))? {
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let job = record::read(&path)?;
                if let Some(reason) = needed(&job)? {
                    let directory = job
                        .program
                        .as_ref()
                        .map(|program| {
                            let path = Path::new(&program.executable);
                            let parent = path.parent().ok_or_else(|| {
                                io::Error::new(io::ErrorKind::InvalidData, "job's program has no directory")
                            })?;
                            fs::canonicalize(parent)
                        })
                        .transpose()?;
                    uses.push(ProgramUse { directory, record: path, reason });
                }
            }
        }
    }
    Ok(uses)
}

fn needed(job: &JobRecord) -> io::Result<Option<&'static str>> {
    match job.end.as_ref().map(|end| &end.how) {
        Some(EndedHow::Finished | EndedHow::Stopped) => return Ok(None),
        // No tree was started. Errors after spawn retain the shell PID and
        // may leave descendants on Linux, so Failed alone is not enough.
        Some(EndedHow::Failed { .. }) if job.pid.is_none() => return Ok(None),
        _ => {}
    }
    if let Some(supervisor) = &job.supervisor {
        if is_alive(supervisor)? {
            return Ok(Some("the job's supervisor still holds its process tree"));
        }
        if cfg!(windows) {
            // The supervisor owns a KILL_ON_JOB_CLOSE job object. Its
            // termination ends that tree even without a final record.
            return Ok(None);
        }
        if is_from_earlier_boot(supervisor)? {
            return Ok(None);
        }
        return Ok(Some("the supervisor ended without establishing that its descendants ended"));
    }
    if let Some(program) = &job.program
        && is_from_earlier_boot(&program.starter)?
    {
        return Ok(None);
    }
    Ok(Some("the supervisor has not recorded the fate of the reserved job"))
}

/// Whether a job can still have processes using its files.
pub fn in_use(job: &JobRecord) -> io::Result<bool> {
    Ok(needed(job)?.is_some())
}

fn entries(path: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
