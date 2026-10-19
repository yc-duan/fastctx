//! Ending a positively identified, owned process. Callers must establish
//! ownership separately; the token check protects against PID reuse only.

use fastctx_types::ProcessIdentity;
use std::io;

/// Nothing names a process for good on macOS: its start time, read just
/// before the signal, tells a reused PID.
#[cfg(target_os = "macos")]
pub fn terminate(identity: &ProcessIdentity) -> io::Result<()> {
    by_pid(identity)
}

/// Kills the process by its PID once its start time shows it is still this
/// lifetime.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn by_pid(identity: &ProcessIdentity) -> io::Result<()> {
    if !crate::is_alive(identity)? {
        return Ok(());
    }
    // SAFETY: a signal to the process just identified.
    if unsafe { libc::kill(identity.pid as libc::pid_t, libc::SIGKILL) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn terminate(identity: &ProcessIdentity) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    };
    // SAFETY: the handle names one process lifetime, is checked before acting,
    // and is closed on every path. No descendant or unrelated PID is selected.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE, 0, identity.pid);
        if process.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) { Ok(()) } else { Err(error) };
        }
        let result = (|| {
            if crate::identity::of_handle(process, identity.pid)? != *identity {
                return Ok(());
            }
            if TerminateProcess(process, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })();
        CloseHandle(process);
        result
    }
}

#[cfg(target_os = "linux")]
pub fn terminate(identity: &ProcessIdentity) -> io::Result<()> {
    // SAFETY: pidfd_open returns an owned descriptor naming a lifetime.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) } as libc::c_int;
    if fd == -1 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(()),
            // Linux names a process by a descriptor from 5.3 on. An older
            // kernel, or a sandbox that refuses the call, has it killed as
            // macOS does.
            Some(libc::ENOSYS | libc::EPERM) => by_pid(identity),
            _ => Err(error),
        };
    }
    let result = (|| {
        if !crate::is_alive(identity)? {
            return Ok(());
        }
        // SAFETY: sending through the descriptor cannot target a reused PID.
        if unsafe {
            libc::syscall(libc::SYS_pidfd_send_signal, fd, libc::SIGKILL, std::ptr::null::<libc::siginfo_t>(), 0)
        } == -1
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    })();
    // SAFETY: this function owns the descriptor.
    unsafe {
        libc::close(fd);
    }
    result
}
