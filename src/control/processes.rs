//! Cross-platform discovery and termination of FastCtx installation processes.

use crate::process_identity::ProcessIdentity;
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
use crate::process_identity::identity_is_alive;
#[cfg(unix)]
use crate::process_identity::process_identity;
#[cfg(target_os = "macos")]
use crate::process_identity::{
    MacosProcessExitWatcher, MacosProcessIdentityProbe, macos_process_identity_probe,
};
use std::path::{Path, PathBuf};
use std::time::Duration;
#[cfg(all(unix, not(target_os = "linux")))]
use std::time::Instant;

const TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Stable process snapshot used by Status and by Unapply's preview/commit boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstalledProcess {
    pub(crate) identity: ProcessIdentity,
    pub(crate) image_path: PathBuf,
}

/// Result of a PID-safe termination attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminationOutcome {
    /// The exact snapshotted process was terminated.
    Terminated,
    /// The PID exited, was reused, or no longer points inside the managed bin directory.
    NoLongerManaged,
}

/// Lists inspectable process images that currently run from below the managed bin directory.
pub(crate) fn installed_processes(bin_directory: &Path) -> Result<Vec<InstalledProcess>, String> {
    let mut processes = platform_processes()?
        .into_iter()
        .filter(|process| path_is_under(&process.image_path, bin_directory))
        .collect::<Vec<_>>();
    processes.sort_by_key(|process| process.identity.pid);
    Ok(processes)
}

/// Terminates only if PID, creation token, and the reopened image path still identify a managed process.
pub(crate) fn terminate_installed_process(
    expected: &InstalledProcess,
    bin_directory: &Path,
) -> Result<TerminationOutcome, String> {
    if expected.identity.pid == std::process::id() {
        return Ok(TerminationOutcome::NoLongerManaged);
    }
    terminate_platform_process(expected, bin_directory)
}

/// Human-readable PID/path details for a residual-directory error.
pub(crate) fn process_details(processes: &[InstalledProcess]) -> String {
    processes
        .iter()
        .map(|process| {
            format!(
                "PID {} ({})",
                process.identity.pid,
                crate::paths::display_path(&process.image_path)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn path_is_under(image: &Path, directory: &Path) -> bool {
    #[cfg(windows)]
    {
        let image = windows_path_key(image);
        let mut directory = windows_path_key(directory);
        while directory.ends_with('\\') {
            directory.pop();
        }
        image.starts_with(&format!("{directory}\\"))
    }
    #[cfg(not(windows))]
    {
        let image = dunce::canonicalize(image).unwrap_or_else(|_| image.to_path_buf());
        let directory = dunce::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
        image != directory && image.starts_with(directory)
    }
}

#[cfg(windows)]
fn windows_path_key(path: &Path) -> String {
    let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_ascii_lowercase()
}

#[cfg(windows)]
fn platform_processes() -> Result<Vec<InstalledProcess>, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: the snapshot is closed on every path after successful creation.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "Cannot enumerate running processes: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: entry has the documented size and remains valid during enumeration.
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) };
    let mut processes = Vec::new();
    while has_entry != 0 {
        if let Some(process) = inspect_windows_process(entry.th32ProcessID) {
            processes.push(process);
        }
        // SAFETY: snapshot and entry remain valid for the next enumeration step.
        has_entry = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    // SAFETY: this function owns the snapshot handle.
    unsafe {
        CloseHandle(snapshot);
    }
    Ok(processes)
}

#[cfg(windows)]
fn inspect_windows_process(pid: u32) -> Option<InstalledProcess> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: the handle is closed after both identity and path are queried from it.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return None;
    }
    let identity = windows_identity_from_handle(process, pid);
    let image_path = windows_image_from_handle(process);
    // SAFETY: this function owns the process handle.
    unsafe {
        CloseHandle(process);
    }
    Some(InstalledProcess {
        identity: identity?,
        image_path: image_path?,
    })
}

#[cfg(windows)]
fn windows_identity_from_handle(
    process: windows_sys::Win32::Foundation::HANDLE,
    pid: u32,
) -> Option<ProcessIdentity> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;

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
    (inspected != 0).then(|| ProcessIdentity {
        pid,
        started: ((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
            .to_string(),
    })
}

#[cfg(windows)]
fn windows_image_from_handle(process: windows_sys::Win32::Foundation::HANDLE) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Threading::QueryFullProcessImageNameW;

    let mut buffer = vec![0u16; 32_768];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: buffer is writable for `length` UTF-16 code units.
    let queried =
        unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) };
    if queried == 0 {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(
        &buffer[..usize::try_from(length).ok()?],
    )))
}

#[cfg(windows)]
fn terminate_platform_process(
    expected: &InstalledProcess,
    bin_directory: &Path,
) -> Result<TerminationOutcome, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
        WaitForSingleObject,
    };

    // SAFETY: the handle is closed on every return path after successful creation.
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,
            0,
            expected.identity.pid,
        )
    };
    if process.is_null() {
        return if identity_is_alive(&expected.identity) {
            Err(format!(
                "Cannot open PID {} ({}) for termination: {}",
                expected.identity.pid,
                crate::paths::display_path(&expected.image_path),
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(TerminationOutcome::NoLongerManaged)
        };
    }
    let current_identity = windows_identity_from_handle(process, expected.identity.pid);
    let current_image = windows_image_from_handle(process);
    if current_identity.as_ref() != Some(&expected.identity)
        || !current_image
            .as_deref()
            .is_some_and(|path| path_is_under(path, bin_directory))
    {
        // SAFETY: this function owns the process handle.
        unsafe {
            CloseHandle(process);
        }
        return Ok(TerminationOutcome::NoLongerManaged);
    }
    // SAFETY: the reopened handle identifies the exact snapshotted process and grants termination.
    let terminated = unsafe { TerminateProcess(process, 1) };
    let terminate_error = std::io::Error::last_os_error();
    // SAFETY: SYNCHRONIZE permits waiting on this process handle.
    let wait = unsafe {
        WaitForSingleObject(
            process,
            u32::try_from(TERMINATION_TIMEOUT.as_millis())
                .expect("the termination timeout fits in a Windows timeout"),
        )
    };
    // SAFETY: this function owns the process handle.
    unsafe {
        CloseHandle(process);
    }
    if wait == WAIT_OBJECT_0 {
        Ok(TerminationOutcome::Terminated)
    } else if terminated == 0 {
        Err(format!(
            "Cannot terminate PID {} ({}): {terminate_error}",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path)
        ))
    } else {
        Err(format!(
            "PID {} ({}) did not exit within {} seconds",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path),
            TERMINATION_TIMEOUT.as_secs()
        ))
    }
}

#[cfg(target_os = "linux")]
fn platform_processes() -> Result<Vec<InstalledProcess>, String> {
    let entries = std::fs::read_dir("/proc").map_err(|error| {
        format!("Cannot enumerate /proc for running FastCtx processes: {error}")
    })?;
    let mut processes = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(process) = inspect_unix_process(pid) {
            processes.push(process);
        }
    }
    Ok(processes)
}

#[cfg(target_os = "macos")]
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_listallpids(buffer: *mut libc::c_void, buffersize: libc::c_int) -> libc::c_int;
    fn proc_pidpath(pid: libc::c_int, buffer: *mut libc::c_void, buffersize: u32) -> libc::c_int;
}

#[cfg(target_os = "macos")]
fn platform_processes() -> Result<Vec<InstalledProcess>, String> {
    // SAFETY: a null buffer asks libproc for the current PID count.
    let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
    if count < 0 {
        return Err("Cannot enumerate running processes with libproc".to_string());
    }
    let mut pids = vec![
        0i32;
        usize::try_from(count)
            .unwrap_or_default()
            .saturating_add(64)
    ];
    let bytes = i32::try_from(pids.len().saturating_mul(std::mem::size_of::<i32>()))
        .map_err(|_| "The process list is too large to inspect".to_string())?;
    // SAFETY: pids is writable for exactly `bytes` bytes.
    let read = unsafe { proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if read < 0 {
        return Err("Cannot enumerate running processes with libproc".to_string());
    }
    pids.truncate(usize::try_from(read).unwrap_or_default().min(pids.len()));
    Ok(pids
        .into_iter()
        .filter_map(|pid| u32::try_from(pid).ok())
        .filter_map(inspect_unix_process)
        .collect())
}

#[cfg(unix)]
fn inspect_unix_process(pid: u32) -> Option<InstalledProcess> {
    let before = process_identity(pid)?;
    let image_path = unix_process_image(pid)?;
    let after = process_identity(pid)?;
    (before == after).then_some(InstalledProcess {
        identity: before,
        image_path,
    })
}

#[cfg(target_os = "linux")]
fn unix_process_image(pid: u32) -> Option<PathBuf> {
    let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let display = path.to_string_lossy();
    display
        .strip_suffix(" (deleted)")
        .map(PathBuf::from)
        .or(Some(path))
}

#[cfg(target_os = "macos")]
fn unix_process_image(pid: u32) -> Option<PathBuf> {
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
    let pid = i32::try_from(pid).ok()?;
    let mut buffer = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    // SAFETY: buffer is writable for the exact byte count supplied.
    let read = unsafe {
        proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast(),
            u32::try_from(buffer.len()).ok()?,
        )
    };
    if read <= 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    Some(PathBuf::from(std::ffi::OsString::from(
        String::from_utf8_lossy(&buffer[..end]).into_owned(),
    )))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn platform_processes() -> Result<Vec<InstalledProcess>, String> {
    Ok(Vec::new())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn unix_process_image(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MacosManagedProcessState {
    Managed,
    Exited,
    NoLongerManaged,
    Unavailable,
}

#[cfg(target_os = "macos")]
fn terminate_platform_process(
    expected: &InstalledProcess,
    bin_directory: &Path,
) -> Result<TerminationOutcome, String> {
    let pid = i32::try_from(expected.identity.pid)
        .map_err(|_| format!("Invalid process id: {}", expected.identity.pid))?;
    let inspection_deadline = Instant::now() + Duration::from_secs(1);
    let watcher = loop {
        match MacosProcessExitWatcher::register(&expected.identity) {
            Ok(Some(watcher)) => break watcher,
            Ok(None) => return Ok(TerminationOutcome::NoLongerManaged),
            Err(_) if Instant::now() < inspection_deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(format!(
                    "Cannot prepare to terminate PID {} ({}): {error}; no signal was sent",
                    expected.identity.pid,
                    crate::paths::display_path(&expected.image_path)
                ));
            }
        }
    };
    match revalidate_macos_managed_process(expected, bin_directory, &watcher)? {
        MacosManagedProcessState::Managed => {}
        MacosManagedProcessState::Exited | MacosManagedProcessState::NoLongerManaged => {
            return Ok(TerminationOutcome::NoLongerManaged);
        }
        MacosManagedProcessState::Unavailable => {
            return Err(format!(
                "Cannot verify PID {} ({}) before termination; no signal was sent",
                expected.identity.pid,
                crate::paths::display_path(&expected.image_path)
            ));
        }
    }
    // SAFETY: the kernel watcher and a fresh identity/path check identify the process immediately
    // before signalling.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) || watcher.wait(Duration::ZERO)? {
            return Ok(TerminationOutcome::Terminated);
        }
        return Err(format!(
            "Cannot terminate PID {} ({}): {error}",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path)
        ));
    }
    if watcher.wait(TERMINATION_TIMEOUT)? {
        return Ok(TerminationOutcome::Terminated);
    }
    match revalidate_macos_managed_process(expected, bin_directory, &watcher)? {
        MacosManagedProcessState::Exited => return Ok(TerminationOutcome::Terminated),
        MacosManagedProcessState::NoLongerManaged => {
            return Ok(TerminationOutcome::NoLongerManaged);
        }
        MacosManagedProcessState::Managed => {}
        MacosManagedProcessState::Unavailable => {
            return Err(format!(
                "Cannot verify that PID {} ({}) is still managed after SIGTERM; refusing to send SIGKILL without revalidating its identity",
                expected.identity.pid,
                crate::paths::display_path(&expected.image_path)
            ));
        }
    }
    // SAFETY: the kernel watcher and identity/image path were revalidated after the graceful
    // timeout.
    if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) || watcher.wait(Duration::ZERO)? {
            return Ok(TerminationOutcome::Terminated);
        }
        return Err(format!(
            "Cannot force-terminate PID {} ({}): {error}",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path)
        ));
    }
    if watcher.wait(Duration::from_secs(1))? {
        Ok(TerminationOutcome::Terminated)
    } else {
        Err(format!(
            "PID {} ({}) did not exit after SIGTERM and SIGKILL",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path)
        ))
    }
}

#[cfg(target_os = "macos")]
fn revalidate_macos_managed_process(
    expected: &InstalledProcess,
    bin_directory: &Path,
    watcher: &MacosProcessExitWatcher,
) -> Result<MacosManagedProcessState, String> {
    let inspection_deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if watcher.wait(Duration::ZERO)? {
            return Ok(MacosManagedProcessState::Exited);
        }
        match macos_managed_process_state(expected, bin_directory) {
            MacosManagedProcessState::Unavailable if Instant::now() < inspection_deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            state => return Ok(state),
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_managed_process_state(
    expected: &InstalledProcess,
    bin_directory: &Path,
) -> MacosManagedProcessState {
    match macos_process_identity_probe(expected.identity.pid) {
        MacosProcessIdentityProbe::Running(identity) if identity != expected.identity => {
            MacosManagedProcessState::NoLongerManaged
        }
        MacosProcessIdentityProbe::Running(_) => match unix_process_image(expected.identity.pid) {
            Some(path) if path_is_under(&path, bin_directory) => MacosManagedProcessState::Managed,
            Some(_) => MacosManagedProcessState::NoLongerManaged,
            None => MacosManagedProcessState::Unavailable,
        },
        MacosProcessIdentityProbe::Exited => MacosManagedProcessState::Exited,
        MacosProcessIdentityProbe::Unavailable => MacosManagedProcessState::Unavailable,
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn terminate_platform_process(
    expected: &InstalledProcess,
    bin_directory: &Path,
) -> Result<TerminationOutcome, String> {
    if !identity_is_alive(&expected.identity)
        || !unix_process_image(expected.identity.pid)
            .as_deref()
            .is_some_and(|path| path_is_under(path, bin_directory))
    {
        return Ok(TerminationOutcome::NoLongerManaged);
    }
    let pid = i32::try_from(expected.identity.pid)
        .map_err(|_| format!("Invalid process id: {}", expected.identity.pid))?;
    // SAFETY: the PID and creation token were revalidated immediately before signalling.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        if !identity_is_alive(&expected.identity) {
            return Ok(TerminationOutcome::Terminated);
        }
        return Err(format!(
            "Cannot terminate PID {} ({}): {}",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path),
            std::io::Error::last_os_error()
        ));
    }
    let deadline = Instant::now() + TERMINATION_TIMEOUT;
    while identity_is_alive(&expected.identity) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    if !identity_is_alive(&expected.identity) {
        return Ok(TerminationOutcome::Terminated);
    }
    if !identity_is_alive(&expected.identity)
        || !unix_process_image(expected.identity.pid)
            .as_deref()
            .is_some_and(|path| path_is_under(path, bin_directory))
    {
        return Ok(TerminationOutcome::NoLongerManaged);
    }
    // SAFETY: identity and image path were revalidated after the graceful timeout.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    let kill_deadline = Instant::now() + Duration::from_secs(1);
    while identity_is_alive(&expected.identity) && Instant::now() < kill_deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    if identity_is_alive(&expected.identity) {
        Err(format!(
            "PID {} ({}) did not exit after SIGTERM and SIGKILL",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path)
        ))
    } else {
        Ok(TerminationOutcome::Terminated)
    }
}

#[cfg(target_os = "linux")]
fn terminate_platform_process(
    expected: &InstalledProcess,
    bin_directory: &Path,
) -> Result<TerminationOutcome, String> {
    use std::os::fd::{FromRawFd, OwnedFd};

    let pid = i32::try_from(expected.identity.pid)
        .map_err(|_| format!("Invalid process id: {}", expected.identity.pid))?;
    // SAFETY: pidfd_open receives a positive PID and no flags; a successful fd is owned below.
    let raw_fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if raw_fd < 0 {
        if !identity_is_alive(&expected.identity) {
            return Ok(TerminationOutcome::NoLongerManaged);
        }
        return Err(format!(
            "Cannot open a PID-safe handle for PID {} ({}): {}",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: pidfd_open returned a newly owned descriptor.
    let pidfd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    if !identity_and_image_match(expected, bin_directory) {
        return Ok(TerminationOutcome::NoLongerManaged);
    }
    pidfd_send_signal(&pidfd, libc::SIGTERM).map_err(|error| {
        format!(
            "Cannot terminate PID {} ({}): {error}",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path)
        )
    })?;
    if wait_pidfd(&pidfd, TERMINATION_TIMEOUT)? {
        return Ok(TerminationOutcome::Terminated);
    }
    // The pidfd remains bound to the exact original process, while this second image check
    // prevents SIGKILL if that process exec'd outside FastCtx during its graceful shutdown.
    if !identity_and_image_match(expected, bin_directory) {
        return Ok(TerminationOutcome::NoLongerManaged);
    }
    pidfd_send_signal(&pidfd, libc::SIGKILL).map_err(|error| {
        format!(
            "Cannot force-terminate PID {} ({}): {error}",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path)
        )
    })?;
    if wait_pidfd(&pidfd, Duration::from_secs(1))? {
        Ok(TerminationOutcome::Terminated)
    } else {
        Err(format!(
            "PID {} ({}) did not exit after SIGTERM and SIGKILL",
            expected.identity.pid,
            crate::paths::display_path(&expected.image_path)
        ))
    }
}

#[cfg(target_os = "linux")]
fn identity_and_image_match(expected: &InstalledProcess, bin_directory: &Path) -> bool {
    identity_is_alive(&expected.identity)
        && unix_process_image(expected.identity.pid)
            .as_deref()
            .is_some_and(|path| path_is_under(path, bin_directory))
}

#[cfg(target_os = "linux")]
fn pidfd_send_signal(
    pidfd: &std::os::fd::OwnedFd,
    signal: libc::c_int,
) -> Result<(), std::io::Error> {
    use std::os::fd::AsRawFd;

    // SAFETY: pidfd is live, signal is SIGTERM/SIGKILL, and null siginfo with flags 0 is valid.
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

#[cfg(target_os = "linux")]
fn wait_pidfd(pidfd: &std::os::fd::OwnedFd, timeout: Duration) -> Result<bool, String> {
    use std::os::fd::AsRawFd;

    let mut descriptor = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout = i32::try_from(timeout.as_millis())
        .map_err(|_| "The process termination timeout is too large".to_string())?;
    // SAFETY: descriptor is a writable one-element pollfd array for the duration of poll.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout) };
    if result > 0 {
        Ok(true)
    } else if result == 0 {
        Ok(false)
    } else {
        Err(format!(
            "Cannot wait for the managed process to exit: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(not(any(unix, windows)))]
fn platform_processes() -> Result<Vec<InstalledProcess>, String> {
    Ok(Vec::new())
}

#[cfg(not(any(unix, windows)))]
fn terminate_platform_process(
    _expected: &InstalledProcess,
    _bin_directory: &Path,
) -> Result<TerminationOutcome, String> {
    Err("Process termination is unsupported on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::{InstalledProcess, TerminationOutcome, path_is_under, terminate_installed_process};

    fn spawn_external_sleeper() -> std::process::Child {
        #[cfg(windows)]
        {
            std::process::Command::new(
                std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into()),
            )
            .args(["/D", "/Q", "/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn()
            .unwrap()
        }
        #[cfg(unix)]
        {
            std::process::Command::new("/bin/sh")
                .args(["-c", "while :; do sleep 1; done"])
                .spawn()
                .unwrap()
        }
    }

    #[test]
    fn managed_image_membership_requires_a_real_descendant_path() {
        let root = std::path::Path::new(if cfg!(windows) {
            r"C:\Users\fixture\.fastctx\bin"
        } else {
            "/home/fixture/.fastctx/bin"
        });
        let child = root.join(if cfg!(windows) {
            "fastctx.exe"
        } else {
            "fastctx"
        });
        let sibling = root.parent().unwrap().join(if cfg!(windows) {
            "binary.exe"
        } else {
            "binary"
        });
        assert!(path_is_under(&child, root));
        assert!(!path_is_under(root, root));
        assert!(!path_is_under(&sibling, root));
    }

    #[test]
    fn termination_revalidates_the_opened_image_path_before_signalling() {
        let temp = tempfile::tempdir().unwrap();
        let managed = temp.path().join("bin");
        std::fs::create_dir(&managed).unwrap();
        let mut child = spawn_external_sleeper();
        let identity = crate::process_identity::process_identity(child.id())
            .expect("the fixture process identity should be inspectable");
        let expected = InstalledProcess {
            identity,
            image_path: managed.join(if cfg!(windows) {
                "fastctx.exe"
            } else {
                "fastctx"
            }),
        };

        assert_eq!(
            terminate_installed_process(&expected, &managed).unwrap(),
            TerminationOutcome::NoLongerManaged
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "path revalidation must not kill an unrelated image"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn termination_always_excludes_the_calling_process() {
        let temp = tempfile::tempdir().unwrap();
        let managed = temp.path().join("bin");
        std::fs::create_dir(&managed).unwrap();
        let expected = InstalledProcess {
            identity: crate::process_identity::process_identity(std::process::id()).unwrap(),
            image_path: managed.join(if cfg!(windows) {
                "fastctx.exe"
            } else {
                "fastctx"
            }),
        };
        assert_eq!(
            terminate_installed_process(&expected, &managed).unwrap(),
            TerminationOutcome::NoLongerManaged
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn deleted_installation_directory_does_not_hide_a_running_managed_image() {
        use super::installed_processes;

        let temp = tempfile::tempdir().unwrap();
        let managed = temp.path().join("bin");
        std::fs::create_dir(&managed).unwrap();
        let executable = managed.join("fastctx");
        std::fs::copy("/bin/sh", &executable).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = std::process::Command::new(&executable)
            .args(["-c", "while :; do sleep 1; done"])
            .spawn()
            .unwrap();
        std::fs::remove_file(&executable).unwrap();
        std::fs::remove_dir(&managed).unwrap();

        let processes = installed_processes(&managed).unwrap();
        assert!(
            processes
                .iter()
                .any(|process| process.identity.pid == child.id()),
            "a deleted image remains attributable to its original managed path"
        );

        let _ = child.kill();
        let _ = child.wait();
    }
}
