//! Keeping a new program from inheriting descriptors by accident.

use std::io;

/// Marks every descriptor from `first` on close-on-exec. Called between
/// fork and exec; the Linux fallback uses only stack storage and syscalls.
#[cfg(target_os = "linux")]
pub fn close_on_exec_from(first: u32) -> io::Result<()> {
    // SAFETY: a plain system call on this process's own descriptors.
    let marked = unsafe { libc::syscall(libc::SYS_close_range, first, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) };
    if marked == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOSYS | libc::EINVAL | libc::EPERM) => {
            if from_proc(first)? {
                Ok(())
            } else {
                through_hard_limit(first)
            }
        }
        _ => Err(error),
    }
}

#[cfg(target_os = "linux")]
fn mark(fd: libc::c_int) -> io::Result<()> {
    // SAFETY: fcntl changes only this descriptor's inheritance flag.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EBADF) { Ok(()) } else { Err(error) }
}

#[cfg(target_os = "linux")]
fn from_proc(first: u32) -> io::Result<bool> {
    // SAFETY: a static terminated path; the returned descriptor is closed below.
    let directory =
        unsafe { libc::open(c"/proc/self/fd".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if directory < 0 {
        return Ok(false);
    }
    let result = read_descriptors(directory, first);
    // On Linux, even EINTR from close releases the descriptor; never retry it.
    // SAFETY: this function alone owns the directory descriptor.
    let closed = unsafe { libc::close(directory) };
    if closed != 0 && result.is_ok() {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(target_os = "linux")]
fn read_descriptors(directory: libc::c_int, first: u32) -> io::Result<bool> {
    let mut buffer = [0_u8; 8192];
    loop {
        // SAFETY: getdents64 writes at most the supplied stack buffer's length.
        let count = unsafe { libc::syscall(libc::SYS_getdents64, directory, buffer.as_mut_ptr(), buffer.len()) };
        if count < 0 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Ok(false);
        }
        if count == 0 {
            return Ok(true);
        }
        if count as usize > buffer.len() {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        let mut at = 0;
        while at < count as usize {
            // linux_dirent64 has a u16 record length at 16 and its name at 19.
            if at + 19 > count as usize {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            let length = u16::from_ne_bytes([buffer[at + 16], buffer[at + 17]]) as usize;
            if length < 20 || at + length > count as usize {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            let name = &buffer[at + 19..at + length];
            let mut number = Some(0_u32);
            let mut digits = 0;
            for &byte in name.iter().take_while(|&&byte| byte != 0) {
                if !byte.is_ascii_digit() {
                    number = None;
                    break;
                }
                number = number.and_then(|n| n.checked_mul(10)?.checked_add(u32::from(byte - b'0')));
                digits += 1;
            }
            if let Some(number) = number.filter(|&n| digits != 0 && n >= first && n <= libc::c_int::MAX as u32) {
                let fd = number as libc::c_int;
                if fd != directory {
                    mark(fd)?;
                }
            }
            at += length;
        }
    }
}

#[cfg(target_os = "linux")]
fn through_hard_limit(first: u32) -> io::Result<()> {
    // A lowered soft limit does not close descriptors that were opened before it.
    // SAFETY: getrlimit writes one initialized structure.
    let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let end = limit.rlim_max.min(libc::c_int::MAX as libc::rlim_t + 1);
    for fd in first as libc::rlim_t..end {
        mark(fd as libc::c_int)?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn close_on_exec_from(first: u32) -> io::Result<()> {
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
