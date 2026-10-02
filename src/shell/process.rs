//! Cross-platform process-tree spawning for non-interactive bash commands.

#[cfg(windows)]
use process_wrap::std::CommandWrapper;
use process_wrap::std::{ChildWrapper, CommandWrap};
use std::io::PipeReader;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Command;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Byte length of command text above which the command runs from a script file.
///
/// Windows CreateProcessW caps one command line at 32,767 UTF-16 units, and argument
/// quoting can double the text in the worst case, so a long command passed straight to
/// `bash -c` fails to spawn (os error 206). Commands over this threshold are written to
/// a temp script instead, which has no length ceiling; 12,000 bytes doubles to well
/// under the cap with room for the bash path and flags. Applied on every platform so
/// behavior does not fork by OS (2026-08-08).
const SCRIPT_SPAWN_THRESHOLD_BYTES: usize = 12_000;

/// A spawned bash process whose kill operation covers the whole descendant tree.
#[derive(Debug)]
pub(crate) struct ManagedProcess {
    child: Box<dyn ChildWrapper>,
    output: Option<PipeReader>,
    /// Keeps an over-threshold command's script file alive until the process is reaped.
    _command_script: Option<tempfile::TempPath>,
}

impl ManagedProcess {
    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn take_output(&mut self) -> PipeReader {
        self.output
            .take()
            .expect("the merged output pipe is taken exactly once")
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub(crate) fn kill_tree(&mut self) -> std::io::Result<()> {
        match self.child.start_kill() {
            Ok(()) => Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Kills the whole tree and waits with a hard bound so an OS failure cannot hang the MCP call.
    pub(crate) fn terminate_tree(&mut self) -> std::io::Result<ExitStatus> {
        let kill_result = self.kill_tree();
        let deadline = Instant::now() + TERMINATION_TIMEOUT;
        loop {
            match self.try_wait()? {
                Some(status) => return Ok(status),
                None if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                None => {
                    return Err(kill_result.err().unwrap_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "the process tree did not exit within 5 seconds after termination",
                        )
                    }));
                }
            }
        }
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        let _ = self.kill_tree();
    }
}

/// Spawns a login or clean non-login bash with one merged stdout/stderr pipe.
pub(crate) fn spawn_bash(
    bash: &Path,
    command_text: &str,
    cwd: &Path,
    login_shell: bool,
    environment: &crate::session::SessionEnvironment,
    utf8_locale: &str,
) -> std::io::Result<ManagedProcess> {
    let (reader, writer) = std::io::pipe()?;
    let mut command = crate::process_policy::noninteractive_command(bash);
    environment.configure_command(&mut command);
    let command_script = if command_text.len() > SCRIPT_SPAWN_THRESHOLD_BYTES {
        let script = write_command_script(command_text)?;
        if login_shell {
            command.arg("-l");
        } else {
            command.args(["--noprofile", "--norc"]);
        }
        command.arg(script_operand(&script));
        Some(script)
    } else {
        if login_shell {
            command.arg("-lc");
        } else {
            command.args(["--noprofile", "--norc", "-c"]);
        }
        command.arg(command_text);
        None
    };
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .env("LANG", utf8_locale)
        .env("LC_ALL", utf8_locale)
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .env("FORCE_COLOR", "0")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("GIT_EDITOR", "true")
        .env("EDITOR", "true")
        .env("VISUAL", "true")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8");

    #[cfg(windows)]
    if !login_shell {
        prepend_windows_toolset(&mut command, bash, environment);
    }
    #[cfg(windows)]
    if login_shell && environment.var_os(MSYS_PATH_TYPE).is_none() {
        command.env(MSYS_PATH_TYPE, "inherit");
    }

    let mut wrapped = CommandWrap::from(command);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessSession);
    #[cfg(windows)]
    wrapped.wrap(KillOnCloseJobObject);
    let child = wrapped.spawn()?;
    drop(wrapped);
    Ok(ManagedProcess {
        child,
        output: Some(reader),
        _command_script: command_script,
    })
}

/// Writes over-threshold command text to a private temp script deleted when dropped.
fn write_command_script(command_text: &str) -> std::io::Result<tempfile::TempPath> {
    use std::io::Write;

    let mut script = tempfile::Builder::new()
        .prefix("fastctx-command-")
        .suffix(".sh")
        .tempfile()?;
    script.write_all(command_text.as_bytes())?;
    script.flush()?;
    Ok(script.into_temp_path())
}

/// Renders the script path as the bash operand.
fn script_operand(script: &tempfile::TempPath) -> std::ffi::OsString {
    #[cfg(windows)]
    {
        // The msys argument layer passes a native absolute path through cleanly only in
        // forward-slash form; backslashes are exposed to POSIX escaping rules.
        std::ffi::OsString::from(script.to_string_lossy().replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        script.as_os_str().to_os_string()
    }
}

/// Owns a Windows Job Object whose process tree dies even if the supervising process is killed.
///
/// `process-wrap`'s synchronous JobObject deliberately omits
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. That is insufficient for persistent
/// background jobs: an externally terminated supervisor would drop its handle
/// without running Rust destructors and leave the bash tree orphaned. This
/// wrapper keeps the child suspended until assignment and makes the kernel
/// enforce the ownership invariant.
#[cfg(windows)]
#[derive(Debug)]
struct KillOnCloseJobObject;

#[cfg(windows)]
impl CommandWrapper for KillOnCloseJobObject {
    fn pre_spawn(&mut self, command: &mut Command, _core: &CommandWrap) -> std::io::Result<()> {
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

        command.creation_flags(crate::process_policy::noninteractive_creation_flags(
            CREATE_SUSPENDED,
        ));
        Ok(())
    }

    fn wrap_child(
        &mut self,
        mut child: Box<dyn ChildWrapper>,
        _core: &CommandWrap,
    ) -> std::io::Result<Box<dyn ChildWrapper>> {
        let process_handle = child.inner_child().as_raw_handle();
        let job = match create_kill_on_close_job(process_handle) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if let Err(error) = resume_process_threads(child.id()) {
            let _ = terminate_job(&job);
            let _ = child.wait();
            return Err(error);
        }
        Ok(Box::new(KillOnCloseJobChild {
            inner: Some(child),
            job,
        }))
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct KillOnCloseJobChild {
    inner: Option<Box<dyn ChildWrapper>>,
    job: OwnedHandle,
}

#[cfg(windows)]
impl ChildWrapper for KillOnCloseJobChild {
    fn inner(&self) -> &dyn ChildWrapper {
        self.inner
            .as_deref()
            .expect("the child wrapper is present until it is consumed")
    }

    fn inner_mut(&mut self) -> &mut dyn ChildWrapper {
        self.inner
            .as_deref_mut()
            .expect("the child wrapper is present until it is consumed")
    }

    fn into_inner(mut self: Box<Self>) -> Box<dyn ChildWrapper> {
        let _ = terminate_job(&self.job);
        self.inner
            .take()
            .expect("the child wrapper is consumed exactly once")
    }

    fn start_kill(&mut self) -> std::io::Result<()> {
        terminate_job(&self.job)
    }
}

#[cfg(windows)]
pub(crate) fn create_kill_on_close_job(
    process_handle: std::os::windows::io::RawHandle,
) -> std::io::Result<OwnedHandle> {
    use std::ffi::c_void;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    // SAFETY: null security/name arguments request an unnamed job with default
    // security. The returned owned handle is closed on every exit path.
    let raw_job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw_job.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: CreateJobObjectW returned a fresh handle owned by this function.
    let job = unsafe { OwnedHandle::from_raw_handle(raw_job.cast()) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: both pointers remain valid for the duration of each synchronous
    // call, and the buffer size exactly matches the declared information class.
    let configured = unsafe {
        SetInformationJobObject(
            job.as_raw_handle().cast(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    if configured == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the process is still suspended, so no descendant can escape
    // between assignment and resumption.
    let assigned =
        unsafe { AssignProcessToJobObject(job.as_raw_handle().cast(), process_handle.cast()) };
    if assigned == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(job)
}

#[cfg(windows)]
pub(crate) fn resume_process_threads(pid: u32) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    // SAFETY: the snapshot handle is closed below on every path after creation.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: entry has the documented size and remains live while enumerated.
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) };
    let mut result = if has_entry == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    };
    let mut resumed_any = false;
    while has_entry != 0 {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: the thread id comes from the live system snapshot.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                result = Err(std::io::Error::last_os_error());
                break;
            }
            // SAFETY: the handle was opened with THREAD_SUSPEND_RESUME.
            let resumed = unsafe { ResumeThread(thread) };
            let resume_error = std::io::Error::last_os_error();
            // SAFETY: this function owns the thread handle.
            let closed = unsafe { CloseHandle(thread) };
            if resumed == u32::MAX {
                result = Err(resume_error);
                break;
            }
            if closed == 0 {
                result = Err(std::io::Error::last_os_error());
                break;
            }
            resumed_any = true;
        }
        // SAFETY: snapshot and entry remain valid for the next enumeration step.
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) };
    }
    // SAFETY: this function owns the snapshot handle.
    let closed = unsafe { CloseHandle(snapshot) };
    if result.is_ok() && closed == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if result.is_ok() && !resumed_any {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("cannot find a suspended thread for process {pid}"),
        ));
    }
    result
}

#[cfg(windows)]
fn terminate_job(job: &OwnedHandle) -> std::io::Result<()> {
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;

    // SAFETY: job is a live Job Object handle owned by the wrapper.
    if unsafe { TerminateJobObject(job.as_raw_handle().cast(), 1) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Selects how the msys `/etc/profile` composes PATH out of the Windows PATH it inherits.
///
/// Git for Windows defaults this to `inherit`, but an msys2 installation defaults to `minimal`,
/// whose profile *replaces* the inherited PATH with four Windows system directories — so a login
/// shell there resolves none of the user's own tools. `prepend_windows_toolset` is the non-login
/// counterpart; a login shell needs this instead, because profile runs after we hand over the
/// environment. An explicit value is always left alone: choosing `minimal` or `strict` is a
/// deliberate isolation choice that belongs to whoever made it.
#[cfg(windows)]
const MSYS_PATH_TYPE: &str = "MSYS2_PATH_TYPE";

/// Gives a non-login Windows bash the Git-for-Windows Unix toolset on PATH.
///
/// `--noprofile --norc` never sources `/etc/profile`, which is what normally puts
/// `/usr/bin` (coreutils, sed, grep, awk) and `/mingw*/bin` (git) on PATH. Without
/// them a clean shell resolves no external command and every one fails with 127 —
/// whenever the server was launched from a host whose PATH lacks those directories
/// (e.g. Codex started from PowerShell rather than Git Bash). We prepend the toolset
/// directories in native Windows form; the msys runtime rewrites them to POSIX for
/// the child. A login shell needs none of this — profile sets it up — so it is left
/// untouched.
#[cfg(windows)]
fn prepend_windows_toolset(
    command: &mut Command,
    bash: &Path,
    environment: &crate::session::SessionEnvironment,
) {
    let Some(usr_bin) = bash.parent() else { return };
    let mut dirs: Vec<PathBuf> = vec![usr_bin.to_path_buf()];
    // Auto-discovered bash is <GitRoot>\usr\bin\bash.exe; git lives in <GitRoot>\mingw*\bin.
    if let Some(git_root) = usr_bin.parent().and_then(Path::parent) {
        for arch in ["mingw64", "mingw32", "clangarm64"] {
            let bin = git_root.join(arch).join("bin");
            if bin.is_dir() {
                dirs.push(bin);
            }
        }
    }
    let Ok(mut path) = std::env::join_paths(&dirs) else {
        return;
    };
    if let Some(existing) = environment.var_os("PATH")
        && !existing.is_empty()
    {
        path.push(";");
        path.push(&existing);
    }
    command.env("PATH", path);
}

pub(crate) fn detect_utf8_locale(
    bash: &Path,
    environment: &crate::session::SessionEnvironment,
) -> String {
    let mut command = crate::process_policy::noninteractive_command(bash);
    environment.configure_command(&mut command);
    let available = command
        .args(["-lc", "locale -a 2>/dev/null || true"])
        .stdin(Stdio::null())
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_ascii_lowercase())
        .unwrap_or_default();
    select_utf8_locale(&available).to_string()
}

fn select_utf8_locale(available: &str) -> &'static str {
    if available
        .lines()
        .any(|locale| matches!(locale.trim(), "c.utf8" | "c.utf-8"))
    {
        "C.UTF-8"
    } else {
        "en_US.UTF-8"
    }
}

/// Converts signal exits to the bash-compatible `128 + signal` convention.
pub(crate) fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}
