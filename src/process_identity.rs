//! Cross-platform process identities and direct-parent lifecycle monitoring.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const PARENT_POLL_INTERVAL: Duration = Duration::from_millis(500);
#[cfg(any(target_os = "linux", target_os = "macos"))]
const PROCESS_INSPECTION_RETRY: Duration = Duration::from_secs(1);

/// PID plus an operating-system process creation token, used together to reject PID reuse.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) started: String,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MacosProcessIdentityProbe {
    Running(ProcessIdentity),
    Exited,
    Unavailable,
}

#[cfg(target_os = "linux")]
enum LinuxProcessIdentityProbe {
    Running(ProcessIdentity),
    Exited,
    Unavailable,
}

#[cfg(target_os = "macos")]
pub(crate) struct MacosProcessExitWatcher {
    queue: std::os::fd::OwnedFd,
    pid: u32,
}

#[cfg(target_os = "macos")]
impl MacosProcessExitWatcher {
    /// Registers a kernel exit notification for the exact process identity.
    ///
    /// `Ok(None)` means the identity exited or was reused while the watch was being established.
    /// Inspection failures remain errors rather than being misreported as process exit.
    pub(crate) fn register(identity: &ProcessIdentity) -> Result<Option<Self>, String> {
        use std::os::fd::{AsRawFd, FromRawFd};

        // SAFETY: kqueue creates a new descriptor owned by this function on success.
        let raw_queue = unsafe { libc::kqueue() };
        if raw_queue < 0 {
            return Err(format!(
                "Cannot create a process exit notification for PID {}: {}",
                identity.pid,
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: kqueue returned a newly owned descriptor.
        let queue = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw_queue) };
        // Prevent a monitored parent's descriptor from leaking into shell work spawned by FastCtx.
        // SAFETY: queue is a live descriptor and F_GETFD/F_SETFD do not consume it.
        let descriptor_flags = unsafe { libc::fcntl(queue.as_raw_fd(), libc::F_GETFD) };
        if descriptor_flags < 0
            || unsafe {
                libc::fcntl(
                    queue.as_raw_fd(),
                    libc::F_SETFD,
                    descriptor_flags | libc::FD_CLOEXEC,
                )
            } < 0
        {
            return Err(format!(
                "Cannot protect the process exit notification for PID {} from descriptor inheritance: {}",
                identity.pid,
                std::io::Error::last_os_error()
            ));
        }

        let change = libc::kevent {
            ident: identity.pid as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: change is a valid one-element registration list; no event buffer is requested.
        let registered = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if registered < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(None);
            }
            return Err(format!(
                "Cannot register a process exit notification for PID {}: {error}",
                identity.pid
            ));
        }

        let watcher = Self {
            queue,
            pid: identity.pid,
        };
        if watcher.wait(Duration::ZERO)? {
            return Ok(None);
        }
        match macos_process_identity_probe(identity.pid) {
            MacosProcessIdentityProbe::Running(current) if current == *identity => {
                Ok(Some(watcher))
            }
            MacosProcessIdentityProbe::Running(_) | MacosProcessIdentityProbe::Exited => Ok(None),
            MacosProcessIdentityProbe::Unavailable => Err(format!(
                "Cannot revalidate PID {} after registering its exit notification",
                identity.pid
            )),
        }
    }

    /// Waits for the registered process object to emit `NOTE_EXIT`.
    pub(crate) fn wait(&self, timeout: Duration) -> Result<bool, String> {
        use std::os::fd::AsRawFd;

        let deadline = std::time::Instant::now() + timeout;
        let mut remaining = timeout;
        loop {
            let timeout = libc::timespec {
                tv_sec: libc::time_t::try_from(remaining.as_secs())
                    .map_err(|_| "The process exit timeout is too large".to_string())?,
                tv_nsec: libc::c_long::from(remaining.subsec_nanos()),
            };
            // SAFETY: an all-zero kevent is valid as an output buffer.
            let mut event = unsafe { std::mem::zeroed::<libc::kevent>() };
            // SAFETY: event is a writable one-element event list and timeout remains live.
            let ready = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &mut event,
                    1,
                    &timeout,
                )
            };
            if ready > 0 {
                let flags = event.flags;
                let data = event.data;
                if flags & libc::EV_ERROR != 0 {
                    let error = i32::try_from(data)
                        .ok()
                        .filter(|code| *code > 0)
                        .map(std::io::Error::from_raw_os_error);
                    return Err(match error {
                        Some(error) => format!(
                            "The process exit notification for PID {} failed: {error}",
                            self.pid
                        ),
                        None => {
                            format!("The process exit notification for PID {} failed", self.pid)
                        }
                    });
                }
                let ident = event.ident;
                let filter = event.filter;
                let fflags = event.fflags;
                if ident == self.pid as libc::uintptr_t
                    && filter == libc::EVFILT_PROC
                    && fflags & libc::NOTE_EXIT != 0
                {
                    return Ok(true);
                }
                return Err(format!(
                    "The process exit notification for PID {} returned an unexpected event",
                    self.pid
                ));
            }
            if ready == 0 {
                return Ok(false);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!(
                    "Cannot wait for the process exit notification for PID {}: {error}",
                    self.pid
                ));
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            remaining = deadline.saturating_duration_since(now);
        }
    }
}

/// Returns a process identity only while the PID can be inspected.
pub(crate) fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    #[cfg(target_os = "linux")]
    {
        match linux_process_identity_probe(pid) {
            LinuxProcessIdentityProbe::Running(identity) => Some(identity),
            LinuxProcessIdentityProbe::Exited | LinuxProcessIdentityProbe::Unavailable => None,
        }
    }
    #[cfg(target_os = "macos")]
    {
        match macos_process_identity_probe(pid) {
            MacosProcessIdentityProbe::Running(identity) => Some(identity),
            MacosProcessIdentityProbe::Exited | MacosProcessIdentityProbe::Unavailable => None,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        process_start_token(pid).map(|started| ProcessIdentity { pid, started })
    }
}

/// Requires both PID and creation token to match, preventing recycled PIDs from looking alive.
pub(crate) fn identity_is_alive(identity: &ProcessIdentity) -> bool {
    #[cfg(target_os = "macos")]
    {
        let deadline = std::time::Instant::now() + PROCESS_INSPECTION_RETRY;
        loop {
            match macos_process_identity_probe(identity.pid) {
                MacosProcessIdentityProbe::Running(current) => return current == *identity,
                MacosProcessIdentityProbe::Exited => return false,
                MacosProcessIdentityProbe::Unavailable if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                MacosProcessIdentityProbe::Unavailable => return false,
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        process_identity(identity.pid).as_ref() == Some(identity)
    }
}

/// Captures the direct parent at startup unless the documented escape hatch is enabled.
///
/// `Ok(Some(None))` means that the direct parent disappeared during capture and callers should
/// shut down immediately. An unsupported platform returns `Ok(None)`.
pub(crate) fn parent_identity_from_environment() -> Result<Option<Option<ProcessIdentity>>, String>
{
    if std::env::var("FASTCTX_NO_PARENT_WATCH").ok().as_deref() == Some("1") {
        return Ok(None);
    }
    let Some(pid) = direct_parent_pid()? else {
        return Ok(None);
    };
    #[cfg(target_os = "linux")]
    {
        let deadline = std::time::Instant::now() + PROCESS_INSPECTION_RETRY;
        loop {
            match linux_process_identity_probe(pid) {
                LinuxProcessIdentityProbe::Running(identity) => {
                    return Ok(Some(Some(identity)));
                }
                LinuxProcessIdentityProbe::Exited => return Ok(Some(None)),
                LinuxProcessIdentityProbe::Unavailable if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                LinuxProcessIdentityProbe::Unavailable => {
                    return Err(format!(
                        "Cannot capture the MCP server parent process identity for PID {pid}: /proc inspection remained unavailable"
                    ));
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let deadline = std::time::Instant::now() + PROCESS_INSPECTION_RETRY;
        loop {
            match macos_process_identity_probe(pid) {
                MacosProcessIdentityProbe::Running(identity) => {
                    return Ok(Some(Some(identity)));
                }
                MacosProcessIdentityProbe::Exited => return Ok(Some(None)),
                MacosProcessIdentityProbe::Unavailable if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                MacosProcessIdentityProbe::Unavailable => {
                    return Err(format!(
                        "Cannot capture the MCP server parent process identity for PID {pid}: libproc inspection remained unavailable"
                    ));
                }
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(Some(process_identity(pid)))
    }
}

/// Blocks until the identity exits or the caller cancels the monitor.
///
/// Returns `true` only when the process exited. Cancellation is checked at most every 500 ms;
/// Windows, Linux, and macOS use kernel notifications bound to the process object where available.
pub(crate) fn wait_for_identity_exit_until(
    identity: &ProcessIdentity,
    cancelled: &AtomicBool,
) -> bool {
    #[cfg(windows)]
    {
        wait_for_identity_exit_windows(identity, cancelled)
    }
    #[cfg(target_os = "macos")]
    {
        match MacosProcessExitWatcher::register(identity) {
            Ok(None) => return true,
            Ok(Some(watcher)) => {
                while !cancelled.load(Ordering::Acquire) {
                    match watcher.wait(PARENT_POLL_INTERVAL) {
                        Ok(true) => return true,
                        Ok(false) => {}
                        // A broken notification is not evidence of exit. Fall back to the
                        // conservative identity probe instead of orphaning or killing a live host.
                        Err(_) => break,
                    }
                }
                if cancelled.load(Ordering::Acquire) {
                    return false;
                }
            }
            // kqueue can be denied for a process the caller may still inspect. The fallback keeps
            // the old conservative behavior without treating watcher setup failure as parent exit.
            Err(_) => {}
        }
        while !cancelled.load(Ordering::Acquire) {
            match macos_process_identity_probe(identity.pid) {
                MacosProcessIdentityProbe::Running(current) if current == *identity => {
                    std::thread::sleep(PARENT_POLL_INTERVAL);
                }
                MacosProcessIdentityProbe::Running(_) | MacosProcessIdentityProbe::Exited => {
                    return true;
                }
                MacosProcessIdentityProbe::Unavailable => {
                    // A failed inspection is not evidence that the parent exited.
                    std::thread::sleep(PARENT_POLL_INTERVAL);
                }
            }
        }
        false
    }
    #[cfg(target_os = "linux")]
    {
        wait_for_identity_exit_linux(identity, cancelled)
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    {
        while !cancelled.load(Ordering::Acquire) && identity_is_alive(identity) {
            std::thread::sleep(PARENT_POLL_INTERVAL);
        }
        !cancelled.load(Ordering::Acquire)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (identity, cancelled);
        false
    }
}

#[cfg(unix)]
fn direct_parent_pid() -> Result<Option<u32>, String> {
    // SAFETY: getppid has no preconditions and returns the caller's current direct parent PID.
    let pid = unsafe { libc::getppid() };
    Ok(u32::try_from(pid).ok().filter(|pid| *pid > 0))
}

#[cfg(windows)]
fn direct_parent_pid() -> Result<Option<u32>, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: the snapshot is closed on every path after successful creation.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "Cannot capture the MCP server parent process: {}",
            std::io::Error::last_os_error()
        ));
    }
    let current = std::process::id();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: entry has the documented size and remains valid during enumeration.
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) };
    let mut parent = None;
    while has_entry != 0 {
        if entry.th32ProcessID == current {
            parent = (entry.th32ParentProcessID > 0).then_some(entry.th32ParentProcessID);
            break;
        }
        // SAFETY: snapshot and entry remain valid for the next enumeration step.
        has_entry = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    // SAFETY: this function owns the snapshot handle.
    unsafe {
        CloseHandle(snapshot);
    }
    Ok(parent)
}

#[cfg(not(any(unix, windows)))]
fn direct_parent_pid() -> Result<Option<u32>, String> {
    Ok(None)
}

#[cfg(target_os = "linux")]
fn linux_process_identity_probe(pid: u32) -> LinuxProcessIdentityProbe {
    let source = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return LinuxProcessIdentityProbe::Exited;
        }
        Err(_) => return LinuxProcessIdentityProbe::Unavailable,
    };
    let Some((_, suffix)) = source.rsplit_once(')') else {
        return LinuxProcessIdentityProbe::Unavailable;
    };
    let fields = suffix.split_whitespace().collect::<Vec<_>>();
    let Some(state) = fields.first() else {
        return LinuxProcessIdentityProbe::Unavailable;
    };
    if matches!(*state, "Z" | "X") {
        return LinuxProcessIdentityProbe::Exited;
    }
    let Some(started) = fields.get(19) else {
        return LinuxProcessIdentityProbe::Unavailable;
    };
    LinuxProcessIdentityProbe::Running(ProcessIdentity {
        pid,
        started: (*started).to_string(),
    })
}

#[cfg(target_os = "linux")]
fn wait_for_identity_exit_linux(identity: &ProcessIdentity, cancelled: &AtomicBool) -> bool {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let Ok(pid) = i32::try_from(identity.pid) else {
        return false;
    };
    // SAFETY: pidfd_open receives a positive PID and no flags; a successful descriptor is owned
    // below and remains bound to that process object across PID reuse.
    let raw_pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if raw_pidfd < 0 {
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return true;
        }
        return wait_for_identity_exit_linux_fallback(identity, cancelled);
    }
    // SAFETY: pidfd_open returned a newly owned descriptor.
    let pidfd = unsafe { OwnedFd::from_raw_fd(raw_pidfd) };
    match linux_process_identity_probe(identity.pid) {
        LinuxProcessIdentityProbe::Running(current) if current == *identity => {}
        LinuxProcessIdentityProbe::Running(_) | LinuxProcessIdentityProbe::Exited => return true,
        LinuxProcessIdentityProbe::Unavailable => {
            return wait_for_identity_exit_linux_fallback(identity, cancelled);
        }
    }

    while !cancelled.load(Ordering::Acquire) {
        let mut descriptor = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: descriptor is a writable one-element pollfd array for the duration of poll.
        let ready = unsafe {
            libc::poll(
                &mut descriptor,
                1,
                i32::try_from(PARENT_POLL_INTERVAL.as_millis())
                    .expect("the parent poll interval fits in a poll timeout"),
            )
        };
        if ready > 0 {
            if descriptor.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                return true;
            }
            return false;
        }
        if ready < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn wait_for_identity_exit_linux_fallback(
    identity: &ProcessIdentity,
    cancelled: &AtomicBool,
) -> bool {
    while !cancelled.load(Ordering::Acquire) {
        match linux_process_identity_probe(identity.pid) {
            LinuxProcessIdentityProbe::Running(current) if current == *identity => {
                std::thread::sleep(PARENT_POLL_INTERVAL);
            }
            LinuxProcessIdentityProbe::Running(_) | LinuxProcessIdentityProbe::Exited => {
                return true;
            }
            LinuxProcessIdentityProbe::Unavailable => {
                // Inspection failure is not proof that the parent exited.
                std::thread::sleep(PARENT_POLL_INTERVAL);
            }
        }
    }
    false
}

#[cfg(target_os = "macos")]
#[link(name = "proc")]
unsafe extern "C" {
    #[link_name = "proc_pidinfo"]
    fn macos_proc_pidinfo(
        pid: libc::c_int,
        flavor: libc::c_int,
        arg: u64,
        buffer: *mut libc::c_void,
        buffersize: libc::c_int,
    ) -> libc::c_int;
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_process_identity_probe(pid: u32) -> MacosProcessIdentityProbe {
    let Ok(raw_pid) = i32::try_from(pid) else {
        return MacosProcessIdentityProbe::Unavailable;
    };
    // SAFETY: proc_bsdinfo is a plain C data struct whose all-zero bit pattern is valid.
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: __error returns the calling thread's writable errno slot on macOS.
    unsafe {
        *libc::__error() = 0;
    }
    // SAFETY: info points to a writable proc_bsdinfo buffer of the exact size passed here.
    let read = unsafe {
        macos_proc_pidinfo(
            raw_pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            i32::try_from(size).expect("proc_bsdinfo fits in a C int"),
        )
    };
    if usize::try_from(read).ok() == Some(size) {
        if info.pbi_status == libc::SZOMB {
            return MacosProcessIdentityProbe::Exited;
        }
        return MacosProcessIdentityProbe::Running(ProcessIdentity {
            pid,
            started: format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec),
        });
    }

    // proc_pidinfo can fail transiently. Confirm that the PID is truly absent before calling it
    // exited; a live but temporarily uninspectable parent must never shut down its server.
    // SAFETY: kill with signal 0 performs an existence/permission probe without sending a signal.
    unsafe {
        *libc::__error() = 0;
    }
    let exists = unsafe { libc::kill(raw_pid, 0) };
    if exists == 0 {
        return MacosProcessIdentityProbe::Unavailable;
    }
    if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        MacosProcessIdentityProbe::Exited
    } else {
        MacosProcessIdentityProbe::Unavailable
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn process_start_token(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

#[cfg(windows)]
fn process_start_token(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: the returned process handle is closed by process_start_token_from_handle.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    process_start_token_from_handle(process)
}

#[cfg(windows)]
fn process_start_token_from_handle(
    process: windows_sys::Win32::Foundation::HANDLE,
) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::GetProcessTimes;

    if process.is_null() {
        return None;
    }
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    // SAFETY: process is live and all FILETIME output buffers are writable.
    let success =
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    // SAFETY: this helper owns the process handle supplied by process_start_token.
    unsafe {
        CloseHandle(process);
    }
    (success != 0).then(|| {
        let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        ticks.to_string()
    })
}

#[cfg(windows)]
fn wait_for_identity_exit_windows(identity: &ProcessIdentity, cancelled: &AtomicBool) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: the handle is closed below after identity verification and waiting.
    let process = unsafe {
        OpenProcess(
            SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            identity.pid,
        )
    };
    if process.is_null() {
        // Inspection failure is not proof of parent death; disabling this watch avoids
        // terminating a live host session on a transient access failure.
        return false;
    }
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    // SAFETY: process is live and all FILETIME output buffers are writable.
    let inspected =
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    let matches = inspected != 0 && ticks.to_string() == identity.started;
    let mut exited = !matches;
    while matches && !cancelled.load(Ordering::Acquire) {
        // SAFETY: SYNCHRONIZE grants waiting on this live process handle.
        let wait = unsafe {
            WaitForSingleObject(
                process,
                u32::try_from(PARENT_POLL_INTERVAL.as_millis())
                    .expect("the parent poll interval fits in a Windows timeout"),
            )
        };
        match wait {
            WAIT_OBJECT_0 => {
                exited = true;
                break;
            }
            WAIT_TIMEOUT => {}
            _ => {
                // A failed wait is not evidence that the parent exited.
                exited = false;
                break;
            }
        }
    }
    // SAFETY: this function owns the process handle.
    unsafe {
        CloseHandle(process);
    }
    exited
}

#[cfg(not(any(unix, windows)))]
fn process_start_token(_pid: u32) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::{identity_is_alive, process_identity, wait_for_identity_exit_until};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn current_process_identity_is_stable_and_rejects_a_different_creation_token() {
        let identity = process_identity(std::process::id())
            .expect("the current process creation token should be inspectable");
        assert!(identity_is_alive(&identity));

        let mut recycled = identity;
        recycled.started.push_str("-different-process");
        assert!(!identity_is_alive(&recycled));
    }

    #[test]
    fn process_handle_wait_observes_the_exact_process_exit() {
        #[cfg(windows)]
        let mut child = std::process::Command::new(
            std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into()),
        )
        .args(["/D", "/Q", "/C", "ping -n 2 127.0.0.1 >NUL"])
        .spawn()
        .unwrap();
        #[cfg(unix)]
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 0.2"])
            .spawn()
            .unwrap();
        let identity = process_identity(child.id()).unwrap();
        let cancelled = AtomicBool::new(false);
        assert!(wait_for_identity_exit_until(&identity, &cancelled));
        assert!(child.wait().unwrap().success());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn process_exit_watch_observes_a_killed_child_before_it_is_reaped() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .unwrap();
        let identity = process_identity(child.id()).unwrap();
        child.kill().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));

        let cancelled = AtomicBool::new(false);
        assert!(wait_for_identity_exit_until(&identity, &cancelled));
        assert!(!child.wait().unwrap().success());
    }
}
