//! A program together with every process it starts.
//!
//! The World's `run` starts a shell and answers for everything that shell
//! starts: it must know when the shell exits, when the last process it left
//! behind is gone and how many are still running, and it must be able to
//! stop them all. The tree's output is one stream, standard output and
//! standard error together in the order they were written, as a terminal
//! would show them; nothing reads standard input.
//!
//! On Linux the process holding the tree becomes a child subreaper: a
//! process whose parent exits is handed to it instead of to init, so the
//! tree is exactly its descendants, and once it has no children left the
//! tree is empty. A process holds at most one tree.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::ExitStatus;

/// A started program and everything it starts.
pub struct ProcessTree(imp::Tree);

impl ProcessTree {
    /// Starts `program` with `args` in `cwd` with exactly the environment
    /// `env`. Returns the tree and the reading end of its output.
    pub fn spawn(
        program: &Path,
        args: &[&OsStr],
        cwd: &Path,
        env: &[(OsString, OsString)],
    ) -> io::Result<(Self, io::PipeReader)> {
        imp::Tree::spawn(program, args, cwd, env).map(|(tree, output)| (Self(tree), output))
    }

    /// The PID of the program itself.
    pub fn main_pid(&self) -> u32 {
        self.0.main_pid()
    }

    /// Waits for the program itself to exit.
    pub fn wait_main(&self) -> io::Result<ExitStatus> {
        self.0.wait_main()
    }

    /// Waits until no process of the tree remains, after `wait_main`.
    pub fn wait_empty(&self) -> io::Result<()> {
        self.0.wait_empty()
    }

    /// How many processes of the tree are running now.
    pub fn count(&self) -> io::Result<usize> {
        self.0.count()
    }

    /// Ends every process of the tree. Safe to call while another thread
    /// waits.
    pub fn kill(&self) -> io::Result<()> {
        self.0.kill()
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
    use std::path::Path;
    use std::process::{Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use crate::procfs::{self, Stat};
    use crate::unix_fds::close_on_exec_from;

    pub struct Tree {
        main: u32,
    }

    static HOLDS_A_TREE: AtomicBool = AtomicBool::new(false);

    impl Tree {
        pub fn spawn(
            program: &Path,
            args: &[&OsStr],
            cwd: &Path,
            env: &[(OsString, OsString)],
        ) -> io::Result<(Self, io::PipeReader)> {
            // Waiting for any child is how the tree is waited for, so a
            // second tree in the same process would steal the first's.
            if HOLDS_A_TREE.swap(true, Ordering::SeqCst) {
                return Err(io::Error::other("a process holds at most one process tree"));
            }
            // SAFETY: a plain prctl on this process.
            if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == -1 {
                return Err(io::Error::last_os_error());
            }
            let (reader, writer) = io::pipe()?;
            let mut command = Command::new(program);
            command
                .args(args)
                .current_dir(cwd)
                .env_clear()
                .envs(env.iter().map(|(name, value)| (name, value)))
                .stdin(Stdio::null())
                .stdout(writer.try_clone()?)
                .stderr(writer);
            // SAFETY: setsid and the descriptor marking are async-signal-safe.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    close_on_exec_from(3)
                });
            }
            let main = command.spawn()?.id();
            // The command keeps copies of the pipe's writing end; the reader
            // sees the end of the output only once they are gone.
            drop(command);
            Ok((Self { main }, reader))
        }

        pub fn main_pid(&self) -> u32 {
            self.main
        }

        pub fn wait_main(&self) -> io::Result<ExitStatus> {
            // Processes the program left behind may end first; they are this
            // process's children too and are reaped on the way.
            loop {
                match reap_one()? {
                    Some((pid, status)) if pid == self.main => return Ok(ExitStatus::from_raw(status)),
                    Some(_) => continue,
                    None => return Err(io::Error::other("the program was reaped before it was waited for")),
                }
            }
        }

        pub fn wait_empty(&self) -> io::Result<()> {
            while reap_one()?.is_some() {}
            Ok(())
        }

        pub fn count(&self) -> io::Result<usize> {
            Ok(descendants()?.len())
        }

        pub fn kill(&self) -> io::Result<()> {
            // Each round kills every process found; one started meanwhile by
            // a process not yet killed is found in the next round.
            loop {
                let processes = descendants()?;
                if processes.is_empty() {
                    return Ok(());
                }
                for process in processes {
                    signal(process, libc::SIGKILL)?;
                }
                // A process in uninterruptible sleep dies only when it wakes;
                // the next round should not spin on it.
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// Reaps one child of this process, waiting for it; `None` once this
    /// process has no children.
    fn reap_one() -> io::Result<Option<(u32, i32)>> {
        loop {
            let mut status = 0;
            // SAFETY: waits for any child of this process.
            let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
            if pid > 0 {
                return Ok(Some((pid as u32, status)));
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ECHILD) => return Ok(None),
                Some(libc::EINTR) => continue,
                _ => return Err(error),
            }
        }
    }

    /// Every running descendant of this process.
    fn descendants() -> io::Result<Vec<Stat>> {
        let processes = procfs::all()?;
        let mut found = Vec::new();
        let mut parents = vec![std::process::id()];
        while let Some(parent) = parents.pop() {
            for process in processes.iter().filter(|process| process.ppid == parent) {
                parents.push(process.pid);
                if !process.has_exited() {
                    found.push(*process);
                }
            }
        }
        Ok(found)
    }

    /// Sends `signal` to `process` if it is still the process that was
    /// found, never to one that took over its PID since.
    fn signal(process: Stat, signal: i32) -> io::Result<()> {
        // SAFETY: opens a descriptor for the PID; closed below.
        let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, process.pid, 0) } as libc::c_int;
        if pidfd == -1 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error) };
        }
        // The descriptor names one process for good. If the PID shows
        // another start time by now, the process found had gone and the PID
        // was reused, and the descriptor names the newcomer.
        let result = match procfs::stat(process.pid) {
            Ok(Some(now)) if now.start == process.start => {
                // SAFETY: a signal through a descriptor this function owns.
                let sent = unsafe {
                    libc::syscall(libc::SYS_pidfd_send_signal, pidfd, signal, std::ptr::null::<libc::siginfo_t>(), 0)
                };
                let error = io::Error::last_os_error();
                if sent == -1 && error.raw_os_error() != Some(libc::ESRCH) { Err(error) } else { Ok(()) }
            }
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        };
        // SAFETY: the descriptor opened above.
        unsafe { libc::close(pidfd) };
        result
    }
}
