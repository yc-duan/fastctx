//! An isolated decoder's additional memory allowance, enforced by the OS.

use std::io;

#[cfg(windows)]
pub fn physical() -> io::Result<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of_val(&status) as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(status.ullTotalPhys)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn physical() -> io::Result<u64> {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if pages < 0 || page < 0 {
        return Err(io::Error::last_os_error());
    }
    (pages as u64).checked_mul(page as u64).ok_or_else(|| io::Error::other("physical memory overflow"))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn physical() -> io::Result<u64> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "isolated image decoding is not implemented on this platform"))
}

#[cfg(windows)]
pub struct Limit {
    _job: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
pub fn limit(additional: u64) -> io::Result<Limit> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let process = unsafe { GetCurrentProcess() };
    let mut usage: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    if unsafe { GetProcessMemoryInfo(process, &mut usage, std::mem::size_of_val(&usage) as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let job = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw) };
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = usize::try_from(additional)
        .ok()
        .and_then(|bytes| bytes.checked_add(usage.PagefileUsage))
        .ok_or_else(|| io::Error::other("decoder memory limit overflow"))?;
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
        || unsafe { AssignProcessToJobObject(job.as_raw_handle(), process) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Limit { _job: job })
}

#[cfg(not(windows))]
pub struct Limit;

#[cfg(target_os = "linux")]
pub fn limit(additional: u64) -> io::Result<Limit> {
    let status = std::fs::read_to_string("/proc/self/statm")?;
    let pages: u64 = status
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::other("missing process size"))?
        .parse()
        .map_err(io::Error::other)?;
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page < 0 {
        return Err(io::Error::last_os_error());
    }
    let bytes = pages
        .checked_mul(page as u64)
        .and_then(|bytes| bytes.checked_add(additional))
        .ok_or_else(|| io::Error::other("decoder address-space limit overflow"))?;
    let limit = libc::rlimit { rlim_cur: bytes, rlim_max: bytes };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Limit)
}

/// macOS enforces no limit on a process's address space: the decoder's own
/// limits on what it allocates bound it there.
#[cfg(target_os = "macos")]
pub fn limit(_: u64) -> io::Result<Limit> {
    Ok(Limit)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn limit(_: u64) -> io::Result<Limit> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "isolated image decoding is not implemented on this platform"))
}
