//! Memory: how much the machine has, how much this process holds, and an
//! isolated decoder's additional allowance, enforced by the OS.

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

/// The memory this process may use: the machine's, or less where the
/// control group it runs in (a container, a systemd unit's `MemoryMax`) caps
/// it lower. What is sized from this then fits under that cap.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn physical() -> io::Result<u64> {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if pages < 0 || page < 0 {
        return Err(io::Error::last_os_error());
    }
    let machine = (pages as u64).checked_mul(page as u64).ok_or_else(|| io::Error::other("physical memory overflow"))?;
    #[cfg(target_os = "linux")]
    if let Some(cap) = group_cap(&std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default(), |path| {
        std::fs::read_to_string(path).ok()
    }) {
        return Ok(machine.min(cap));
    }
    Ok(machine)
}

/// The tightest memory cap of the control group in `membership` (as
/// `/proc/self/cgroup` lists it) and of each group above it: version 2's
/// `memory.max`, else version 1's `memory.limit_in_bytes`; `None` when none
/// caps it. `read` reads a file of the control-group file system.
#[cfg(target_os = "linux")]
fn group_cap(membership: &str, read: impl Fn(&std::path::Path) -> Option<String>) -> Option<u64> {
    use std::path::Path;
    let mut cap: Option<u64> = None;
    let mut tighten = |value: Option<String>| {
        if let Some(bytes) = value.and_then(|text| text.trim().parse::<u64>().ok()) {
            cap = Some(cap.map_or(bytes, |cap| cap.min(bytes)));
        }
    };
    for line in membership.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(_), Some(controllers), Some(path)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let (root, file) = match controllers {
            "" => (Path::new("/sys/fs/cgroup").to_path_buf(), "memory.max"),
            list if list.split(',').any(|controller| controller == "memory") => {
                (Path::new("/sys/fs/cgroup/memory").to_path_buf(), "memory.limit_in_bytes")
            }
            _ => continue,
        };
        let mut group = Path::new(path.trim_start_matches('/'));
        loop {
            // "max" (version 2) does not parse and caps nothing; version 1
            // says no cap with a value past any machine, left to `min`.
            tighten(read(&root.join(group).join(file)));
            match group.parent() {
                Some(parent) if group != Path::new("") => group = parent,
                _ => break,
            }
        }
    }
    cap
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn physical() -> io::Result<u64> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "isolated image decoding is not implemented on this platform"))
}

/// The bytes of memory this process has resident now.
#[cfg(windows)]
pub fn resident() -> io::Result<u64> {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut usage: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut usage, std::mem::size_of_val(&usage) as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(usage.WorkingSetSize as u64)
}

/// The bytes of memory this process has resident now.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn resident() -> io::Result<u64> {
    let status = std::fs::read_to_string("/proc/self/statm")?;
    let pages: u64 = status
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| io::Error::other("missing resident size"))?
        .parse()
        .map_err(io::Error::other)?;
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page < 0 {
        return Err(io::Error::last_os_error());
    }
    pages.checked_mul(page as u64).ok_or_else(|| io::Error::other("resident size overflow"))
}

/// The bytes of memory this process has resident now.
#[cfg(target_os = "macos")]
pub fn resident() -> io::Result<u64> {
    // SAFETY: zeroed is a valid out-parameter for proc_pidinfo.
    let mut task: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly one proc_taskinfo.
    let read = unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            (&mut task as *mut libc::proc_taskinfo).cast(),
            size,
        )
    };
    if read != size {
        return Err(io::Error::last_os_error());
    }
    Ok(task.pti_resident_size)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android", target_os = "macos")))]
pub fn resident() -> io::Result<u64> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "this process's resident memory is not read on this platform"))
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::group_cap;
    use std::path::Path;

    #[test]
    fn a_group_and_its_parents_cap_memory() {
        let files = |path: &Path| match path.to_str()? {
            "/sys/fs/cgroup/system.slice/hub.service/memory.max" => Some("2147483648\n".into()),
            "/sys/fs/cgroup/system.slice/memory.max" => Some("max\n".into()),
            "/sys/fs/cgroup/memory.max" => None,
            "/sys/fs/cgroup/memory/docker/abc/memory.limit_in_bytes" => Some("1073741824\n".into()),
            "/sys/fs/cgroup/memory/docker/memory.limit_in_bytes" => Some("9223372036854771712\n".into()),
            _ => None,
        };
        assert_eq!(group_cap("0::/system.slice/hub.service\n", files), Some(2 << 30));
        assert_eq!(group_cap("0::/user.slice\n", files), None);
        assert_eq!(group_cap("4:memory:/docker/abc\n3:cpu,cpuacct:/docker/abc\n", files), Some(1 << 30));
    }
}
