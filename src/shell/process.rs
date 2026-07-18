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
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);
static UTF8_LOCALE: OnceLock<String> = OnceLock::new();

/// A spawned bash process whose kill operation covers the whole descendant tree.
#[derive(Debug)]
pub(crate) struct ManagedProcess {
    child: Box<dyn ChildWrapper>,
    output: Option<PipeReader>,
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
) -> std::io::Result<ManagedProcess> {
    let (reader, writer) = std::io::pipe()?;
    let mut command = crate::process_policy::noninteractive_command(bash);
    let locale = utf8_locale(bash);
    if login_shell {
        command.arg("-lc");
    } else {
        command.args(["--noprofile", "--norc", "-c"]);
    }
    command
        .arg(command_text)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .env("LANG", locale)
        .env("LC_ALL", locale)
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
        .env("PYTHONUNBUFFERED", "1");

    #[cfg(windows)]
    if !login_shell {
        prepend_windows_toolset(&mut command, bash);
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
    })
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
fn create_kill_on_close_job(
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
fn resume_process_threads(pid: u32) -> std::io::Result<()> {
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
fn prepend_windows_toolset(command: &mut Command, bash: &Path) {
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
    if let Some(existing) = std::env::var_os("PATH")
        && !existing.is_empty()
    {
        path.push(";");
        path.push(&existing);
    }
    command.env("PATH", path);
}

fn utf8_locale(bash: &Path) -> &'static str {
    UTF8_LOCALE
        .get_or_init(|| detect_utf8_locale(bash))
        .as_str()
}

fn detect_utf8_locale(bash: &Path) -> String {
    let available = crate::process_policy::noninteractive_command(bash)
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

#[cfg(test)]
mod tests {
    use super::select_utf8_locale;

    #[test]
    fn locale_selection_prefers_c_utf8_and_has_the_frozen_fallback() {
        assert_eq!(select_utf8_locale("c\nc.utf8\nen_us.utf8\n"), "C.UTF-8");
        assert_eq!(select_utf8_locale("c\nposix\n"), "en_US.UTF-8");
    }

    #[cfg(windows)]
    #[test]
    fn managed_bash_child_preserves_the_no_console_window_policy() {
        use std::io::Read;
        use std::time::{Duration, Instant};

        let bash = crate::shell::bash::probe_bash().unwrap();
        let executable = std::env::current_exe().unwrap();
        let quoted_executable = format!(
            "'{}'",
            executable
                .to_string_lossy()
                .replace('\\', "/")
                .replace('\'', "'\"'\"'")
        );
        let command = format!(
            "FASTCTX_TEST_NO_WINDOW_PROBE=1 {quoted_executable} --exact process_policy::tests::noninteractive_child_has_no_console --test-threads=1"
        );
        let mut process =
            super::spawn_bash(&bash, &command, &std::env::current_dir().unwrap(), false).unwrap();
        let mut output = process.take_output();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = process.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "the no-window probe did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut text = String::new();
        output.read_to_string(&mut text).unwrap();
        assert!(status.success(), "{text}");
    }
}
