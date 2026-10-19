//! Keeping a new program from inheriting descriptors by accident.

use std::io;

/// Marks every descriptor from `first` on close-on-exec, so nothing the
/// starter opened without that flag reaches the new program. Called between
/// fork and exec: async-signal-safe.
#[cfg(target_os = "linux")]
pub fn close_on_exec_from(first: u32) -> io::Result<()> {
    // SAFETY: a plain system call on this process's own descriptors.
    let marked = unsafe { libc::syscall(libc::SYS_close_range, first, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) };
    if marked == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        // Linux has close_range from 5.9 and marks with it from 5.11. An
        // older kernel, or a sandbox that refuses the call, has each
        // descriptor marked in turn.
        Some(libc::ENOSYS | libc::EINVAL | libc::EPERM) => each_from(first),
        _ => Err(error),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn close_on_exec_from(first: u32) -> io::Result<()> {
    each_from(first)
}

/// Marks each descriptor below the soft limit on open files, one by one.
/// That limit may read as unlimited, and no process of the World holds
/// anything near 65,536 descriptors.
fn each_from(first: u32) -> io::Result<()> {
    // SAFETY: getrlimit and fcntl are async-signal-safe.
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == -1 {
            return Err(io::Error::last_os_error());
        }
        for fd in first as libc::c_int..limit.rlim_cur.min(65_536) as libc::c_int {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    Ok(())
}
