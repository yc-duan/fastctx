//! Jobs on this machine: starting one for `run`, waiting for it within the
//! call, and noticing jobs whose supervisor is gone.
//!
//! A call waits until its job settles, the wait is over, or words come for
//! the resident who asked: an answer is how words reach a session the World
//! cannot speak into, and the moment a busy session reads them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fastctx_hands::run::{self, LONGEST_WAIT, Launch, WAIT, record, window};
use fastctx_platform::{FileLock, is_alive, user_home, world_process_environment};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::Action;
use fastctx_types::run::{
    Answered, Ended, EndedHow, JobRecord, OutputPiece, OutputWindow, RunReply, RunRequest, RunState,
};
use fastctx_types::{JobId, TownName};
use jiff::Timestamp;
use tokio::sync::{broadcast, watch};
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
        let path = node.world.job_record(&node.names(), town.as_ref(), &owner, &job);
        let record = read(&path).await.inspect_err(|_| {
            crate::inbox::retry_job(node, &path);
        })?;
        let (writes, steps) = if record.end.is_some() && !record.is_sentinel() {
            (
                crate::run_facts::writes(&path).map_err(|error| failure("reading write facts", error))?,
                crate::run_facts::steps(&path).map_err(|error| failure("reading step facts", error))?,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        crate::activity_book::note(node, &record, &writes, &steps);
        crate::run_facts::update_job_with(node, &path, writes, steps)
            .map_err(|error| failure("updating call facts", error))?;
        match crate::inbox::tell_if_ended(node, &path, &record).await {
            Ok(()) => Ok(()),
            Err(error) => {
                crate::inbox::retry_job(node, &path);
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    Ok(())
                } else {
                    Err(failure("telling the job's end", error))
                }
            }
        }
    }

    /// Listens for changes to jobs' records: each change names its job.
    pub(crate) fn notify(&self, job: JobId) {
        let _ = self.changes.send(job);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JobId> {
        self.changes.subscribe()
    }

    /// Starts `request` for `owner` and returns at once.
    #[allow(clippy::too_many_arguments)]
    pub async fn launch(
        &self,
        node: &Arc<Node>,
        owner: fastctx_types::run::JobOwner,
        town: Option<TownName>,
        request: RunRequest,
        on: Option<(fastctx_types::run::OnJob, Option<PathBuf>)>,
        registration: Option<crate::on::Registration>,
        account: Option<fastctx_types::access::HubAccount>,
        answer_receipts: bool,
        dispatch: Option<fastctx_types::NoticeId>,
    ) -> Result<run::Launched, Failure> {
        let _place = match owner.resident().and_then(|address| crate::data::places::of_owner(node, address)) {
            Some(place) => Some(node.places.using(&place).await),
            None => None,
        };
        if let Some(address) = owner.resident()
            && node.is_me(&address.machine)
        {
            node.residents.active(&address.resident)?;
        }
        let (world, names, executable) = (node.world.clone(), node.names(), node.fastctx.clone());
        // Counted for whom it works, within the limit the owner set for it.
        let sent_for = on.as_ref().and_then(|(on, _)| on.account.clone()).or_else(|| account.clone());
        let counted = crate::job_accounts::of(node, &owner, sent_for.as_ref());
        let share = crate::job_accounts::share_key(counted.as_ref());
        let registering = node.clone();
        let launched = tokio::task::spawn_blocking(move || -> Result<run::Launched, Failure> {
            let starting = crate::job_accounts::start(&registering, counted)?;
            let _collecting = registering.disk.lock();
            let home = user_home().map_err(|error| before_start("finding the home directory", error))?;
            let mut environment =
                world_process_environment().map_err(|error| before_start("reading the user's environment", error))?;
            if on.as_ref().is_some_and(|(on, _)| on.sentinel.is_some()) && fastctx_hands::run::sentinel::unwalled() {
                environment.push(("FASTCTX_LAB_UNSANDBOXED_SENTINEL".into(), "1".into()));
            }
            // Its supervisor joins its account's share of the processors.
            if let Some(shares) = &registering.cpu_shares {
                match shares.of(&share) {
                    Ok(dir) => environment.push((fastctx_platform::cpu_share::VARIABLE.into(), dir.into())),
                    Err(error) => {
                        registering.log.line(format_args!("sharing the processors with job of {share}: {error}"))
                    }
                }
            }
            // An `on` job hands its standard error back apart from its output,
            // and reads the input that came with it (`world-run.md`).
            let (on, mode, input) = match on {
                Some((on, input)) => (Some(on), fastctx_types::run::OutputMode::Separate, input),
                None => (None, fastctx_types::run::OutputMode::Merged, None),
            };
            let launch = Launch {
                answer_receipts,
                world: &world,
                names: &names,
                owner: &owner,
                town: town.as_ref(),
                request: &request,
                home: &home,
                executable: &executable,
                environment: &environment,
                on: on.as_ref(),
                account: account.as_ref(),
            };
            let launched = run::launch::launch_with_io_registered(&launch, mode, input.as_deref(), |path| {
                if let Some(dispatch) = &dispatch {
                    crate::run_stops::register(&registering, path, dispatch)?;
                }
                if let Some(registration) = registration {
                    *registration.lock().expect("never poisoned") = Some(crate::on::Follower::reserved(
                        &registering,
                        path,
                        on.as_ref().is_some_and(|on| on.sentinel.is_some()),
                    ));
                }
                Ok(())
            })?;
            starting.started(&launched.record);
            crate::job_watch::start(&registering, &launched.record, launched.supervisor.clone());
            crate::run_facts::update_job(&registering, &launched.record, Vec::new())
                .map_err(|error| failure("updating call facts", error))?;
            Ok(launched)
        })
        .await
        .map_err(|error| Failure::new(format!("starting the job: {error}")))??;
        Ok(launched)
    }

    /// Starts `request` for `owner` and waits until the job settles, the
    /// wait is over, or words come for the resident who asked. Returns the
    /// answer and the job's record as it stood.
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        node: &Arc<Node>,
        owner: fastctx_types::run::JobOwner,
        town: Option<TownName>,
        mut request: RunRequest,
        account: Option<fastctx_types::access::HubAccount>,
        answer_receipts: bool,
        dispatch: Option<fastctx_types::NoticeId>,
    ) -> Result<(RunReply, JobRecord), Failure> {
        let machine = &node.machine();
        let wait = match request.wait {
            None => WAIT,
            Some(seconds) if u64::from(seconds) <= LONGEST_WAIT.as_secs() => Duration::from_secs(seconds.into()),
            Some(seconds) => {
                return Err(Failure::not_run(format!(
                    "a call waits at most {} seconds, not {seconds}",
                    LONGEST_WAIT.as_secs()
                )));
            }
        };
        let started = Instant::now();
        let deadline = started + wait;
        // Listening before the job exists, so its first change is not missed.
        let mut changes = self.changes.subscribe();
        // The machines the script names after `on` are reached for whom the
        // job works: its resident here, or the account a resident of
        // another machine sent it for. A page's acts are its own.
        let acting = if let fastctx_types::run::JobOwner::Page { page } = &owner {
            Some(crate::pages::identity::acting(node, page)?)
        } else {
            match owner.resident() {
                Some(address) if node.is_me(&address.machine) => {
                    node.residents.about(&address.resident).map(|(about, _)| about.acting)
                }
                Some(_) => account
                    .as_ref()
                    .map(|account| fastctx_types::access::Acting::one(account.hub.clone(), account.account.clone())),
                None => None,
            }
        };
        if let Some(acting) = &acting {
            crate::here::check_named(node, acting, &request.script)?;
        }
        // Words for a resident of another machine that works here come
        // from where it lives.
        let mut words = owner.resident().map(|address| node.words.watch(address));
        // The job stops with the turn of the call that waits for it; one the
        // call left running goes on (`world-run.md`).
        let mut interrupted = owner.resident().map(|address| node.interrupts.watch(address));
        // The files the user attached come before the script that may read them.
        let files = std::mem::take(&mut request.attachments);
        let missing = match (owner.resident(), &acting) {
            (Some(address), Some(acting)) => {
                let waits = Some(crate::attachments::follow::RUN_WAITS);
                crate::attachments::follow::bring(node, address, acting, files, waits).await
            }
            _ => Vec::new(),
        };
        let launched = self.launch(node, owner, town, request, None, None, account, answer_receipts, dispatch).await?;
        let mut answered = None;
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
                () = came(&mut interrupted) => {
                    // Its end, as the stop records it, is what this call answers.
                    interrupted = None;
                    let reason = "the turn that was waiting for it was interrupted";
                    if let Err(failure) = stop(node, &launched.record, fastctx_types::residents::STOPPED_BY_USER, reason).await {
                        node.log.line(format_args!("stopping job {} with its turn: {}", launched.job, failure.message));
                    }
                    continue;
                }
                () = tokio::time::sleep_until(deadline) => {
                    answered = Some(Answered::WaitOver { seconds: seconds(wait) });
                    break;
                }
                () = came(&mut words) => {
                    answered = Some(Answered::Words { seconds: seconds(started.elapsed()) });
                    break;
                }
            }
        }
        let mut record = read(&launched.record).await?;
        if record.end.is_none() && supervisor_gone(&record, &launched).await {
            // It ended without saying how; the record says so now.
            record = interrupt(&launched.record, "its supervisor ended without finishing the record").await?;
        }
        let output = {
            let log = PathBuf::from(&record.output);
            let through = record.output_drained.filter(|_| record.end.is_none()).map(|drained| drained.stdout);
            tokio::task::spawn_blocking(move || match through {
                Some(bytes) => window::window_through(&log, bytes),
                None => window::window(&log),
            })
            .await
            .map_err(|error| Failure::new(format!("reading the output: {error}")))?
            .map_err(|error| failure("reading the output", error))?
        };
        if record.end.is_none() && record.owner.resident().is_some() {
            // The job goes on past this answer: its end is owed to the
            // resident who started it. It may have ended between the answer
            // and the marker, and then nobody else would tell.
            let path = launched.record.clone();
            let now = tokio::task::spawn_blocking(move || crate::inbox::owe_job(&path))
                .await
                .map_err(|error| Failure::new(format!("noting that the job's end is owed: {error}")))?
                .map_err(|error| failure("noting that the job's end is owed", error))?;
            if let Err(error) = crate::inbox::tell_if_ended(node, &launched.record, &now).await {
                crate::inbox::retry_job(node, &launched.record);
                if error.kind() != std::io::ErrorKind::WouldBlock {
                    return Err(failure("telling the job's end", error));
                }
            }
        }
        // Beside its record, in whichever room the job went on (`run::launch`).
        let visited = launched.record.parent().map(|jobs| visited(jobs, &launched.job)).unwrap_or_default();
        let state = state(&record);
        // Why it answered says something only of a job that still runs.
        let answered = answered.filter(|_| state == RunState::Running);
        let (receipt_node, receipt_path) = (node.clone(), launched.record.clone());
        let consumed = tokio::task::spawn_blocking(move || crate::job_receipts::snapshot(&receipt_node, &receipt_path))
            .await
            .map_err(|error| Failure::new(format!("reading completion receipts: {error}")))?
            .map_err(|error| failure("reading completion receipts", error))?;
        if let Some(owner) = record.owner.resident().filter(|owner| node.is_me(&owner.machine)) {
            let directory = node.world.conversation(&owner.resident);
            crate::job_receipts::apply_consumed(&directory, &consumed)
                .map_err(|error| failure("reading completion receipts", error))?;
        }
        let consumed = (!consumed.is_empty()).then(|| Box::new(consumed));
        // What it ran was refused something: on a Mac, the node looks
        // whether macOS keeps its user's private places from it, which the
        // command's own words cannot tell.
        let privacy = if cfg!(target_os = "macos") && refused_something(&output) {
            tokio::task::spawn_blocking(fastctx_platform::privacy::refused_now).await.unwrap_or_default()
        } else {
            Vec::new()
        };
        let scratch = crate::scratch::jobs::locations(&launched.record, machine)
            .map_err(|error| failure("reading scratch locations", error))?;
        let mut reply = RunReply {
            scratch,
            run: None,
            machine: machine.clone(),
            job: launched.job,
            state,
            output,
            visited,
            answered,
            consumed,
            privacy,
            missing,
        };
        if record.call_id.is_some() {
            let mut facts = crate::run_facts::assemble(&reply, &launched.record)
                .map_err(|error| failure("recording run facts", error))?;
            if matches!(reply.state, RunState::Ended { .. }) && !facts.output.as_ref().is_some_and(|output| output.cut)
            {
                facts.output = None;
            }
            reply.run = Some(Box::new(facts));
        }
        if let Some(owner) = record.owner.resident() {
            crate::run_receipts::remember(node, owner, &reply)
                .map_err(|error| failure("remembering the run answer", error))?;
        }
        Ok((reply, record))
    }
}

/// Removes the files of a job that ended within its call and whose output
/// its answer showed whole, once the answer reached the host: nobody reads
/// them again, and `fastctx w` does not list such jobs (`world-run.md`,
/// 输出). Only `owner`'s own finished jobs go, never one owed word of its
/// end or sent here by another machine's `on`.
pub async fn forget(node: &Arc<Node>, town: Option<TownName>, owner: fastctx_types::Address, job: JobId) {
    let path = node.world.job_record(&node.names(), town.as_ref(), &owner.clone().into(), &job);
    let checked = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<Option<JobRecord>> {
            let record = record::read(&path)?;
            let finished = matches!(record.end, Some(Ended { how: EndedHow::Finished, .. }));
            Ok((record.owner == owner
                && finished
                && record.on.is_none()
                && !path.with_extension("owed").try_exists()?)
            .then_some(record))
        })
        .await
    };
    match checked {
        Ok(Ok(Some(record))) => {
            let owner = record.owner.resident().expect("checked resident");
            if let Err(error) = crate::retention::received(
                node,
                owner,
                &fastctx_types::run::JobDelivery {
                    job: record.id.clone(),
                    output: record.output.clone(),
                    at: Timestamp::now(),
                    shown_whole: true,
                },
            ) {
                node.log.line(format_args!("recording job delivery: {error}"));
            }
        }
        Ok(Ok(None)) => {}
        Ok(Err(error)) => node.log.line(format_args!("removing a job shown whole: {error}")),
        Err(error) => node.log.line(format_args!("removing a job shown whole: {error}")),
    }
}

/// How long collection, holding the disk's lock, waits for an ended job's
/// supervisor to exit: it exits as soon as it has written the end, and one
/// that has not by then is left for a later pass.
const SUPERVISOR_EXIT: Duration = Duration::from_secs(5);

/// Removes the files of the ended job whose record is at `path`; those of
/// a job an `on` sent here include the package it came in and what was
/// kept of its end. Gone already, there is nothing to remove; still in use,
/// it fails as `WouldBlock`.
pub(crate) fn remove(path: &Path) -> std::io::Result<()> {
    let record = match record::read(path) {
        Ok(record) => record,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if fastctx_hands::run::program::in_use(&record)? {
        return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "job processes still use its files"));
    }
    // The supervisor writes its last record and then lets go of the log; on
    // Windows a file it still holds cannot go.
    if let Some(supervisor) = &record.supervisor
        && !fastctx_platform::host_process::exited_within(supervisor, SUPERVISOR_EXIT)?
    {
        return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "its supervisor has not exited"));
    }
    let delivered =
        fastctx_install::renewal::read_json::<fastctx_types::run::JobDelivery>(&path.with_extension("delivered"))?
            .map(|delivery| delivery.at)
            .unwrap_or_else(Timestamp::now);
    retain_scratch(path, delivered)?;
    match std::fs::remove_dir_all(path.with_extension("writes")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    // The record goes last: a log without one would be found by nobody.
    let mut files = vec![
        PathBuf::from(&record.output),
        path.with_extension("sh"),
        path.with_extension("wait-taken"),
        path.with_extension("facts-state"),
        path.with_extension("facts-pending"),
        path.with_extension("writes.jsonl"),
        record::lock_path(path),
        path.with_extension("on"),
        path.with_extension("scratch"),
        path.with_extension("waited"),
    ];
    files.extend(record.stderr.as_deref().map(PathBuf::from));
    if let Some(on) = &record.on {
        files.push(PathBuf::from(&on.state));
        files.extend(record.stdin.as_deref().map(PathBuf::from));
        files.extend(["on-taken", "on-piped", "on-stopped"].map(|extension| path.with_extension(extension)));
    }
    files.extend(
        ["notice", "owed", "delivered", "posted", "answer-wait", "done", "retry", "run-request"]
            .map(|extension| path.with_extension(extension)),
    );
    files.extend(["copied", "wait-taken", "consumed", "exit-receipt"].map(|extension| path.with_extension(extension)));
    files.push(path.to_path_buf());
    for file in files {
        match std::fs::remove_file(&file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// A transferred output can go immediately; temporary artifacts retain
/// their own durable delivery clock after the job record is gone.
pub(crate) fn retain_scratch(path: &Path, delivered: Timestamp) -> std::io::Result<()> {
    let job = match record::read(path) {
        Ok(job) => job,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let out = Path::new(&job.output).parent().ok_or_else(|| std::io::Error::other("job output has no directory"))?;
    let scratch = out.join(format!("{}.tmp", job.id));
    if !scratch.try_exists()? {
        return Ok(());
    }
    if fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&scratch)?) {
        return Err(std::io::Error::other("job scratch is a filesystem link"));
    }
    if crate::retention::entries(&scratch)?.is_empty() {
        return std::fs::remove_dir(scratch);
    }
    match fastctx_platform::publish_new(
        &scratch.join(".delivered"),
        &serde_json::to_vec(&delivered).map_err(std::io::Error::other)?,
        fastctx_platform::Permissions::Private,
    ) {
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        result => result,
    }
}

/// Returns when words come for the resident `words` watches; never, when
/// the job is nobody's here.
async fn came(words: &mut Option<watch::Receiver<u64>>) {
    match words {
        // The node holds every sender, so the watch never closes.
        Some(words) => {
            let _ = words.changed().await;
        }
        None => std::future::pending().await,
    }
}

fn seconds(span: Duration) -> u32 {
    u32::try_from(span.as_secs()).unwrap_or(u32::MAX)
}

/// Whether a command's output says something was refused it, in the words
/// macOS gives what its privacy protection refuses.
fn refused_something(output: &OutputWindow) -> bool {
    output.pieces.iter().any(|piece| match piece {
        OutputPiece::Text { text, .. } | OutputPiece::PartOfLine { text, .. } => {
            text.contains("Operation not permitted")
        }
        _ => false,
    })
}

/// The action a `run` was, as its resident's `actions` record it.
pub fn action(reply: &RunReply, script: &str, at: jiff::Timestamp) -> Action {
    Action {
        turn: None,
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

/// Whether the answer can be given now. With no process left, the end is
/// moments away and the answer waits for it, so the resident is not also
/// told by letter of an end it has already read.
fn settled(record: &JobRecord) -> bool {
    record.end.is_some()
        || (record.exit.is_some()
            && (record.output_drained.is_some() || record.output_ended_at.is_some())
            && record.remaining.is_some_and(|n| n > 0))
}

pub(crate) fn state(record: &JobRecord) -> RunState {
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
        (None, Some(exit))
            if (record.output_drained.is_some() || record.output_ended_at.is_some())
                && record.remaining.is_some_and(|n| n > 0) =>
        {
            RunState::Exited { status: exit.status, remaining: record.remaining.expect("checked") }
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

/// One line naming the jobs of `resident` here that went on in the
/// background and still run, for the end of each answer of its World tools,
/// as 0.2's tools did: it keeps them in view after the conversation that
/// started them is compacted, and spares asking whether they still run.
/// `None` when there are none but `answering`, the job this answer is about.
pub fn still_running(
    world: &WorldDir,
    here: &fastctx_types::layout::Names,
    resident: &fastctx_types::ResidentId,
    answering: Option<&JobId>,
    elsewhere: Vec<(String, Timestamp, String)>,
) -> std::io::Result<Option<String>> {
    let owner = fastctx_types::run::JobOwner::Resident(fastctx_types::Address {
        machine: here.me.clone(),
        resident: resident.clone(),
    });
    let address = fastctx_types::Address { machine: here.me.clone(), resident: resident.clone() };
    let mut running = Vec::new();
    for jobs in world.job_directories(here, &address)? {
        for owed in read_dir(&jobs)? {
            if owed.extension().is_none_or(|extension| extension != "owed") {
                continue;
            }
            let job = match record::read(&owed.with_extension("json")) {
                Ok(job) => job,
                // Its record is being replaced; the next answer shows it.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if job.end.is_none() && job.owner == owner && Some(&job.id) != answering {
                running.push((job.id.to_string(), job.started_at, first_line(&job.script)));
            }
        }
    }
    running.extend(elsewhere);
    if running.is_empty() {
        return Ok(None);
    }
    running.sort_by_key(|(_, started_at, _)| *started_at);
    let now = Timestamp::now();
    let jobs: Vec<String> = running
        .iter()
        .map(|(job, started_at, command)| {
            let ran = now.as_second().saturating_sub(started_at.as_second()).max(0) as u64;
            format!("{job} ({}) {}", fastctx_hands::told::duration(ran), short(command))
        })
        .collect();
    Ok(Some(format!("=== your jobs still running: {} ===", jobs.join("; "))))
}

/// The accounts the jobs still running here of `owner`, a resident of
/// another machine, were sent for: an `on`'s hop, or a run handed over to
/// where it works.
pub(crate) fn running_for(
    world: &WorldDir,
    here: &fastctx_types::layout::Names,
    owner: &fastctx_types::Address,
) -> Vec<fastctx_types::access::HubAccount> {
    let directories = world.job_directories(here, owner).unwrap_or_default();
    let owner = fastctx_types::run::JobOwner::Resident(owner.clone());
    let mut accounts = Vec::new();
    for jobs in directories {
        let Ok(entries) = std::fs::read_dir(jobs) else { continue };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Ok(record) = record::read(&path) else { continue };
            if record.end.is_some() || record.owner != owner {
                continue;
            }
            if let Some(account) = record.on.and_then(|on| on.account).or(record.account)
                && !accounts.contains(&account)
            {
                accounts.push(account);
            }
        }
    }
    accounts
}

/// A command cut to what names it in a line of several.
fn short(command: &str) -> String {
    // Enough to tell jobs apart, few enough to repeat with every answer.
    const SHOWN: usize = 40;
    match command.char_indices().nth(SHOWN) {
        Some((cut, _)) => format!("{} …", command[..cut].trim_end()),
        None => command.to_string(),
    }
}

/// The machines a job's `on`s went to, as `on` noted them beside it.
pub(crate) fn visited(jobs: &Path, job: &JobId) -> Vec<(fastctx_types::MachineName, JobId)> {
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
pub(crate) async fn interrupt(path: &Path, reason: &str) -> Result<JobRecord, Failure> {
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

/// Marks jobs whose supervisor is gone, excluding this node's launches:
/// their records can precede the supervisor taking its lock.
pub fn reconcile(world: &WorldDir) -> std::io::Result<Reconciled> {
    let mut found = Reconciled::default();
    let starter = fastctx_platform::current_identity()?;
    for residents in world.job_parents()? {
        for resident in read_dir(&residents)? {
            for entry in read_dir(&resident.join("jobs"))? {
                if entry.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let checked = record::read(&entry).and_then(|job| {
                    if job.end.is_some() || job.program.as_ref().is_some_and(|program| program.starter == starter) {
                        return Ok(false);
                    }
                    interrupt_now(&entry, "its supervisor was gone when the node started").map(|job| job.end.is_some())
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

/// What failed before the job's process was started: nothing ran.
fn before_start(what: &str, error: std::io::Error) -> Failure {
    Failure::not_run(format!("{what}: {error}"))
}

/// Stops the job at `record_path`: its supervisor ends everything it
/// started and records who stopped it. Returns the job's final record; one
/// that had already ended is returned as it was.
pub async fn stop(node: &Arc<Node>, record_path: &Path, by: &str, reason: &str) -> Result<JobRecord, Failure> {
    let request = fastctx_types::run::StopRequest { by: by.to_string(), reason: reason.to_string() };
    run::stop(&node.world, record_path, None, request).await
}
