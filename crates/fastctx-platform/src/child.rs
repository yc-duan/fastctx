//! A program the node keeps as its own child, which lives no longer than the
//! node and takes what it started with it.
//!
//! The node drives some programs over their standard input and output for
//! as long as it runs, such as the Codex app-server. Such a program must
//! end when the node ends, however the node ends: an app-server left behind
//! by a node that crashed would keep its threads loaded, and the node that
//! starts next could not take them back. Its end is known from the process
//! itself, never from its output closing: a program it started may have
//! inherited its standard output and keep that open long after it is gone.
//!
//! On Windows the program is created inside a job that ends everything in
//! it when its last handle closes; the handle closes when this child is
//! dropped, or when the node exits for any reason. On Linux the program
//! leads a process group of its own and receives SIGKILL when the thread
//! that started it ends. That thread waits for it and lives as long as this
//! child, so the signal comes only when the node is gone; dropping the child
//! ends whatever is left of its process group. A program the child started
//! in a process group or session of its own is not reached on Linux.
//!
//! macOS has no death signal. The program leads a process group of its own
//! there too, and dropping the child ends it; a node that ends any other way
//! leaves the program its standard input, which closes with the node, and
//! the programs the node drives this way end when it does.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::ExitStatus;

/// A started program, its standard streams, and whatever it starts.
pub struct Child {
    pub stdin: Option<io::PipeWriter>,
    pub stdout: Option<io::PipeReader>,
    pub stderr: Option<io::PipeReader>,
    inner: imp::Child,
}

impl Child {
    /// Starts `program` with `args` in `cwd` with exactly the environment
    /// `env`, its standard streams piped to this process.
    pub fn spawn(program: &Path, args: &[&OsStr], cwd: &Path, env: &[(OsString, OsString)]) -> io::Result<Self> {
        let (inner, stdin, stdout, stderr) = imp::Child::spawn(program, args, cwd, env)?;
        Ok(Self { stdin: Some(stdin), stdout: Some(stdout), stderr: Some(stderr), inner })
    }

    pub fn pid(&self) -> u32 {
        self.inner.pid()
    }

    /// Waits for the program itself to exit. Blocks; may be called from
    /// several threads and again after it returned.
    pub fn wait(&self) -> io::Result<ExitStatus> {
        self.inner.wait()
    }

    /// Ends the program and what it started. Safe to call while another
    /// thread waits, and after it exited.
    pub fn kill(&self) -> io::Result<()> {
        self.inner.kill()
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::os::windows::io::{AsRawHandle as _, OwnedHandle};
    use std::os::windows::process::ExitStatusExt as _;
    use std::path::Path;
    use std::process::ExitStatus;

    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    use windows_sys::Win32::System::Threading::GetExitCodeProcess;

    use crate::windows_job::{Stdio, create_in_job, new_job, wait};

    pub struct Child {
        /// Closing it ends whatever is still in the job.
        job: OwnedHandle,
        process: OwnedHandle,
        pid: u32,
    }

    impl Child {
        pub fn spawn(
            program: &Path,
            args: &[&OsStr],
            cwd: &Path,
            env: &[(OsString, OsString)],
        ) -> io::Result<(Self, io::PipeWriter, io::PipeReader, io::PipeReader)> {
            let job = new_job()?;
            let (stdin_reader, stdin_writer) = io::pipe()?;
            let (stdout_reader, stdout_writer) = io::pipe()?;
            let (stderr_reader, stderr_writer) = io::pipe()?;
            let stdio = Stdio {
                input: stdin_reader.as_raw_handle(),
                output: stdout_writer.as_raw_handle(),
                error: stderr_writer.as_raw_handle(),
            };
            let (process, pid) = create_in_job(&job, program, args, cwd, env, &stdio)?;
            // The child's ends are the child's alone from here: the readers
            // see the end of its output only once these copies are gone.
            drop((stdin_reader, stdout_writer, stderr_writer));
            Ok((Self { job, process, pid }, stdin_writer, stdout_reader, stderr_reader))
        }

        pub fn pid(&self) -> u32 {
            self.pid
        }

        pub fn wait(&self) -> io::Result<ExitStatus> {
            wait(&self.process)?;
            let mut code = 0;
            // SAFETY: a process handle that has signalled.
            if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(ExitStatus::from_raw(code))
        }

        pub fn kill(&self) -> io::Result<()> {
            // SAFETY: this child owns the job and everything in it.
            if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::os::fd::OwnedFd;
    use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
    use std::path::Path;
    use std::process::{Command, ExitStatus, Stdio};
    use std::sync::{Arc, Condvar, Mutex};

    use crate::unix_fds::close_on_exec_from;

    pub struct Child {
        pid: u32,
        shared: Arc<Shared>,
    }

    struct Shared {
        state: Mutex<State>,
        changed: Condvar,
    }

    #[derive(Default)]
    struct State {
        /// Set once the program exited. It is not reaped until the child is
        /// dropped, so its PID, which is also its process group's, cannot
        /// be taken by another process while signals may still be sent to
        /// it.
        exited: Option<ExitStatus>,
        /// Set when the keeper could not wait for the program.
        failed: Option<String>,
        /// Set when the child is dropped.
        released: bool,
    }

    impl Child {
        pub fn spawn(
            program: &Path,
            args: &[&OsStr],
            cwd: &Path,
            env: &[(OsString, OsString)],
        ) -> io::Result<(Self, io::PipeWriter, io::PipeReader, io::PipeReader)> {
            let mut command = Command::new(program);
            command
                .args(args)
                .current_dir(cwd)
                .env_clear()
                .envs(env.iter().map(|(name, value)| (name, value)))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(target_os = "linux")]
            let parent = std::process::id();
            // SAFETY: setpgid, prctl, getppid and the descriptor marking are
            // async-signal-safe.
            unsafe {
                command.pre_exec(move || {
                    if libc::setpgid(0, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    #[cfg(target_os = "linux")]
                    {
                        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) == -1 {
                            return Err(io::Error::last_os_error());
                        }
                        // The parent may have died before the signal was asked
                        // for; then nothing would ever send it.
                        if libc::getppid() as u32 != parent {
                            return Err(io::Error::other("the parent exited while the program started"));
                        }
                    }
                    close_on_exec_from(3)
                });
            }
            let shared = Arc::new(Shared { state: Mutex::new(State::default()), changed: Condvar::new() });
            // The program is started on the thread that waits for it: the
            // kernel sends the death signal when the thread that started a
            // process ends, and this thread ends only once the program is
            // gone or the node is.
            let (started, starting) = std::sync::mpsc::channel();
            let keeper = shared.clone();
            std::thread::Builder::new().name("child keeper".to_string()).spawn(move || {
                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(error) => {
                        let _ = started.send(Err(error));
                        return;
                    }
                };
                let pid = child.id();
                let streams = (child.stdin.take(), child.stdout.take(), child.stderr.take());
                let _ = started.send(Ok((pid, streams)));
                drop(command);
                keep(&keeper, pid);
            })?;
            let (pid, (stdin, stdout, stderr)) = starting
                .recv()
                .map_err(|_| io::Error::other("the keeper thread ended before the program started"))??;
            let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
                return Err(io::Error::other("the program's streams were not piped"));
            };
            Ok((
                Self { pid, shared },
                io::PipeWriter::from(OwnedFd::from(stdin)),
                io::PipeReader::from(OwnedFd::from(stdout)),
                io::PipeReader::from(OwnedFd::from(stderr)),
            ))
        }

        pub fn pid(&self) -> u32 {
            self.pid
        }

        pub fn wait(&self) -> io::Result<ExitStatus> {
            let mut state = self.shared.state.lock().expect("never poisoned");
            loop {
                if let Some(status) = state.exited {
                    return Ok(status);
                }
                if let Some(failed) = &state.failed {
                    return Err(io::Error::other(failed.clone()));
                }
                state = self.shared.changed.wait(state).expect("never poisoned");
            }
        }

        pub fn kill(&self) -> io::Result<()> {
            // Held while signalling, so the keeper cannot reap the program
            // and free its PID in between.
            let _state = self.shared.state.lock().expect("never poisoned");
            // SAFETY: a signal to the program's own process group, which
            // exists as long as the program is unreaped.
            if unsafe { libc::killpg(self.pid as libc::pid_t, libc::SIGKILL) } == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
            Ok(())
        }
    }

    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.kill();
            self.shared.state.lock().expect("never poisoned").released = true;
            self.shared.changed.notify_all();
        }
    }

    /// Waits for the program to exit without reaping it, records how it
    /// exited, and once the child is dropped ends what is left of its
    /// process group and reaps it.
    fn keep(shared: &Shared, pid: u32) {
        let exited = loop {
            // SAFETY: zeroed is a valid siginfo for an out-parameter.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: waits for this process's own child, leaving it
            // unreaped.
            let waited =
                unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) };
            if waited == 0 {
                break Ok(status_of(&info));
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINTR) {
                break Err(error.to_string());
            }
        };
        let mut state = shared.state.lock().expect("never poisoned");
        match exited {
            Ok(status) => state.exited = Some(status),
            Err(error) => state.failed = Some(format!("waiting for the program: {error}")),
        }
        shared.changed.notify_all();
        while !state.released {
            state = shared.changed.wait(state).expect("never poisoned");
        }
        // SAFETY: the group is the program's own, and the program is not yet
        // reaped, so its PID is still its own.
        unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
        let mut status = 0;
        // SAFETY: reaps this process's own child.
        unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
    }

    /// How the program exited, in the form `wait` reports it.
    fn status_of(info: &libc::siginfo_t) -> ExitStatus {
        // SAFETY: waitid filled in a child-state record.
        let (code, status) = unsafe { (info.si_code, info.si_status()) };
        match code {
            libc::CLD_EXITED => ExitStatus::from_raw((status & 0xff) << 8),
            libc::CLD_DUMPED => ExitStatus::from_raw((status & 0x7f) | 0x80),
            _ => ExitStatus::from_raw(status & 0x7f),
        }
    }
}
