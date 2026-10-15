//! Observing a host's process lifetime without owning or terminating it.

use fastctx_types::ProcessIdentity;
use std::io;

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
#[cfg(windows)]
pub fn wait_for_exit(identity: &ProcessIdentity) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: the query/synchronize handle is owned here and always closed.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, identity.pid);
        if handle.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) { Ok(()) } else { Err(error) };
        }
        let result = (|| {
            if crate::identity::of_handle(handle, identity.pid)? != *identity {
                return Ok(());
            }
            if WaitForSingleObject(handle, INFINITE) == WAIT_OBJECT_0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        })();
        CloseHandle(handle);
        result
    }
}

#[cfg(target_os = "linux")]
pub fn wait_for_exit(identity: &ProcessIdentity) -> io::Result<()> {
    // SAFETY: opens a descriptor naming one lifetime, closed below.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) } as libc::c_int;
    if fd == -1 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error) };
    }
    let result = (|| {
        if !crate::is_alive(identity)? {
            return Ok(());
        }
        let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        loop {
            // SAFETY: poll receives one initialized descriptor.
            if unsafe { libc::poll(&mut poll, 1, -1) } >= 0 {
                return Ok(());
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

/// On macOS a kqueue tells of the process's exit; the registration comes
/// before the identity is checked, so an exit in between is not missed.
#[cfg(target_os = "macos")]
pub fn wait_for_exit(identity: &ProcessIdentity) -> io::Result<()> {
    let Some(queue) = exit_queue(identity.pid)? else { return Ok(()) };
    if !crate::is_alive(identity)? {
        return Ok(());
    }
    use std::os::fd::AsRawFd as _;
    loop {
        // SAFETY: a zeroed kevent is a valid output buffer for one event.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: waits on a kqueue owned here for one event, no timeout.
        let got = unsafe { libc::kevent(queue.as_raw_fd(), std::ptr::null(), 0, &mut event, 1, std::ptr::null()) };
        if got >= 0 {
            return Ok(());
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
pub fn wait_for_exit(_identity: &ProcessIdentity) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "host process observation is not implemented on this platform"))
}
