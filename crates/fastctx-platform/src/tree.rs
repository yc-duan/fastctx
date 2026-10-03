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
//!
//! On Windows the tree is a job object the program is created in, so no
//! instant passes in which it could start anything outside. Every process
//! created the ordinary way inherits the job, whether it detaches, opens a
//! console of its own or makes a nested job; none may break away. Closing
//! the last handle to the job ends whatever is left in it, so a holder that
//! dies takes the tree with it. A process started on the program's behalf
//! by a service (WMI, the Task Scheduler) is not its descendant and is not
//! in the job.

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

#[cfg(windows)]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::mem::{offset_of, size_of, zeroed};
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use std::os::windows::process::ExitStatusExt as _;
    use std::path::Path;
    use std::process::ExitStatus;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, GENERIC_READ, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOBOBJECTINFOCLASS, JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
        LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcess, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
        PROCESS_INFORMATION, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        QueryFullProcessImageNameW, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
        WaitForSingleObject,
    };

    use crate::detach::check_environment;
    use crate::windows_process::{command_line, environment_block, wide};

    pub struct Tree {
        job: OwnedHandle,
        main: OwnedHandle,
        main_pid: u32,
    }

    /// The flag that gives the program a console of its own with no window.
    /// Console programs it starts share that console, so none of them opens
    /// a window on the desktop.
    const CONSOLE: u32 = CREATE_NO_WINDOW;

    impl Tree {
        pub fn spawn(
            program: &Path,
            args: &[&OsStr],
            cwd: &Path,
            env: &[(OsString, OsString)],
        ) -> io::Result<(Self, io::PipeReader)> {
            if !program.is_absolute() || !cwd.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "program and working directory must be absolute paths",
                ));
            }
            check_environment(env)?;
            let job = new_job()?;
            let (reader, writer) = io::pipe()?;
            let stdin = open_null_for_reading()?;
            // Only these two handles are inherited, and only by this child:
            // the handle list names them explicitly.
            set_inheritable(writer.as_raw_handle())?;
            let handles = [stdin.as_raw_handle(), writer.as_raw_handle()];
            let jobs = [job.as_raw_handle()];
            let mut attributes = Attributes::new(2)?;
            attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST as usize, &jobs)?;
            attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, &handles)?;

            let application = wide(program);
            let mut line = command_line(program.as_os_str(), args)?;
            let directory = wide(cwd);
            let block = environment_block(env);
            // SAFETY: zeroed is a valid empty STARTUPINFOEXW.
            let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = stdin.as_raw_handle();
            startup.StartupInfo.hStdOutput = writer.as_raw_handle();
            startup.StartupInfo.hStdError = writer.as_raw_handle();
            startup.lpAttributeList = attributes.as_ptr();
            // SAFETY: zeroed is valid for an out-parameter.
            let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
            // SAFETY: every string, array and the attribute list outlive the
            // call. The job is assigned before any code of the program runs.
            let created = unsafe {
                CreateProcessW(
                    application.as_ptr(),
                    line.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    1,
                    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CONSOLE,
                    block.as_ptr().cast(),
                    directory.as_ptr(),
                    &startup.StartupInfo,
                    &mut info,
                )
            };
            if created == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: handles returned by CreateProcessW, owned from here.
            let main = unsafe {
                drop(OwnedHandle::from_raw_handle(info.hThread));
                OwnedHandle::from_raw_handle(info.hProcess)
            };
            // The reader sees the end of the output only once this copy of
            // the writing end is gone too.
            drop(writer);
            Ok((Self { job, main, main_pid: info.dwProcessId }, reader))
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
                // The console host Windows adds for a console is not one of
                // the program's processes.
                if image_name(&member).is_some_and(|name| name.eq_ignore_ascii_case("conhost.exe")) {
                    continue;
                }
                count += 1;
            }
            Ok(count)
        }

        pub fn kill(&self) -> io::Result<()> {
            // With at most one active process allowed, no member can start
            // another; existing members are not evicted by the change.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION =
                query(&self.job, JobObjectExtendedLimitInformation)?;
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

    fn new_job() -> io::Result<OwnedHandle> {
        // SAFETY: an unnamed job with default security, owned from here.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new handle.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: zeroed is a valid empty limit structure.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        set(&job, JobObjectExtendedLimitInformation, &limits)?;
        Ok(job)
    }

    fn set<T>(job: &OwnedHandle, class: JOBOBJECTINFOCLASS, value: &T) -> io::Result<()> {
        // SAFETY: each caller pairs the class with its documented type.
        let done = unsafe {
            SetInformationJobObject(job.as_raw_handle(), class, (value as *const T).cast(), size_of::<T>() as u32)
        };
        if done == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn query<T>(job: &OwnedHandle, class: JOBOBJECTINFOCLASS) -> io::Result<T> {
        // SAFETY: each caller pairs the class with its documented type, for
        // which zeroed memory is a valid starting value.
        unsafe {
            let mut value: T = zeroed();
            if QueryInformationJobObject(
                job.as_raw_handle(),
                class,
                (&raw mut value).cast(),
                size_of::<T>() as u32,
                ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(value)
        }
    }

    fn wait(process: &OwnedHandle) -> io::Result<()> {
        // SAFETY: a valid process handle.
        match unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) } {
            WAIT_OBJECT_0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
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

    fn open_null_for_reading() -> io::Result<OwnedHandle> {
        let name: Vec<u16> = "NUL".encode_utf16().chain([0]).collect();
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: 1,
        };
        // SAFETY: a NUL-terminated name; the handle is owned from here.
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &attributes,
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    fn set_inheritable(handle: HANDLE) -> io::Result<()> {
        // SAFETY: a valid handle this process owns.
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// A process-creation attribute list.
    struct Attributes(Vec<usize>);

    impl Attributes {
        fn new(count: u32) -> io::Result<Self> {
            let mut bytes = 0;
            // SAFETY: the first call only reports the size needed.
            unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &mut bytes) };
            let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
            // SAFETY: storage of the reported size, aligned for the list.
            if unsafe { InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), count, 0, &mut bytes) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(storage))
        }

        /// Sets `attribute` to `handles`, which must outlive the process
        /// creation that uses this list.
        fn set(&mut self, attribute: usize, handles: &[HANDLE]) -> io::Result<()> {
            // SAFETY: an initialized list and an array the caller keeps alive.
            let done = unsafe {
                UpdateProcThreadAttribute(
                    self.as_ptr(),
                    0,
                    attribute,
                    handles.as_ptr().cast(),
                    size_of_val(handles),
                    ptr::null_mut(),
                    ptr::null(),
                )
            };
            if done == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        fn as_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
            self.0.as_mut_ptr().cast()
        }
    }

    impl Drop for Attributes {
        fn drop(&mut self) {
            // SAFETY: an initialized list whose storage is still alive.
            unsafe { DeleteProcThreadAttributeList(self.as_ptr()) }
        }
    }
}
