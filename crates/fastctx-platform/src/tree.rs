//! A program together with every process it starts.
//!
//! The World's `run` starts a shell and answers for everything that shell
//! starts: it must know when the shell exits, when the last process it left
//! behind is gone and how many are still running, and it must be able to
//! stop them all. Output is either one merged terminal stream or separate
//! stdout and stderr streams; stdin is either null or an already complete local file.
//!
//! On Linux the process holding the tree becomes a child subreaper: a
//! process whose parent exits is handed to it instead of to init, so the
//! tree is exactly its descendants, and once it has no children left the
//! tree is empty. A process holds at most one tree.
//!
//! On Windows the tree is a job object the program is created in, so no
//! instant passes in which it could start anything outside. Every process
//! created the ordinary way inherits the job, whether it detaches, opens a
//! console of its own or makes a nested job; none may break away. Closing
//! the last handle to the job ends whatever is left in it, so a holder that
//! dies takes the tree with it. A process started on the program's behalf
//! by a service (WMI, the Task Scheduler) is not its descendant and is not
//! in the job.
//!
//! macOS has no subreaper: a process whose parent exits goes to launchd. The
//! program leads a session of its own, and the tree is that session together
//! with the program's descendants found through their parents; a process
//! that leaves the session and loses its parent both is not reached. Whether
//! the tree is empty is seen by looking, not proven by the kernel.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::ExitStatus;

/// A started program and everything it starts.
pub struct ProcessTree(imp::Tree);

/// Reading ends owned by the caller; EOF means every writer has closed.
pub struct TreeOutput {
    pub stdout: crate::output_pipe::Reader,
    pub stderr: Option<crate::output_pipe::Reader>,
}

impl ProcessTree {
    /// Starts `program` with `args` in `cwd` with exactly the environment
    /// `env`. Returns the tree and the reading end of its output.
    pub fn spawn(
        program: &Path,
        args: &[&OsStr],
        cwd: &Path,
        env: &[(OsString, OsString)],
    ) -> io::Result<(Self, crate::output_pipe::Reader)> {
        let (tree, output) =
            Self::spawn_with_io(program, args, cwd, env, fastctx_types::run::OutputMode::Merged, None)?;
        Ok((tree, output.stdout))
    }

    /// Starts a tree with the requested output layout. Both readers must be
    /// drained concurrently, through EOF, including after the main exits.
    pub fn spawn_with_io(
        program: &Path,
        args: &[&OsStr],
        cwd: &Path,
        env: &[(OsString, OsString)],
        mode: fastctx_types::run::OutputMode,
        stdin: Option<&Path>,
    ) -> io::Result<(Self, TreeOutput)> {
        let (stdout, writer) = crate::output_pipe::pipe()?;
        let (stderr, error) = match mode {
            fastctx_types::run::OutputMode::Merged => (None, writer.try_clone()?),
            fastctx_types::run::OutputMode::Separate => {
                let (reader, writer) = crate::output_pipe::pipe()?;
                (Some(reader), writer)
            }
        };
        let tree = imp::Tree::spawn(program, args, cwd, env, writer, error, stdin)?;
        Ok((Self(tree), TreeOutput { stdout, stderr }))
    }

    /// The PID of the program itself.
    pub fn main_pid(&self) -> u32 {
        self.0.main_pid()
    }

    /// Waits for the program itself to exit.
    pub fn wait_main(&self) -> io::Result<ExitStatus> {
        self.0.wait_main()
    }

    /// Counts survivors after `wait_main`, before starting `wait_empty`.
    /// On Linux this also reaps exited children so an empty sampled process
    /// graph is never mistaken for the kernel's proof that no child remains.
    pub fn remaining(&self) -> io::Result<usize> {
        self.0.remaining()
    }

    /// Waits until no process of the tree remains, after `wait_main`.
    pub fn wait_empty(&self) -> io::Result<()> {
        self.0.wait_empty()
    }

    /// How many processes of the tree are running now; one in the middle
    /// of exiting is not counted.
    pub fn count(&self) -> io::Result<usize> {
        self.0.count()
    }

    /// Ends every process of the tree. On Linux another thread must drive
    /// wait_main followed by wait_empty: its reaping lets the kernel prove
    /// that no child remains, without discarding the main program's status.
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
            stdout: io::PipeWriter,
            stderr: io::PipeWriter,
            stdin: Option<&Path>,
        ) -> io::Result<Self> {
            // Waiting for any child is how the tree is waited for, so a
            // second tree in the same process would steal the first's.
            if HOLDS_A_TREE.swap(true, Ordering::SeqCst) {
                return Err(io::Error::other("a process holds at most one process tree"));
            }
            // SAFETY: a plain prctl on this process.
            if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == -1 {
                return Err(io::Error::last_os_error());
            }
            let mut command = Command::new(program);
            command
                .args(args)
                .current_dir(cwd)
                .env_clear()
                .envs(env.iter().map(|(name, value)| (name, value)))
                .stdin(match stdin {
                    Some(path) => Stdio::from(std::fs::File::open(path)?),
                    None => Stdio::null(),
                })
                .stdout(stdout)
                .stderr(stderr);
            // SAFETY: setsid and the descriptor marking are async-signal-safe.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    // exec preserves the parent's signal mask. Supervision
                    // blocks stop signals, but the program must receive them.
                    let mut empty: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut empty);
                    if libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    close_on_exec_from(3)
                });
            }
            let main = command.spawn()?.id();
            // The command keeps copies of the pipe's writing end; the reader
            // sees the end of the output only once they are gone.
            drop(command);
            Ok(Self { main })
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

        pub fn remaining(&self) -> io::Result<usize> {
            loop {
                if reap_ready()? {
                    return Ok(0);
                }
                let count = self.count()?;
                if reap_ready()? {
                    return Ok(0);
                }
                if count > 0 {
                    return Ok(count);
                }
                // Reparenting can temporarily disconnect the sampled /proc
                // graph; exiting children may not yet be waitable. Neither
                // proves an empty tree. Reap and observe again, without a
                // guessed grace period or counting a dying child as alive.
                std::thread::yield_now();
            }
        }

        pub fn count(&self) -> io::Result<usize> {
            Ok(descendants()?.iter().filter(|process| !process.is_ending()).count())
        }

        pub fn kill(&self) -> io::Result<()> {
            // Each round kills every process found; one started meanwhile by
            // a process not yet killed is found in the next round.
            loop {
                let processes = descendants()?;
                // /proc is a sequence of observations: reparenting can
                // disconnect a live descendant in that sampled graph. Only
                // the kernel's ECHILD proves that the subreaper is empty.
                if no_children()? {
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

    fn no_children() -> io::Result<bool> {
        // WNOWAIT leaves every status for the thread calling wait_main and
        // wait_empty, including the main program's status.
        // SAFETY: a zeroed siginfo_t is a valid waitid output buffer.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: the buffer is valid, and WNOWAIT does not consume status.
        let result = unsafe { libc::waitid(libc::P_ALL, 0, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
        if result == 0 {
            return Ok(false);
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::ECHILD) => Ok(true),
            Some(libc::EINTR) => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// Reaps all available statuses; true only for the kernel's ECHILD.
    fn reap_ready() -> io::Result<bool> {
        loop {
            let mut status = 0;
            // SAFETY: the main status has already been consumed and no other
            // thread reaps until remaining returns.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid > 0 {
                continue;
            }
            if pid == 0 {
                return Ok(false);
            }
            match io::Error::last_os_error().raw_os_error() {
                Some(libc::ECHILD) => return Ok(true),
                Some(libc::EINTR) => continue,
                _ => return Err(io::Error::last_os_error()),
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
            return match error.raw_os_error() {
                Some(libc::ESRCH) => Ok(()),
                // Linux names a process by a descriptor from 5.3 on. An
                // older kernel, or a sandbox that refuses the call, has the
                // PID signalled right after its start time is read, which
                // leaves only that moment for the PID to be reused.
                Some(libc::ENOSYS | libc::EPERM) => send_if_same(process, || {
                    // SAFETY: a signal to the process just identified.
                    unsafe { libc::kill(process.pid as libc::pid_t, signal) as libc::c_long }
                }),
                _ => Err(error),
            };
        }
        // The descriptor names one process for good. If the PID shows
        // another start time by now, the process found had gone and the PID
        // was reused, and the descriptor names the newcomer.
        let result = send_if_same(process, || {
            // SAFETY: a signal through a descriptor this function owns.
            unsafe { libc::syscall(libc::SYS_pidfd_send_signal, pidfd, signal, std::ptr::null::<libc::siginfo_t>(), 0) }
        });
        // SAFETY: the descriptor opened above.
        unsafe { libc::close(pidfd) };
        result
    }

    /// Sends with `send` while `process` still has the start time it was
    /// found with; one gone by then is left be.
    fn send_if_same(process: Stat, send: impl FnOnce() -> libc::c_long) -> io::Result<()> {
        match procfs::stat(process.pid) {
            Ok(Some(now)) if now.start == process.start => {
                let sent = send();
                let error = io::Error::last_os_error();
                if sent == -1 && error.raw_os_error() != Some(libc::ESRCH) { Err(error) } else { Ok(()) }
            }
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
    use std::path::Path;
    use std::process::{Command, ExitStatus, Stdio};
    use std::time::Duration;

    use crate::darwin_proc::{self, Info};
    use crate::unix_fds::close_on_exec_from;

    /// How often an emptying tree is looked at.
    const LOOK: Duration = Duration::from_millis(100);

    pub struct Tree {
        main: u32,
    }

    impl Tree {
        pub fn spawn(
            program: &Path,
            args: &[&OsStr],
            cwd: &Path,
            env: &[(OsString, OsString)],
            stdout: io::PipeWriter,
            stderr: io::PipeWriter,
            stdin: Option<&Path>,
        ) -> io::Result<Self> {
            let mut command = Command::new(program);
            command
                .args(args)
                .current_dir(cwd)
                .env_clear()
                .envs(env.iter().map(|(name, value)| (name, value)))
                .stdin(match stdin {
                    Some(path) => Stdio::from(std::fs::File::open(path)?),
                    None => Stdio::null(),
                })
                .stdout(stdout)
                .stderr(stderr);
            // SAFETY: setsid, sigprocmask and the descriptor marking are
            // async-signal-safe.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    let mut empty: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut empty);
                    if libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    close_on_exec_from(3)
                });
            }
            let main = command.spawn()?.id();
            drop(command);
            Ok(Self { main })
        }

        pub fn main_pid(&self) -> u32 {
            self.main
        }

        pub fn wait_main(&self) -> io::Result<ExitStatus> {
            loop {
                let mut status = 0;
                // SAFETY: waits for this process's own child, the program.
                let pid = unsafe { libc::waitpid(self.main as libc::pid_t, &mut status, 0) };
                if pid == self.main as libc::pid_t {
                    return Ok(ExitStatus::from_raw(status));
                }
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EINTR) {
                    return Err(error);
                }
            }
        }

        pub fn remaining(&self) -> io::Result<usize> {
            self.count()
        }

        pub fn wait_empty(&self) -> io::Result<()> {
            while self.count()? > 0 {
                std::thread::sleep(LOOK);
            }
            Ok(())
        }

        pub fn count(&self) -> io::Result<usize> {
            Ok(members(self.main)?.len())
        }

        pub fn kill(&self) -> io::Result<()> {
            // Each round kills every process found; one started meanwhile by
            // a process not yet killed is found in the next round.
            loop {
                let found = members(self.main)?;
                if found.is_empty() {
                    return Ok(());
                }
                for process in found {
                    signal(process, libc::SIGKILL)?;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// Every live process of the tree: the program's session, and the
    /// program's descendants found through their parents.
    fn members(main: u32) -> io::Result<Vec<Info>> {
        let processes = darwin_proc::all()?;
        let here = std::process::id();
        let mut found: Vec<Info> = Vec::new();
        let mut parents = vec![main];
        while let Some(parent) = parents.pop() {
            for process in processes.iter().filter(|process| process.ppid == parent) {
                if !found.iter().any(|known| known.pid == process.pid) {
                    parents.push(process.pid);
                    found.push(*process);
                }
            }
        }
        for process in &processes {
            if process.pid != here
                && process.session() == Some(main)
                && !found.iter().any(|known| known.pid == process.pid)
            {
                found.push(*process);
            }
        }
        found.retain(|process| !process.zombie);
        Ok(found)
    }

    /// Sends `signal` to `process` if it is still the process that was
    /// found: nothing names a process for good on macOS, so its start time,
    /// read again just before, tells a reused PID.
    fn signal(process: Info, signal: i32) -> io::Result<()> {
        match darwin_proc::info(process.pid)? {
            Some(now) if now.start == process.start => {
                // SAFETY: a signal to the process just identified.
                if unsafe { libc::kill(process.pid as libc::pid_t, signal) } == -1 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::mem::{offset_of, size_of};
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use std::os::windows::process::ExitStatusExt as _;
    use std::path::Path;
    use std::process::ExitStatus;
    use std::ptr;

    use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, STILL_ACTIVE, WAIT_TIMEOUT};
    use windows_sys::Win32::System::JobObjects::{
        IsProcessInJob, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
        JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        QueryFullProcessImageNameW, WaitForSingleObject,
    };

    use crate::windows_job::{Stdio, create_in_job, new_job, open_null_for_reading, query, set, wait};

    pub struct Tree {
        job: OwnedHandle,
        main: OwnedHandle,
        main_pid: u32,
    }

    impl Tree {
        pub fn spawn(
            program: &Path,
            args: &[&OsStr],
            cwd: &Path,
            env: &[(OsString, OsString)],
            stdout: io::PipeWriter,
            stderr: io::PipeWriter,
            stdin: Option<&Path>,
        ) -> io::Result<Self> {
            let job = new_job()?;
            let stdin = match stdin {
                Some(path) => OwnedHandle::from(std::fs::File::open(path)?),
                None => open_null_for_reading()?,
            };
            let stdio =
                Stdio { input: stdin.as_raw_handle(), output: stdout.as_raw_handle(), error: stderr.as_raw_handle() };
            let (main, main_pid) = create_in_job(&job, program, args, cwd, env, &stdio)?;
            // The reader sees the end of the output only once this copy of
            // the writing end is gone too.
            drop((stdout, stderr));
            Ok(Self { job, main, main_pid })
        }

        pub fn main_pid(&self) -> u32 {
            self.main_pid
        }

        pub fn wait_main(&self) -> io::Result<ExitStatus> {
            wait(&self.main)?;
            let mut code = 0;
            // SAFETY: a process handle that has signalled.
            if unsafe { GetExitCodeProcess(self.main.as_raw_handle(), &mut code) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(ExitStatus::from_raw(code))
        }

        pub fn wait_empty(&self) -> io::Result<()> {
            // Waiting on each member's handle, then asking again, covers
            // members that start meanwhile. The job's process count can
            // reach zero before an ending member's handle signals, so both
            // the list and the count must be empty.
            loop {
                let members = self.members()?;
                if members.is_empty() && self.accounting()?.ActiveProcesses == 0 {
                    return Ok(());
                }
                for member in &members {
                    wait(member)?;
                }
                std::thread::yield_now();
            }
        }

        pub fn count(&self) -> io::Result<usize> {
            let mut count = 0;
            for member in self.members()? {
                // SAFETY: a zero wait on a live handle.
                if unsafe { WaitForSingleObject(member.as_raw_handle(), 0) } != WAIT_TIMEOUT {
                    continue;
                }
                // A process that is exiting already has its exit code while
                // its handle has yet to signal. (One that exits with 259,
                // the value of STILL_ACTIVE, counts as running.)
                let mut code = 0;
                // SAFETY: a live handle with query access.
                if unsafe { GetExitCodeProcess(member.as_raw_handle(), &mut code) } != 0 && code != STILL_ACTIVE as u32
                {
                    continue;
                }
                // The console host Windows adds for a console is not one of
                // the program's processes.
                if image_name(&member).is_some_and(|name| name.eq_ignore_ascii_case("conhost.exe")) {
                    continue;
                }
                count += 1;
            }
            Ok(count)
        }

        pub fn remaining(&self) -> io::Result<usize> {
            self.count()
        }

        pub fn kill(&self) -> io::Result<()> {
            // With at most one active process allowed, no member can start
            // another; existing members are not evicted by the change.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = query(&self.job, JobObjectExtendedLimitInformation)?;
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
            set(&self.job, JobObjectExtendedLimitInformation, &limits)?;
            let members = self.members()?;
            // SAFETY: this tree owns the job and everything in it.
            if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // Termination is asynchronous; it is done when every member's
            // handle has signalled and the job is empty.
            for member in &members {
                wait(member)?;
            }
            self.wait_empty()
        }

        fn accounting(&self) -> io::Result<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
            query(&self.job, JobObjectBasicAccountingInformation)
        }

        /// Handles to the job's members now. A PID may be reused between
        /// listing and opening; a handle counts only once the process behind
        /// it is confirmed to be in the job.
        fn members(&self) -> io::Result<Vec<OwnedHandle>> {
            let mut handles = Vec::new();
            for pid in self.pids()? {
                // SAFETY: query and synchronize access only.
                let raw =
                    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid as u32) };
                if raw.is_null() {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                        continue;
                    }
                    return Err(error);
                }
                // SAFETY: a new handle owned from here.
                let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
                let mut member = 0;
                // SAFETY: two valid handles and an out-parameter.
                if unsafe { IsProcessInJob(handle.as_raw_handle(), self.job.as_raw_handle(), &mut member) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                if member != 0 {
                    handles.push(handle);
                }
            }
            Ok(handles)
        }

        fn pids(&self) -> io::Result<Vec<usize>> {
            // Size the list from the count, and again from what the system
            // reports if members arrived meanwhile.
            let mut count = self.accounting()?.ActiveProcesses as usize;
            loop {
                let bytes =
                    offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList) + count.max(1) * size_of::<usize>();
                let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
                let list = storage.as_mut_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
                // SAFETY: aligned storage sized for the header and the array.
                unsafe {
                    if QueryInformationJobObject(
                        self.job.as_raw_handle(),
                        JobObjectBasicProcessIdList,
                        list.cast(),
                        bytes as u32,
                        ptr::null_mut(),
                    ) == 0
                    {
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) {
                            return Err(error);
                        }
                        count = (*list).NumberOfAssignedProcesses as usize;
                        continue;
                    }
                    let length = (*list).NumberOfProcessIdsInList as usize;
                    let first = ptr::addr_of!((*list).ProcessIdList).cast::<usize>();
                    return Ok(std::slice::from_raw_parts(first, length).to_vec());
                }
            }
        }
    }

    fn image_name(process: &OwnedHandle) -> Option<String> {
        let mut buffer = vec![0u16; 32_768];
        let mut length = buffer.len() as u32;
        // SAFETY: the buffer holds `length` units.
        let named = unsafe {
            QueryFullProcessImageNameW(process.as_raw_handle(), PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut length)
        };
        if named == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        path.rsplit(['\\', '/']).next().map(str::to_string)
    }
}
