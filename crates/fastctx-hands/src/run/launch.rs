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
use fastctx_types::layout::{ConversationDir, Names, WorldDir};
use fastctx_types::run::{JobProgram, JobRecord, OutputMode, RunRequest};
use fastctx_types::{JobId, ProcessIdentity, TownName};

use super::record;

/// Who asks, where, and with what.
pub struct Launch<'a> {
    /// The owner confirms the foreground answer after its host accepts it.
    pub answer_receipts: bool,
    pub world: &'a WorldDir,
    /// The names machines answer to here: this machine's, and every name
    /// of a machine renamed since, whose residents' jobs go on in the rooms
    /// they had under its earlier names.
    pub names: &'a Names,
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
    pub on: Option<&'a fastctx_types::run::OnJob>,
    /// The account a job run for a resident of another machine works for.
    pub account: Option<&'a fastctx_types::access::HubAccount>,
}

/// A job whose supervisor has been started.
pub struct Launched {
    pub job: JobId,
    pub record: PathBuf,
    pub output: PathBuf,
    pub stderr: Option<PathBuf>,
    /// `None` when the supervisor had already exited by the time it was
    /// looked at.
    pub supervisor: Option<ProcessIdentity>,
}

pub fn launch(launch: &Launch<'_>) -> Result<Launched, Failure> {
    launch_with_io(launch, OutputMode::Merged, None)
}

/// Starts a job with merged terminal output or separate byte streams.
/// The latter is intended for `on`, whose stdout must remain pipeline data.
/// `stdin` names an already complete, verified local file. The caller keeps
/// it unchanged and present until the job ends. None connects null input.
pub fn launch_with_io(launch: &Launch<'_>, mode: OutputMode, stdin: Option<&Path>) -> Result<Launched, Failure> {
    launch_with_io_registered(launch, mode, stdin, |_| Ok(()))
}

/// Registers a consumer after preparation is durable and before a
/// supervisor can execute. The registration must release itself on errors.
pub fn launch_with_io_registered(
    launch: &Launch<'_>,
    mode: OutputMode,
    stdin: Option<&Path>,
    register: impl FnOnce(&Path) -> Result<(), Failure>,
) -> Result<Launched, Failure> {
    let (job, output, stderr, record_path, resident) =
        prepare(launch, mode, stdin).map_err(|failure| Failure::not_run(failure.message))?;
    register(&record_path)?;
    if let Some(request) = launch.on.and_then(|on| on.request.as_ref()) {
        let stopped = resident.jobs().join(format!("request-{request}.stopped"));
        if stopped
            .try_exists()
            .map_err(|error| Failure::not_run(format!("reading the request's cancellation: {error}")))?
        {
            let mut record = record::read(&record_path)
                .map_err(|error| Failure::not_run(format!("reading the cancelled job: {error}")))?;
            let at = jiff::Timestamp::now();
            record.stop = Some(fastctx_types::run::StopRequested {
                at,
                request: fastctx_types::run::StopRequest {
                    by: launch.owner.to_string(),
                    reason: "the on that sent it stopped before its supervisor started".to_string(),
                },
            });
            record.end = Some(fastctx_types::run::Ended { at, how: fastctx_types::run::EndedHow::Stopped });
            record::write(&record_path, &record)
                .map_err(|error| Failure::not_run(format!("recording the cancelled job: {error}")))?;
            return Err(Failure::not_run("the on was stopped before its supervisor started"));
        }
    }
    let started = spawn_detached(
        launch.executable,
        &[OsStr::new("supervise"), launch.world.path().as_os_str(), record_path.as_os_str()],
        resident.path(),
        launch.environment,
    )
    .map_err(|error| failure("starting the job's supervisor (execution may have begun)", &error))?;
    Ok(Launched { job, record: record_path, output, stderr, supervisor: started.identity })
}

type Prepared = (JobId, PathBuf, Option<PathBuf>, PathBuf, ConversationDir);

fn prepare(launch: &Launch<'_>, mode: OutputMode, stdin: Option<&Path>) -> Result<Prepared, Failure> {
    super::preread::check(&launch.request.script)?;
    if let Some(path) = stdin {
        if !path.is_absolute() {
            return Err(Failure::new(format!("stdin must name an absolute path: {}", path.display())));
        }
        let metadata = std::fs::metadata(path).map_err(|error| failure("reading the stdin file", &error))?;
        if !metadata.is_file() {
            return Err(Failure::new(format!("stdin must name a regular file: {}", path.display())));
        }
    }
    let cwd = starting_directory(launch)?;
    let resident = launch.world.job_owner(launch.names, launch.owner);
    let (job, output) = reserve(&resident).map_err(|error| failure("could not reserve a job", &error))?;
    let stderr = if mode == OutputMode::Separate {
        let path = output.with_extension("stderr.log");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| failure("could not create the stderr log", &error))?;
        Some(path)
    } else {
        None
    };
    let record_path = resident.job_record(&job);
    let script = resident.job_script(&job);
    // Bash reads the script with its `on`-to-`on` pipes marked; the record
    // keeps it as it was written.
    publish_new(&script, super::heredoc::mark_on_pipes(&launch.request.script).as_bytes(), Permissions::Private)
        .map_err(|error| failure("could not write the script", &error))?;
    let record = JobRecord {
        turn: launch.request.turn.clone(),
        call_id: launch.request.call_id.clone(),
        id: job.clone(),
        machine: launch.names.me.clone(),
        owner: launch.owner.clone(),
        town: launch.town.cloned(),
        script: launch.request.script.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        stdin: stdin.map(|path| path.to_string_lossy().into_owned()),
        output: output.to_string_lossy().into_owned(),
        stderr: stderr.as_ref().map(|path| path.to_string_lossy().into_owned()),
        started_at: jiff::Timestamp::now(),
        program: Some(JobProgram {
            executable: launch.executable.to_string_lossy().into_owned(),
            starter: current_identity().map_err(|error| failure("could not identify the job's starter", &error))?,
        }),
        supervisor: None,
        pid: None,
        exit: None,
        output_ended_at: None,
        output_drained: None,
        remaining: None,
        output_lost: None,
        stderr_lost: None,
        stop: None,
        end: None,
        ends_with_node: false,
        writes_off: launch.request.writes_off,
        on: launch.on.cloned(),
        account: launch.account.cloned(),
    };
    record::write(&record_path, &record).map_err(|error| failure("could not write the job's record", &error))?;
    if launch.owner.resident().is_some()
        && launch.on.is_none_or(|on| on.sentinel.is_none())
        && (launch.on.is_some() || launch.answer_receipts)
    {
        // No supervisor can execute until its completion debt is durable.
        // Empty markers retain the pre-negotiation background/on behavior.
        let debt: &[u8] = if launch.on.is_none() && launch.answer_receipts { b"answer" } else { b"" };
        publish_new(&resident.job_owed(&job), debt, Permissions::Private)
            .map_err(|error| failure("could not keep the job's completion debt", &error))?;
    }
    Ok((job, output, stderr, record_path, resident))
}

/// The directory the script starts in: the one asked for, which must be an
/// absolute path to a directory on this machine, or the home.
fn starting_directory(launch: &Launch<'_>) -> Result<PathBuf, Failure> {
    let Some(cwd) = &launch.request.cwd else {
        return Ok(launch.home.to_path_buf());
    };
    // A path that is not this machine's is most often another machine's,
    // written either way: the model meant to work there, which `on` does,
    // not `cwd`. Every refusal says so, whatever this machine found.
    let refused = |reason: String| {
        Failure::new(format!(
            "cwd {cwd:?} is not a directory of {}, where the script starts ({reason}); to work in another machine's directory, write `on MACHINE` in the script and `cd` there",
            launch.names.me
        ))
    };
    let path = fastctx_platform::local_path(cwd).map_err(|error| refused(error.to_string()))?;
    if !path.is_absolute() {
        let example = if cfg!(windows) { "C:/Users/me/project" } else { "/home/me/project" };
        return Err(refused(format!("not an absolute path here, where paths are written like {example}")));
    }
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Err(refused("it is a file".into())),
        Err(error) => Err(refused(error.to_string())),
    }
}

/// Picks an unused job id. The output file, created exclusively, is what
/// reserves it.
pub fn reserve(resident: &ConversationDir) -> io::Result<(JobId, PathBuf)> {
    ensure_private_dir(resident.path())?;
    std::fs::create_dir_all(resident.jobs())?;
    std::fs::create_dir_all(resident.out())?;
    loop {
        let job = random_job_id()?;
        // Losing or removing a log does not release the identity of the
        // job whose durable record still exists.
        if resident.job_record(&job).try_exists()? {
            continue;
        }
        let output = resident.job_output(&job);
        if resident.job_scratch(&job).try_exists()? {
            continue;
        }
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
