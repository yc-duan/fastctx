//! A job's supervisor: one process per job, from its first moment to its
//! last.
//!
//! It starts the job's shell as a process tree, copies every byte of output
//! into the log, and keeps the job's record: when the shell started, when
//! and how it exited, when the output ended, when nothing of the job ran
//! any more. After each change it tells the node, which may be waiting to
//! answer the `run` that started the job, or to tell the resident that it
//! ended. If the node is not there to hear, the record still holds it, and
//! the node reads it when it starts.

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fastctx_client::Client;
use fastctx_platform::{FileLock, ProcessTree, current_identity, in_job};
use fastctx_types::Call;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::run::{Ended, EndedHow, ExitStatus, Exited, JobRecord, OutputLost};
use jiff::Timestamp;

use super::{record, shell};

/// Runs the job whose first record the node wrote at `record_path`, in the
/// World kept in `world`, until nothing of it runs any more.
pub fn supervise(world: &Path, record_path: &Path, fastctx_dir: &Path) -> io::Result<()> {
    supervise_with(world, record_path, |world, record| {
        let resident = world.job_owner(&record.machine, record.town.as_ref(), &record.owner);
        let (program, args) = shell::command(&resident.job_script(&record.id))?;
        let fastctx_home = std::env::var_os("FASTCTX_HOME");
        let environment = shell::environment(&shell::Context {
            machine: &record.machine,
            owner: &record.owner,
            town: record.town.as_ref(),
            fastctx_dir,
            fastctx_home: fastctx_home.as_deref(),
            job: Some(&record.id),
        })?;
        Ok(shell::Prepared { program, args, environment })
    })
}

/// Supervises a complete process tree with a caller-resolved shell. The
/// lifecycle and durable record are the same as for the default login shell.
pub fn supervise_with(
    world: &Path,
    record_path: &Path,
    prepare: impl FnOnce(&WorldDir, &JobRecord) -> io::Result<shell::Prepared>,
) -> io::Result<()> {
    let world = WorldDir::at(world);
    let _lock = FileLock::try_acquire(&record::lock_path(record_path))?
        .ok_or_else(|| io::Error::other(format!("another supervisor holds {}", record_path.display())))?;
    let mut job = Job { record: record::read(record_path)?, path: record_path.to_path_buf(), world };
    if let Err(error) = job.run(prepare) {
        // The job could not be started, or its supervision broke down: say
        // so in the record, whatever state it had reached.
        job.record.end = Some(Ended { at: Timestamp::now(), how: EndedHow::Failed { error: error.to_string() } });
        job.publish()?;
        return Err(error);
    }
    Ok(())
}

struct Job {
    record: JobRecord,
    path: PathBuf,
    world: WorldDir,
}

impl Job {
    fn run(&mut self, prepare: impl FnOnce(&WorldDir, &JobRecord) -> io::Result<shell::Prepared>) -> io::Result<()> {
        #[cfg(unix)]
        let stop_signals = signals::block()?;
        let shell = prepare(&self.world, &self.record)?;
        let args: Vec<&std::ffi::OsStr> = shell.args.iter().map(|arg| arg.as_os_str()).collect();
        let log = OpenOptions::new().append(true).open(&self.record.output)?;

        self.record.supervisor = Some(current_identity()?);
        self.record.ends_with_node = in_job()?;
        let (tree, output) =
            ProcessTree::spawn(&shell.program, &args, Path::new(&self.record.cwd), &shell.environment)?;
        let tree = Arc::new(tree);
        self.record.pid = Some(tree.main_pid());
        self.publish()?;

        let stopped = Arc::new(AtomicBool::new(false));
        #[cfg(unix)]
        signals::stop_on_signal(stop_signals, tree.clone(), stopped.clone());
        let copier = std::thread::spawn(move || copy_output(output, log));

        let status = tree.wait_main()?;
        self.record.exit = Some(Exited { status: exit_status(status), at: Timestamp::now() });
        self.publish()?;

        let lost = copier.join().map_err(|_| io::Error::other("the output copier panicked"))??;
        self.record.output_ended_at = Some(Timestamp::now());
        self.record.output_lost = lost;
        let remaining = tree.count()?;
        self.record.remaining = Some(u32::try_from(remaining).unwrap_or(u32::MAX));
        // With nothing left running the job is over; the end is written in
        // the same record, so nobody reads it as still going.
        if remaining > 0 {
            self.publish()?;
        }

        tree.wait_empty()?;
        let how = if stopped.load(Ordering::SeqCst) { EndedHow::Stopped } else { EndedHow::Finished };
        self.record.end = Some(Ended { at: Timestamp::now(), how });
        self.publish()
    }

    /// Writes the record and tells the node it changed.
    fn publish(&self) -> io::Result<()> {
        record::write(&self.path, &self.record)?;
        tell_node(&self.world, &self.record);
        Ok(())
    }
}

/// Copies the output into the log until every writer has closed it. If the
/// log stops taking bytes, the rest is read and dropped, so no process of
/// the job ever blocks on a pipe nobody reads.
fn copy_output(mut output: io::PipeReader, mut log: File) -> io::Result<Option<OutputLost>> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut kept = 0u64;
    let mut lost = None;
    loop {
        let read = match output.read(&mut buffer) {
            Ok(0) => return Ok(lost),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            // A broken pipe is how Windows reports that every writer closed.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(lost),
            Err(error) => return Err(error),
        };
        if lost.is_none() {
            match log.write_all(&buffer[..read]) {
                Ok(()) => kept += read as u64,
                Err(error) => lost = Some(OutputLost { kept_bytes: kept, error: error.to_string() }),
            }
        }
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

/// Windows exit codes are 32 bits, kept unsigned.
#[cfg(windows)]
fn exit_status(status: std::process::ExitStatus) -> ExitStatus {
    let code = status.code().expect("a Windows process always has an exit code");
    ExitStatus::Code(i64::from(code as u32))
}

/// Tells the node the record changed. A node that is not running, or not
/// reachable, reads the record when it starts; the record is the truth and
/// this is only a prompt, so its failure is not the job's.
fn tell_node(world: &WorldDir, record: &JobRecord) {
    let call = Call::JobChanged { job: record.id.clone(), owner: record.owner.clone(), town: record.town.clone() };
    let peer = Peer::Supervisor { job: record.id.clone() };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return;
    };
    runtime.block_on(async {
        if let Ok(mut client) = Client::connect(world, peer).await {
            let _ = client.call(call).await;
        }
    });
}

#[cfg(unix)]
mod signals {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use fastctx_platform::ProcessTree;

    /// Blocks the signals that ask the supervisor to stop, in this thread and
    /// every thread it starts, so one thread can wait for them. The shell
    /// does not inherit the block: a new program starts with none.
    pub fn block() -> io::Result<libc::sigset_t> {
        // SAFETY: building and installing a signal mask for this thread.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
                libc::sigaddset(&mut set, signal);
            }
            let code = libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code));
            }
            Ok(set)
        }
    }

    /// When asked to stop, stops the whole job and remembers that it did.
    pub fn stop_on_signal(set: libc::sigset_t, tree: Arc<ProcessTree>, stopped: Arc<AtomicBool>) {
        std::thread::spawn(move || {
            let mut signal = 0;
            // SAFETY: waits for one of the blocked signals.
            if unsafe { libc::sigwait(&set, &mut signal) } == 0 {
                // Marked first, so the job reads as stopped however soon its
                // last process is gone; unmarked if it could not be stopped
                // and runs on.
                stopped.store(true, Ordering::SeqCst);
                if tree.kill().is_err() {
                    stopped.store(false, Ordering::SeqCst);
                }
            }
        });
    }
}
