//! Jobs on this machine: starting one for `run`, waiting for it within the
//! call, and noticing jobs whose supervisor is gone.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_hands::run::{self, Launch, WAIT, record, window};
use fastctx_platform::{FileLock, is_alive, user_home, world_process_environment};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::Action;
use fastctx_types::run::{Ended, EndedHow, JobRecord, RunReply, RunRequest, RunState};
use fastctx_types::{JobId, TownName};
use jiff::Timestamp;
use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::Node;

pub struct Jobs {
    changes: broadcast::Sender<JobId>,
}

impl Jobs {
    pub fn new() -> Self {
        // A waiter that falls this far behind reads its job's record again,
        // so the capacity only bounds how often that happens.
        let (changes, _) = broadcast::channel(1024);
        Self { changes }
    }

    /// A supervisor said its job's record changed: wake whoever waits for
    /// it, and tell its resident if it ended and that is owed.
    pub async fn changed(
        &self,
        node: &Arc<Node>,
        job: JobId,
        owner: fastctx_types::run::JobOwner,
        town: Option<TownName>,
    ) -> Result<(), Failure> {
        // With nobody waiting there is nobody to wake.
        let _ = self.changes.send(job.clone());
        let path = node.world.job_owner(&node.machine(), town.as_ref(), &owner).job_record(&job);
        let record = read(&path).await?;
        crate::inbox::tell_if_ended(node, &record).await.map_err(|error| failure("telling the job's end", error))
    }

    /// Listens for changes to jobs' records: each change names its job.
    pub fn subscribe(&self) -> broadcast::Receiver<JobId> {
        self.changes.subscribe()
    }

    /// Starts `request` for `owner` and returns at once.
    pub async fn launch(
        &self,
        node: &Arc<Node>,
        owner: fastctx_types::run::JobOwner,
        town: Option<TownName>,
        request: RunRequest,
        on: Option<fastctx_types::run::OnJob>,
    ) -> Result<run::Launched, Failure> {
        let (world, machine, executable) = (node.world.clone(), node.machine(), node.fastctx.clone());
        tokio::task::spawn_blocking(move || -> Result<run::Launched, Failure> {
            let home = user_home().map_err(|error| failure("finding the home directory", error))?;
            let environment =
                world_process_environment().map_err(|error| failure("reading the user's environment", error))?;
            run::launch(&Launch {
                world: &world,
                machine: &machine,
                owner: &owner,
                town: town.as_ref(),
                request: &request,
                home: &home,
                executable: &executable,
                environment: &environment,
                on,
            })
        })
        .await
        .map_err(|error| Failure::new(format!("starting the job: {error}")))?
    }

    /// Starts `request` for `owner` and waits until the job settles or the
    /// wait is over. Returns the answer and the job's record as it stood.
    pub async fn run(
        &self,
        node: &Arc<Node>,
        owner: fastctx_types::run::JobOwner,
        town: Option<TownName>,
        request: RunRequest,
    ) -> Result<(RunReply, JobRecord), Failure> {
        let (world, machine) = (&node.world, &node.machine());
        let deadline = Instant::now() + WAIT;
        // Listening before the job exists, so its first change is not missed.
        let mut changes = self.changes.subscribe();
        let launched = self.launch(node, owner, town, request, None).await?;
        loop {
            let record = read(&launched.record).await?;
            if settled(&record) {
                break;
            }
            tokio::select! {
                change = changes.recv() => match change {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => unreachable!("the node holds the sender"),
                },
                () = tokio::time::sleep_until(deadline) => break,
            }
        }
        let mut record = read(&launched.record).await?;
        if record.end.is_none() && supervisor_gone(&record, &launched).await {
            // It ended without saying how; the record says so now.
            record = interrupt(&launched.record, "its supervisor ended without finishing the record").await?;
        }
        let output = {
            let log = PathBuf::from(&record.output);
            tokio::task::spawn_blocking(move || window::window(&log))
                .await
                .map_err(|error| Failure::new(format!("reading the output: {error}")))?
                .map_err(|error| failure("reading the output", error))?
        };
        if record.end.is_none() && record.owner.resident().is_some() {
            // The job goes on past this answer: its end is owed to the
            // resident who started it. It may have ended between the answer
            // and the marker, and then nobody else would tell.
            let (world, here, job, path) =
                (world.clone(), machine.clone(), launched.job.clone(), launched.record.clone());
            let now = tokio::task::spawn_blocking(move || crate::inbox::owe_job(&world, &here, &job, &path))
                .await
                .map_err(|error| Failure::new(format!("noting that the job's end is owed: {error}")))?
                .map_err(|error| failure("noting that the job's end is owed", error))?;
            crate::inbox::tell_if_ended(node, &now).await.map_err(|error| failure("telling the job's end", error))?;
        }
        let visited =
            visited(&node.world.job_owner(machine, record.town.as_ref(), &record.owner).jobs(), &launched.job);
        let reply = RunReply { machine: machine.clone(), job: launched.job, state: state(&record), output, visited };
        Ok((reply, record))
    }
}

/// The action a `run` was, as its resident's `actions` record it.
pub fn action(reply: &RunReply, script: &str, at: jiff::Timestamp) -> Action {
    Action {
        at,
        machine: reply.machine.clone(),
        hand: "run".to_string(),
        what: first_line(script),
        result: outcome(reply),
    }
}

/// A script's first line, marked when more lines follow.
pub fn first_line(script: &str) -> String {
    let trimmed = script.trim();
    match trimmed.split_once('\n') {
        Some((first, _)) => format!("{} …", first.trim_end()),
        None => trimmed.to_string(),
    }
}

/// How a `run` came out, in a few words.
fn outcome(reply: &RunReply) -> String {
    let status = |status: &fastctx_types::run::ExitStatus| match status {
        fastctx_types::run::ExitStatus::Code(code) => format!("exit {code}"),
        fastctx_types::run::ExitStatus::Signal(signal) => format!("signal {signal}"),
    };
    match &reply.state {
        RunState::Ended { status: exit } => status(exit),
        RunState::Exited { status: exit, remaining } => {
            format!("{}; {remaining} still running as {}", status(exit), reply.job)
        }
        RunState::Running => format!("running as {}", reply.job),
        RunState::Stopped => "stopped".to_string(),
        RunState::Interrupted { .. } => "interrupted".to_string(),
        RunState::Failed { .. } => "could not run".to_string(),
    }
}

/// Whether the call can answer now: the job ended, or its shell exited
/// and its output is complete.
fn settled(record: &JobRecord) -> bool {
    record.end.is_some() || (record.exit.is_some() && record.output_ended_at.is_some())
}

fn state(record: &JobRecord) -> RunState {
    match (&record.end, &record.exit) {
        (Some(Ended { how: EndedHow::Failed { error }, .. }), _) => RunState::Failed { error: error.clone() },
        (Some(Ended { how: EndedHow::Stopped, .. }), _) => RunState::Stopped,
        (Some(Ended { how: EndedHow::Interrupted { reason }, .. }), _) => {
            RunState::Interrupted { reason: reason.clone() }
        }
        (Some(Ended { how: EndedHow::Finished, .. }), Some(exit)) => RunState::Ended { status: exit.status },
        (Some(Ended { how: EndedHow::Finished, .. }), None) => {
            RunState::Failed { error: "its record says it finished but holds no exit status".to_string() }
        }
        (None, Some(exit)) if record.output_ended_at.is_some() => {
            RunState::Exited { status: exit.status, remaining: record.remaining.unwrap_or(0) }
        }
        (None, _) => RunState::Running,
    }
}

async fn supervisor_gone(record: &JobRecord, launched: &run::Launched) -> bool {
    let identity = record.supervisor.clone().or_else(|| launched.supervisor.clone());
    match identity {
        // A supervisor that cannot be checked is not taken for gone.
        Some(identity) => {
            tokio::task::spawn_blocking(move || !is_alive(&identity).unwrap_or(true)).await.unwrap_or(false)
        }
        // The supervisor had already exited when it was started.
        None => true,
    }
}

/// The machines a job's `on`s went to, as `on` noted them beside it.
fn visited(jobs: &Path, job: &JobId) -> Vec<(fastctx_types::MachineName, JobId)> {
    let Ok(text) = std::fs::read_to_string(jobs.join(format!("{job}.on"))) else { return Vec::new() };
    text.lines()
        .filter_map(|line| {
            let (machine, job) = line.split_once(' ')?;
            Some((fastctx_types::MachineName::try_from(machine).ok()?, JobId::try_from(job).ok()?))
        })
        .collect()
}

pub(crate) async fn read(path: &Path) -> Result<JobRecord, Failure> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || record::read(&path))
        .await
        .map_err(|error| Failure::new(format!("reading the job's record: {error}")))?
        .map_err(|error| failure("reading the job's record", error))
}

/// Records that a job ended without its supervisor, if the supervisor is
/// indeed gone: its lock is free.
async fn interrupt(path: &Path, reason: &str) -> Result<JobRecord, Failure> {
    let (path, reason) = (path.to_path_buf(), reason.to_string());
    tokio::task::spawn_blocking(move || interrupt_now(&path, &reason))
        .await
        .map_err(|error| Failure::new(format!("recording the interruption: {error}")))?
        .map_err(|error| failure("recording the interruption", error))
}

fn interrupt_now(path: &Path, reason: &str) -> std::io::Result<JobRecord> {
    let Some(_lock) = FileLock::try_acquire(&record::lock_path(path))? else {
        // A supervisor holds it after all.
        return record::read(path);
    };
    let mut job = record::read(path)?;
    if job.end.is_none() {
        job.end = Some(Ended { at: Timestamp::now(), how: EndedHow::Interrupted { reason: reason.to_string() } });
        record::write(path, &job)?;
    }
    Ok(job)
}

/// What reconciling found.
#[derive(Default)]
pub struct Reconciled {
    /// Jobs whose supervisor was gone, now marked interrupted.
    pub interrupted: Vec<PathBuf>,
    /// Records that could not be read or marked, and why. They are left as
    /// they are for a person to look at.
    pub problems: Vec<(PathBuf, std::io::Error)>,
}

/// Marks every job whose supervisor is gone, as after the machine
/// restarted.
pub fn reconcile(world: &WorldDir) -> std::io::Result<Reconciled> {
    let mut found = Reconciled::default();
    for residents in world.job_parents()? {
        for resident in read_dir(&residents)? {
            for entry in read_dir(&resident.join("jobs"))? {
                if entry.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let checked = record::read(&entry).and_then(|job| match job.end {
                    Some(_) => Ok(false),
                    None => interrupt_now(&entry, "its supervisor was gone when the node started")
                        .map(|job| job.end.is_some()),
                });
                match checked {
                    Ok(true) => found.interrupted.push(entry),
                    Ok(false) => {}
                    Err(error) => found.problems.push((entry, error)),
                }
            }
        }
    }
    Ok(found)
}

fn read_dir(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn failure(what: &str, error: std::io::Error) -> Failure {
    Failure::new(format!("{what}: {error}"))
}

/// Stops the job at `record_path`: its supervisor ends everything it
/// started and records who stopped it.
pub async fn stop(_node: &Arc<Node>, record_path: &Path, by: &str) -> Result<(), Failure> {
    Err(Failure::new(format!("stopping {} for {by} is not wired yet", record_path.display())))
}
