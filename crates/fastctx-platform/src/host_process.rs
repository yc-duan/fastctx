//! Observing a host's process lifetime without owning or terminating it.

use fastctx_types::ProcessIdentity;
use std::io;

mod asynchronous;
pub use asynchronous::exited;

pub fn parent_identity() -> io::Result<ProcessIdentity> {
    crate::identity_of(parent_pid()?)?.ok_or_else(|| io::Error::other("the parent process already exited"))
}

#[cfg(unix)]
fn parent_pid() -> io::Result<u32> {
    // SAFETY: getppid has no pointers or ownership requirements.
    Ok(unsafe { libc::getppid() } as u32)
}

#[cfg(windows)]
fn parent_pid() -> io::Result<u32> {
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
            if entry.th32ProcessID == std::process::id() {
                found = Some(entry.th32ParentProcessID);
                break;
            }
            more = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        found.ok_or_else(|| io::Error::other("the current process was absent from the process snapshot"))
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

#[cfg(not(any(windows, target_os = "linux")))]
pub fn wait_for_exit(_identity: &ProcessIdentity) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "host process observation is not implemented on this platform"))
}
