//! Observing a host's process lifetime without owning or terminating it.

use fastctx_types::ProcessIdentity;
use std::io;
use std::time::Duration;

mod asynchronous;
pub use asynchronous::exited;
mod status;
pub use status::exit_status;

/// The process that started this one. A command an entry forwarded to the
/// World program counts the entry's parent, as it would had the entry
/// replaced itself with this program (`forwarded_by`).
pub fn parent_identity() -> io::Result<ProcessIdentity> {
    crate::identity_of(parent_pid()?)?.ok_or_else(|| io::Error::other("the parent process already exited"))
}

#[cfg(windows)]
static FORWARDER: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// Records that this process runs for the entry with this process id, which
/// forwarded it to the World program and waits for it: Windows has no exec.
#[cfg(windows)]
pub fn forwarded_by(forwarder: u32) {
    let _ = FORWARDER.set(forwarder);
}

#[cfg(unix)]
fn parent_pid() -> io::Result<u32> {
    // SAFETY: getppid has no pointers or ownership requirements.
    Ok(unsafe { libc::getppid() } as u32)
}

#[cfg(windows)]
fn parent_pid() -> io::Result<u32> {
    let parent = parent_of(std::process::id())?
        .ok_or_else(|| io::Error::other("the current process was absent from the process snapshot"))?;
    if FORWARDER.get() != Some(&parent) {
        return Ok(parent);
    }
    // The entry waits for this process, so it is still in the snapshot.
    parent_of(parent)?.ok_or_else(|| io::Error::other("the entry that forwarded this command already exited"))
}

#[cfg(windows)]
fn parent_of(pid: u32) -> io::Result<Option<u32>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    // SAFETY: the snapshot is owned here and every output structure has its size set.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        let mut more = Process32FirstW(snapshot, &mut entry);
        while more != 0 {
            if entry.th32ProcessID == pid {
                found = Some(entry.th32ParentProcessID);
                break;
            }
            more = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        Ok(found)
    }
}

/// Waits for this process identity to exit. Never sends it a signal.
pub fn wait_for_exit(identity: &ProcessIdentity) -> io::Result<()> {
    wait_until_exit(identity, None).map(drop)
}

/// Whether this process identity exits within `within`, waiting for it no
/// longer. Never sends it a signal.
pub fn exited_within(identity: &ProcessIdentity, within: Duration) -> io::Result<bool> {
    wait_until_exit(identity, Some(within))
}

/// The time left until `deadline`, if there is one, in whole milliseconds
/// rounded up, so a wait never ends before it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn millis_left(deadline: Option<std::time::Instant>) -> Option<u128> {
    deadline.map(|deadline| deadline.saturating_duration_since(std::time::Instant::now()).as_micros().div_ceil(1000))
}

/// Whether this process identity exited, waiting for it at most `within`,
/// or for as long as it runs.
#[cfg(windows)]
fn wait_until_exit(identity: &ProcessIdentity, within: Option<Duration>) -> io::Result<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // A bounded wait never reaches INFINITE, which would make it unbounded.
    let wait =
        within.map_or(INFINITE, |within| u32::try_from(within.as_millis()).unwrap_or(INFINITE - 1).min(INFINITE - 1));
    // SAFETY: the query/synchronize handle is owned here and always closed.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, identity.pid);
        if handle.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) { Ok(true) } else { Err(error) };
        }
        let result = (|| {
            if crate::identity::of_handle(handle, identity.pid)? != *identity {
                return Ok(true);
            }
            match WaitForSingleObject(handle, wait) {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(io::Error::last_os_error()),
            }
        })();
        CloseHandle(handle);
        result
    }
}

#[cfg(target_os = "linux")]
fn wait_until_exit(identity: &ProcessIdentity, within: Option<Duration>) -> io::Result<bool> {
    let deadline = within.map(|within| std::time::Instant::now() + within);
    // SAFETY: opens a descriptor naming one lifetime, closed below.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) } as libc::c_int;
    if fd == -1 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(true),
            // Linux opens a process as a descriptor from 5.3 on. An older
            // kernel, or a sandbox that refuses the call, is asked again
            // and again.
            // A PID can remain visible while pidfd_open can no longer find
            // its task. Poll the exact identity through that exit window.
            Some(libc::ENOSYS | libc::EPERM | libc::EINVAL) => poll_until_exit(identity, deadline),
            _ => Err(error),
        };
    }
    let result = (|| {
        if crate::has_ended(identity)? {
            return Ok(true);
        }
        let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        loop {
            let timeout = millis_left(deadline).map_or(-1, |left| i32::try_from(left).unwrap_or(i32::MAX));
            // SAFETY: poll receives one initialized descriptor.
            let ready = unsafe { libc::poll(&mut poll, 1, timeout) };
            if ready > 0 {
                return Ok(true);
            }
            if ready == 0 {
                return Ok(false);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    })();
    // SAFETY: closes this function's owned descriptor.
    unsafe {
        libc::close(fd);
    }
    result
}

/// Whether the process exited by `deadline`, asked every few milliseconds.
#[cfg(target_os = "linux")]
fn poll_until_exit(identity: &ProcessIdentity, deadline: Option<std::time::Instant>) -> io::Result<bool> {
    const PAUSE: Duration = Duration::from_millis(20);
    loop {
        if crate::has_ended(identity)? {
            return Ok(true);
        }
        let pause = match deadline {
            Some(deadline) => match deadline.checked_duration_since(std::time::Instant::now()) {
                Some(left) if !left.is_zero() => left.min(PAUSE),
                _ => return Ok(false),
            },
            None => PAUSE,
        };
        std::thread::sleep(pause);
    }
}

/// On macOS a kqueue tells of the process's exit; the registration comes
/// before the identity is checked, so an exit in between is not missed.
#[cfg(target_os = "macos")]
fn wait_until_exit(identity: &ProcessIdentity, within: Option<Duration>) -> io::Result<bool> {
    let deadline = within.map(|within| std::time::Instant::now() + within);
    let Some(queue) = exit_queue(identity.pid)? else { return Ok(true) };
    if crate::has_ended(identity)? {
        return Ok(true);
    }
    use std::os::fd::AsRawFd as _;
    loop {
        let timeout = millis_left(deadline).map(|left| libc::timespec {
            tv_sec: libc::time_t::try_from(left / 1000).unwrap_or(libc::time_t::MAX),
            tv_nsec: ((left % 1000) * 1_000_000) as libc::c_long,
        });
        let timeout = timeout.as_ref().map_or(std::ptr::null(), |timeout| timeout as *const libc::timespec);
        // SAFETY: a zeroed kevent is a valid output buffer for one event.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: waits on a kqueue owned here for one event, until `timeout`
        // when there is one.
        let got = unsafe { libc::kevent(queue.as_raw_fd(), std::ptr::null(), 0, &mut event, 1, timeout) };
        if got > 0 {
            return Ok(true);
        }
        if got == 0 {
            return Ok(false);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// A kqueue watching `pid` for its exit; `None` when no such process is left
/// to watch.
#[cfg(target_os = "macos")]
pub(crate) fn exit_queue(pid: u32) -> io::Result<Option<std::os::fd::OwnedFd>> {
    use std::os::fd::FromRawFd as _;
    // SAFETY: kqueue returns a new descriptor or -1.
    let queue = unsafe { libc::kqueue() };
    if queue == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor was just created and is owned here.
    let queue = unsafe { std::os::fd::OwnedFd::from_raw_fd(queue) };
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    use std::os::fd::AsRawFd as _;
    // SAFETY: registers one change on the kqueue owned here.
    if unsafe { libc::kevent(queue.as_raw_fd(), &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) } == -1 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) { Ok(None) } else { Err(error) };
    }
    Ok(Some(queue))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn wait_until_exit(_identity: &ProcessIdentity, _within: Option<Duration>) -> io::Result<bool> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "host process observation is not implemented on this platform"))
}
