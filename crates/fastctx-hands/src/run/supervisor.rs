//! One supervisor per job, owning its tree, output and durable record.
//! Every record change prompts the node through JobChanged; a node absent
//! at that moment reads the record when it returns.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_client::Client;
use fastctx_platform::ipc::Connection;
use fastctx_platform::{FileLock, ProcessTree, current_identity, in_job};
use fastctx_types::Call;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::{ConversationDir, WorldDir};
use fastctx_types::run::{
    Ended, EndedHow, ExitStatus, Exited, JobRecord, OutputDrained, OutputLost, OutputMode, StopRequest, StopRequested,
};
use jiff::Timestamp;
use tokio::sync::mpsc;

use super::{control, output, record, shell};

/// Runs the job at `record_path` until nothing of it runs any more.
pub fn supervise(world: &Path, record_path: &Path, fastctx_dir: &Path) -> io::Result<()> {
    supervise_with(world, record_path, |_, record| {
        let resident = room(record_path)?;
        shell::ensure_prelude(fastctx_dir)?;
        let fastctx_home = std::env::var_os("FASTCTX_HOME");
        let scratch = resident.scratch();
        let job_temp = resident.job_scratch(&record.id);
        fastctx_platform::ensure_private_dir(&scratch)?;
        fastctx_platform::ensure_private_dir(&job_temp)?;
        let on_state = record.on.as_ref().map(|on| std::path::PathBuf::from(&on.state));
        shell::prepare(
            &resident.job_script(&record.id),
            &shell::Context {
                machine: &record.machine,
                owner: &record.owner,
                town: record.town.as_ref(),
                fastctx_dir,
                fastctx_home: fastctx_home.as_deref(),
                job: Some(&record.id),
                scratch: Some(&scratch),
                job_temp: Some(&job_temp),
                writes_off: record.writes_off,
                on_state: on_state.as_deref(),
            },
        )
    })
}

/// The room of the conversation a job is for: the one its record is kept
/// in. Worked out again from the owner's name it could be another, as the
/// room of a machine renamed since stays under the name it had.
/// The room whose scratch directory the supervisor gives this job.
pub fn room(record_path: &Path) -> io::Result<ConversationDir> {
    record_path
        .parent()
        .and_then(Path::parent)
        .map(ConversationDir::at)
        .ok_or_else(|| io::Error::other(format!("{} is in no conversation's jobs", record_path.display())))
}

/// Supervises a complete tree with a caller-resolved shell. Lifecycle,
/// control and durable records are the same as for the default login shell.
pub fn supervise_with(
    world: &Path,
    record_path: &Path,
    prepare: impl FnOnce(&WorldDir, &JobRecord) -> io::Result<shell::Prepared>,
) -> io::Result<()> {
    let world = WorldDir::at(world);
    let _lock = FileLock::try_acquire(&record::lock_path(record_path))?
        .ok_or_else(|| io::Error::other(format!("another supervisor holds {}", record_path.display())))?;
    let mut job =
        Job { record: record::read(record_path)?, path: record_path.to_path_buf(), world, waiting: Vec::new() };
    if job.record.end.is_some() {
        return Err(io::Error::other("cannot supervise an already ended job"));
    }
    #[cfg(unix)]
    let stop_signals = signals::block()?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let result = job
            .run(
                prepare,
                #[cfg(unix)]
                stop_signals,
            )
            .await;
        if let Err(error) = &result {
            job.record.end = Some(Ended { at: Timestamp::now(), how: EndedHow::Failed { error: error.to_string() } });
            job.publish().await?;
        }
        // Closing is the receipt: each caller reads the final record. The
        // supervisor never depends on a caller consuming response bytes.
        job.waiting.clear();
        result
    })
}

struct Job {
    record: JobRecord,
    path: PathBuf,
    world: WorldDir,
    waiting: Vec<Connection>,
}

enum Event {
    Main(io::Result<std::process::ExitStatus>),
    Drained {
        stderr: bool,
        bytes: u64,
        lost: Option<OutputLost>,
    },
    Remaining(io::Result<usize>),
    Output {
        stderr: bool,
        result: io::Result<Option<OutputLost>>,
    },
    Empty(io::Result<()>),
    #[cfg(unix)]
    Stop(io::Result<StopRequest>),
}

impl Job {
    async fn run(
        &mut self,
        prepare: impl FnOnce(&WorldDir, &JobRecord) -> io::Result<shell::Prepared>,
        #[cfg(unix)] stop_signals: libc::sigset_t,
    ) -> io::Result<()> {
        // Its account's share of the processors, where the node gave one:
        // joined before anything of the job starts.
        if let Some(dir) = std::env::var_os(fastctx_platform::cpu_share::VARIABLE) {
            let _ = fastctx_platform::cpu_share::join(Path::new(&dir));
        }
        let mut server = control::Server::bind(&self.world, &self.record)?;
        let shell = prepare(&self.world, &self.record)?;
        let args: Vec<&std::ffi::OsStr> = shell.args.iter().map(|arg| arg.as_os_str()).collect();
        let log = OpenOptions::new().append(true).open(&self.record.output)?;
        let stderr = self.record.stderr.as_ref().map(|path| OpenOptions::new().append(true).open(path)).transpose()?;
        let mode = if stderr.is_some() { OutputMode::Separate } else { OutputMode::Merged };
        self.record.supervisor = Some(current_identity()?);
        self.record.ends_with_node = in_job()?;
        // The control endpoint exists before this identity is published.
        // A node stopping a just-started job can therefore address it even
        // before the shell has been spawned.
        record::write(&self.path, &self.record)?;
        if let Some(request) = self.record.on.as_ref().and_then(|on| on.request.as_ref()) {
            let directory = room(&self.path)?;
            if directory.jobs().join(format!("request-{request}.stopped")).try_exists()? {
                self.record.stop = Some(StopRequested {
                    at: Timestamp::now(),
                    request: StopRequest {
                        by: self.record.owner.to_string(),
                        reason: "the on that sent it stopped".to_string(),
                    },
                });
                self.record.end = Some(Ended { at: Timestamp::now(), how: EndedHow::Stopped });
                return self.publish().await;
            }
        }
        let (tree, output) = if self.record.is_sentinel() {
            super::sentinel::spawn(&shell, &self.record, self.world.path())?
        } else {
            ProcessTree::spawn_with_io(
                &shell.program,
                &args,
                Path::new(&self.record.cwd),
                &shell.environment,
                mode,
                self.record.stdin.as_deref().map(Path::new),
            )?
        };
        let tree = Arc::new(tree);
        self.record.pid = Some(tree.main_pid());
        let (sender, mut events) = mpsc::unbounded_channel();
        let (resume, after_drain) = std::sync::mpsc::channel();
        let mut resume = Some(resume);
        let mut checkpoints = vec![output.stdout.checkpoint()];
        let mut workers = Vec::new();
        let output_limit = self.record.is_sentinel().then_some(65_536);
        let mut streams = 1;
        // A thread the system will not start leaves the shell running with
        // no one to wait for it: what started is stopped, and the job fails.
        let started = (|| -> io::Result<()> {
            let (waiting, reaped) = (tree.clone(), sender.clone());
            workers.push(spawn("fastctx-job-wait", move || {
                let _ = reaped.send(Event::Main(waiting.wait_main()));
                // Keep reaping and counting on one thread. Dropping the gate
                // also resumes reaping, including during stop/error cleanup.
                if after_drain.recv().is_ok() {
                    let _ = reaped.send(Event::Remaining(waiting.remaining()));
                }
                let _ = reaped.send(Event::Empty(waiting.wait_empty()));
            })?);
            if let (Some(reader), Some(log)) = (output.stderr, stderr) {
                streams += 1;
                checkpoints.push(reader.checkpoint());
                let sender = sender.clone();
                workers.push(spawn("fastctx-job-stderr", move || {
                    let result = output::copy(reader, log, output_limit, |bytes, lost| {
                        let _ = sender.send(Event::Drained { stderr: true, bytes, lost });
                    });
                    let _ = sender.send(Event::Output { stderr: true, result });
                })?);
            }
            let sender_out = sender.clone();
            workers.push(spawn("fastctx-job-stdout", move || {
                let result = output::copy(output.stdout, log, output_limit, |bytes, lost| {
                    let _ = sender_out.send(Event::Drained { stderr: false, bytes, lost });
                });
                let _ = sender_out.send(Event::Output { stderr: false, result });
            })?);
            #[cfg(unix)]
            signals::stop_on_signal(stop_signals, sender.clone())?;
            Ok(())
        })();
        drop(sender);
        if let Err(error) = started {
            drop(resume);
            let cleanup = tree.kill();
            for worker in workers {
                let _ = worker.join();
            }
            return Err(match cleanup {
                Ok(()) => io::Error::other(format!("starting the job's supervision: {error}")),
                Err(cleanup) => io::Error::other(format!(
                    "starting the job's supervision: {error}; stopping its processes also failed: {cleanup}"
                )),
            });
        }
        let result = self.observe(&tree, &mut server, &mut events, streams, &checkpoints, &mut resume).await;
        drop(resume);
        if let Err(error) = &result {
            // Linux has no kill-on-close tree handle. Clean up explicitly,
            // and never report a successful stop if cleanup itself failed.
            if let Err(cleanup) = tree.kill() {
                return Err(io::Error::other(format!(
                    "{error}; stopping the remaining processes also failed: {cleanup}"
                )));
            }
        }
        for worker in workers {
            worker.join().map_err(|_| io::Error::other("a job supervisor worker panicked"))?;
        }
        result
    }

    async fn observe(
        &mut self,
        tree: &ProcessTree,
        server: &mut control::Server,
        events: &mut mpsc::UnboundedReceiver<Event>,
        mut streams: usize,
        checkpoints: &[fastctx_platform::output_pipe::Checkpoint],
        resume: &mut Option<std::sync::mpsc::Sender<()>>,
    ) -> io::Result<()> {
        self.publish().await?;
        let mut empty = false;
        let mut drained_out = None;
        let mut drained_err = (streams == 1).then_some(0);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(deadline), if self.record.is_sentinel() && self.record.stop.is_none() => {
                    resume.take();
                    self.stop(tree, StopRequest {by:"sentinel".into(),reason:"observation exceeded 30 seconds".into()}).await?;
                }
                request = server.next() => {
                    let (connection, request) = request?;
                    self.waiting.push(connection);
                    resume.take();
                    self.stop(tree, request).await?;
                }
                event = events.recv() => {
                    let mut event = event.ok_or_else(|| io::Error::other("the job's lifecycle channel closed before it ended"))?;
                    loop {
                        match event {
                            Event::Main(status) => {
                                self.record.exit = Some(Exited { status: exit_status(status?), at: Timestamp::now() });
                                for checkpoint in checkpoints {
                                    checkpoint.request()?;
                                }
                            }
                            Event::Drained { stderr, bytes, lost } => {
                                if stderr {
                                    drained_err = Some(bytes);
                                    self.record.stderr_lost = lost;
                                } else {
                                    drained_out = Some(bytes);
                                    self.record.output_lost = lost;
                                }
                            }
                            Event::Remaining(count) => {
                                self.record.remaining = Some(u32::try_from(count?).map_err(io::Error::other)?);
                            }
                            Event::Output { stderr, result } => {
                                if stderr { self.record.stderr_lost = result?; } else { self.record.output_lost = result?; }
                                streams -= 1;
                                if streams == 0 {
                                    self.record.output_ended_at = Some(Timestamp::now());
                                }
                            }
                            Event::Empty(result) => { result?; empty = true; }
                            #[cfg(unix)]
                            Event::Stop(request) => {
                                resume.take();
                                self.stop(tree, request?).await?;
                            },
                        }
                        if self.record.exit.is_some()
                            && let (Some(stdout), Some(stderr)) = (drained_out, drained_err)
                        {
                            self.record.output_drained = Some(OutputDrained { stdout, stderr });
                            if let Some(resume) = resume.take() {
                                resume.send(()).map_err(|_| io::Error::other("the job's process waiter closed"))?;
                            }
                        }
                        if empty && streams == 0 {
                            self.record.remaining = Some(0);
                            let how = if self.record.stop.is_some() { EndedHow::Stopped } else { EndedHow::Finished };
                            self.record.end = Some(Ended { at: Timestamp::now(), how });
                            return self.publish().await;
                        }
                        // The record is a snapshot. Include events already queued
                        // before publishing it, without delaying for future events.
                        match events.try_recv() {
                            Ok(next) => event = next,
                            Err(_) => break,
                        }
                    }
                    self.publish().await?;
                }
            }
        }
    }

    async fn stop(&mut self, tree: &ProcessTree, request: StopRequest) -> io::Result<()> {
        if self.record.stop.is_none() {
            self.record.stop = Some(StopRequested { at: Timestamp::now(), request });
            // Attribution survives even a failure while terminating the tree.
            self.publish().await?;
            tree.kill()?;
        }
        Ok(())
    }

    async fn publish(&self) -> io::Result<()> {
        record::write(&self.path, &self.record)?;
        tell_node(&self.world, &self.record).await;
        Ok(())
    }
}

#[cfg(unix)]
fn exit_status(status: std::process::ExitStatus) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    match status.signal() {
        Some(signal) => ExitStatus::Signal(signal),
        None => ExitStatus::Code(i64::from(status.code().expect("a Unix status is a code or a signal"))),
    }
}

#[cfg(windows)]
fn exit_status(status: std::process::ExitStatus) -> ExitStatus {
    ExitStatus::Code(i64::from(status.code().expect("a Windows process always has an exit code") as u32))
}

/// The record is authoritative. An absent node reads it on startup, so a
/// failed prompt is not a failure of the job.
async fn tell_node(world: &WorldDir, record: &JobRecord) {
    let call = Call::JobChanged { job: record.id.clone(), owner: record.owner.clone(), town: record.town.clone() };
    let peer = Peer::Supervisor { job: record.id.clone() };
    if let Ok(mut client) = Client::connect(world, peer).await {
        let _ = client.call(call).await;
    }
}

/// Starts a named thread, saying so when the system will not, rather than
/// failing the supervisor.
fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name(name.into()).spawn(work)
}

#[cfg(unix)]
mod signals {
    use std::io;

    const STOP: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

    /// The main thread and every thread it creates block these signals, so
    /// only the thread waiting for them takes them. ProcessTree resets the
    /// child's mask.
    pub fn block() -> io::Result<libc::sigset_t> {
        // SAFETY: building a signal set in place.
        let set = unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signal in STOP {
                libc::sigaddset(&mut set, signal);
            }
            set
        };
        mask(libc::SIG_BLOCK, &set)?;
        Ok(set)
    }

    fn mask(how: libc::c_int, set: &libc::sigset_t) -> io::Result<()> {
        // SAFETY: changes this thread's signal mask by a valid set.
        let code = unsafe { libc::pthread_sigmask(how, set, std::ptr::null_mut()) };
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code));
        }
        Ok(())
    }

    pub fn stop_on_signal(
        set: libc::sigset_t,
        sender: tokio::sync::mpsc::UnboundedSender<super::Event>,
    ) -> std::io::Result<()> {
        super::spawn("fastctx-job-signal", move || {
            let result = wait(&set).map(|info| {
                // SAFETY: the kernel filled in who sent the signal.
                let (pid, uid) = unsafe { (info.si_pid(), info.si_uid()) };
                fastctx_types::run::StopRequest {
                    by: format!("process {pid} (uid {uid})"),
                    reason: format!("signal {}", info.si_signo),
                }
            });
            let _ = sender.send(super::Event::Stop(result));
        })
        .map(drop)
    }

    /// One of the blocked signals, as the kernel describes it.
    #[cfg(target_os = "linux")]
    fn wait(set: &libc::sigset_t) -> io::Result<libc::siginfo_t> {
        // SAFETY: an out-parameter for one of the blocked signals.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        while unsafe { libc::sigwaitinfo(set, &mut info) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        Ok(info)
    }

    /// The pipe's end the handler writes to, set before it is installed.
    #[cfg(not(target_os = "linux"))]
    static TOLD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

    /// Passes on, whole, what the kernel tells a handler about its signal.
    #[cfg(not(target_os = "linux"))]
    extern "C" fn pass_on(_: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
        // SAFETY: an SA_SIGINFO handler is given a valid siginfo, and write is
        // async-signal-safe; errno is put back for the code it interrupted.
        unsafe {
            let errno = *libc::__error();
            let told = TOLD.load(std::sync::atomic::Ordering::Relaxed);
            libc::write(told, info.cast_const().cast(), std::mem::size_of::<libc::siginfo_t>());
            *libc::__error() = errno;
        }
    }

    /// One of the blocked signals, as the kernel describes it. macOS has no
    /// sigwaitinfo, but tells a signal's handler who sent it: this thread
    /// alone leaves the signals unblocked, so the handler runs here and
    /// passes the description on through a pipe.
    #[cfg(not(target_os = "linux"))]
    fn wait(set: &libc::sigset_t) -> io::Result<libc::siginfo_t> {
        use std::io::Read as _;
        use std::os::fd::AsRawFd as _;
        let (mut told, teller) = std::io::pipe()?;
        // A full pipe loses a signal instead of stopping its handler; the
        // first one is all that is read.
        // SAFETY: sets a flag on a descriptor this function owns.
        if unsafe { libc::fcntl(teller.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
            return Err(io::Error::last_os_error());
        }
        TOLD.store(teller.as_raw_fd(), std::sync::atomic::Ordering::Relaxed);
        // SAFETY: the handler only writes to the pipe, which stays open until
        // the signals are blocked again.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = pass_on as extern "C" fn(_, _, _) as libc::sighandler_t;
            action.sa_flags = libc::SA_SIGINFO;
            for signal in STOP {
                if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        mask(libc::SIG_UNBLOCK, set)?;
        // SAFETY: siginfo_t is plain data, which the handler writes whole.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let bytes = unsafe {
            std::slice::from_raw_parts_mut((&raw mut info).cast::<u8>(), std::mem::size_of::<libc::siginfo_t>())
        };
        let read = told.read_exact(bytes);
        // Blocked again before the pipe closes, the signals reach no handler.
        mask(libc::SIG_BLOCK, set)?;
        read.map(|()| info)
    }
}
