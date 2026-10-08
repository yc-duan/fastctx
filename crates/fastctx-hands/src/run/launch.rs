//! Starting a job: the node's side.
//!
//! The node reserves the job's id, writes the script and the first record,
//! and starts the job's supervisor on its own. From the first moment the
//! script runs under that supervisor, so a job that outlives the wait needs
//! no hand-over, and a node that restarts leaves it untouched.

use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, current_identity, ensure_private_dir, publish_new, spawn_detached};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::{ResidentDir, WorldDir};
use fastctx_types::run::{JobProgram, JobRecord, RunRequest};
use fastctx_types::{JobId, MachineName, ProcessIdentity, TownName};

use super::record;

/// Who asks, where, and with what.
pub struct Launch<'a> {
    pub world: &'a WorldDir,
    pub machine: &'a MachineName,
    /// Who asks, at the machine it lives on.
    pub owner: &'a fastctx_types::run::JobOwner,
    pub town: Option<&'a TownName>,
    pub request: &'a RunRequest,
    /// The machine's home, where a script without a `cwd` starts.
    pub home: &'a Path,
    /// The fastctx binary, which also serves as the supervisor.
    pub executable: &'a Path,
    /// The environment the supervisor starts with.
    pub environment: &'a [(OsString, OsString)],
    /// What an `on` sent with the script; the files are in the resident's
    /// directory already.
    pub on: Option<fastctx_types::run::OnJob>,
}

/// A job whose supervisor has been started.
pub struct Launched {
    pub job: JobId,
    pub record: PathBuf,
    pub output: PathBuf,
    /// `None` when the supervisor had already exited by the time it was
    /// looked at.
    pub supervisor: Option<ProcessIdentity>,
}

pub fn launch(launch: &Launch<'_>) -> Result<Launched, Failure> {
    super::preread::check(&launch.request.script)?;
    let cwd = starting_directory(launch)?;
    let resident = launch.world.job_owner(launch.machine, launch.town, launch.owner);
    let (job, output) = reserve(&resident).map_err(|error| failure("could not reserve a job", &error))?;
    let record_path = resident.job_record(&job);
    let script = resident.job_script(&job);
    publish_new(&script, launch.request.script.as_bytes(), Permissions::Private)
        .map_err(|error| failure("could not write the script", &error))?;
    let record = JobRecord {
        id: job.clone(),
        machine: launch.machine.clone(),
        owner: launch.owner.clone(),
        town: launch.town.cloned(),
        script: launch.request.script.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        output: output.to_string_lossy().into_owned(),
        started_at: jiff::Timestamp::now(),
        program: Some(JobProgram {
            executable: launch.executable.to_string_lossy().into_owned(),
            starter: current_identity().map_err(|error| failure("could not identify the job's starter", &error))?,
        }),
        supervisor: None,
        pid: None,
        exit: None,
        output_ended_at: None,
        remaining: None,
        output_lost: None,
        end: None,
        ends_with_node: false,
        writes_off: launch.request.writes_off,
        // Standard error apart from the output, for `on` to hand back on its
        // own (`world-run.md`).
        on: launch.on.clone().map(|on| fastctx_types::run::OnJob {
            errors: resident.out().join(format!("{job}.err")).to_string_lossy().into_owned(),
            ..on
        }),
    };
    record::write(&record_path, &record).map_err(|error| failure("could not write the job's record", &error))?;
    let started = spawn_detached(
        launch.executable,
        &[OsStr::new("supervise"), launch.world.path().as_os_str(), record_path.as_os_str()],
        resident.path(),
        launch.environment,
    )
    .map_err(|error| failure("could not start the job's supervisor", &error))?;
    Ok(Launched { job, record: record_path, output, supervisor: started.identity })
}

/// The directory the script starts in: the one asked for, which must be an
/// absolute path to a directory on this machine, or the home.
fn starting_directory(launch: &Launch<'_>) -> Result<PathBuf, Failure> {
    let Some(cwd) = &launch.request.cwd else {
        return Ok(launch.home.to_path_buf());
    };
    let path = PathBuf::from(cwd);
    if !path.is_absolute() {
        let example = if cfg!(windows) { "C:/Users/me/project" } else { "/home/me/project" };
        return Err(Failure::new(format!(
            "cwd {cwd:?} is not an absolute path on {}; write it the way this machine does, such as {example}",
            launch.machine
        )));
    }
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Err(Failure::new(format!("cwd {cwd:?} on {} is not a directory", launch.machine))),
        Err(error) => Err(failure(&format!("cwd {cwd:?} on {}", launch.machine), &error)),
    }
}

/// Picks an unused job id. The output file, created exclusively, is what
/// reserves it.
fn reserve(resident: &ResidentDir) -> io::Result<(JobId, PathBuf)> {
    ensure_private_dir(resident.path())?;
    std::fs::create_dir_all(resident.jobs())?;
    std::fs::create_dir_all(resident.out())?;
    loop {
        let job = random_job_id()?;
        let output = resident.job_output(&job);
        match OpenOptions::new().write(true).create_new(true).open(&output) {
            Ok(_) => return Ok((job, output)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn random_job_id() -> io::Result<JobId> {
    const ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 6];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    // Each byte picks one of 36 symbols; the slight bias of 256 mod 36
    // does not matter for an id that is checked for collisions anyway.
    let suffix: String = bytes.iter().map(|&b| ALPHABET[usize::from(b) % ALPHABET.len()] as char).collect();
    Ok(JobId::try_from(format!("j-{suffix}")).expect("six lowercase letters or digits"))
}

fn failure(what: &str, error: &io::Error) -> Failure {
    Failure::new(format!("{what}: {error}"))
}
