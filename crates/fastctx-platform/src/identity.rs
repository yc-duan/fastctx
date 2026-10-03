//! Identifying one lifetime of a process.
//!
//! A PID alone does not: the system reuses it. A process is its PID plus a
//! token that changes whenever the PID is reused (on Linux the boot and the
//! start time, on Windows the creation time). Asking whether an identity is
//! alive answers for that lifetime only.

use std::io;

/// One lifetime of a process.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Opaque; compared, stored and shown, never parsed by callers.
    pub token: String,
}

impl ProcessIdentity {
    /// The identity of the process with `pid` now, or `None` if no process
    /// with that PID is running.
    pub fn of(pid: u32) -> io::Result<Option<Self>> {
        Ok(imp::token(pid)?.map(|token| Self { pid, token }))
    }

    /// The identity of the calling process.
    pub fn current() -> io::Result<Self> {
        Self::of(std::process::id())?.ok_or_else(|| io::Error::other("the current process has no identity"))
    }

    /// Whether this lifetime of the process is still running.
    pub fn is_alive(&self) -> io::Result<bool> {
        Ok(imp::token(self.pid)?.as_deref() == Some(self.token.as_str()))
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::io;

    use crate::procfs;

    /// `boot_id:starttime`: the start time counts clock ticks since boot, so
    /// the boot it belongs to is part of it.
    pub fn token(pid: u32) -> io::Result<Option<String>> {
        match procfs::stat(pid)? {
            Some(stat) if !stat.has_exited() => Ok(Some(format!("{}:{}", procfs::boot_id()?, stat.start))),
            _ => Ok(None),
        }
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
mod imp {
    use std::io;
    use std::process::Command;

    /// Without /proc, `ps` gives the start time; at one-second precision a
    /// reused PID within the same second would look like the same process.
    pub fn token(pid: u32) -> io::Result<Option<String>> {
        let output = Command::new("ps").args(["-p", &pid.to_string(), "-o", "lstart=", "-o", "stat="]).env("LC_ALL", "C").output()?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || text.is_empty() || text.ends_with('Z') {
            return Ok(None);
        }
        Ok(Some(text))
    }
}

#[cfg(windows)]
pub(crate) use imp::of_handle;

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    use super::ProcessIdentity;

    pub fn token(pid: u32) -> io::Result<Option<String>> {
        // SAFETY: plain handle-based queries; the handle is closed below.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                let error = io::Error::last_os_error();
                return match error.raw_os_error() {
                    Some(code) if code == ERROR_INVALID_PARAMETER as i32 => Ok(None),
                    _ => Err(error),
                };
            }
            // An exited process keeps its handle valid while anyone holds one.
            let result =
                if WaitForSingleObject(handle, 0) == WAIT_OBJECT_0 { Ok(None) } else { creation_time(handle).map(Some) };
            CloseHandle(handle);
            result
        }
    }

    /// The identity of the process behind `handle`, which has query access.
    pub fn of_handle(handle: HANDLE, pid: u32) -> io::Result<ProcessIdentity> {
        Ok(ProcessIdentity { pid, token: creation_time(handle)? })
    }

    /// The creation time, in 100-nanosecond units since 1601.
    fn creation_time(handle: HANDLE) -> io::Result<String> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: a valid process handle and four out-parameters.
        if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((u64::from(creation.dwHighDateTime) << 32 | u64::from(creation.dwLowDateTime)).to_string())
    }
}
