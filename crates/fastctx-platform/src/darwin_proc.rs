//! Processes as macOS shows them through libproc: each one's parent, process
//! group, session, start time and image, read without spawning a program.

use std::io;

/// What the World reads of one process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    /// Microseconds since the epoch at which the process started: with the
    /// PID, it names the process for good.
    pub start: u64,
    /// Exited, not yet reaped.
    pub zombie: bool,
}

impl Info {
    /// The session the process belongs to, or `None` once it is gone.
    pub fn session(&self) -> Option<u32> {
        // SAFETY: a query about a PID; no memory is passed.
        let sid = unsafe { libc::getsid(self.pid as libc::pid_t) };
        (sid >= 0).then_some(sid as u32)
    }
}

/// The process with `pid`, or `None` if there is none.
pub fn info(pid: u32) -> io::Result<Option<Info>> {
    // SAFETY: zeroed is a valid out-parameter for proc_pidinfo.
    let mut bsd: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly one proc_bsdinfo.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut bsd as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if read != size {
        let error = io::Error::last_os_error();
        // ESRCH: no such process; EPERM: one of another user's, which the
        // World neither owns nor may signal.
        return match error.raw_os_error() {
            Some(libc::ESRCH) | Some(libc::EPERM) | Some(0) | None => Ok(None),
            _ => Err(error),
        };
    }
    Ok(Some(Info {
        pid,
        ppid: bsd.pbi_ppid,
        pgid: bsd.pbi_pgid,
        start: bsd.pbi_start_tvsec.saturating_mul(1_000_000).saturating_add(bsd.pbi_start_tvusec),
        zombie: bsd.pbi_status == libc::SZOMB,
    }))
}

/// Every process now that can be read. One that exits while the list is
/// being read is left out.
pub fn all() -> io::Result<Vec<Info>> {
    let mut found = Vec::new();
    for pid in pids()? {
        if let Some(info) = info(pid)? {
            found.push(info);
        }
    }
    Ok(found)
}

/// The PIDs of every process now.
fn pids() -> io::Result<Vec<u32>> {
    // SAFETY: a null buffer asks only how many there are.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    // Room for those that start meanwhile.
    let mut pids = vec![0 as libc::pid_t; count as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer holds `bytes` bytes of PIDs.
    let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if listed < 0 {
        return Err(io::Error::last_os_error());
    }
    pids.truncate(listed as usize);
    Ok(pids.into_iter().filter(|pid| *pid > 0).map(|pid| pid as u32).collect())
}

/// The image `pid` runs, or `None` once it is gone or cannot be read.
pub fn image(pid: u32) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt as _;
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer holds PROC_PIDPATHINFO_MAXSIZE bytes.
    let length = unsafe { libc::proc_pidpath(pid as libc::c_int, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return None;
    }
    buffer.truncate(length as usize);
    Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(buffer)))
}
