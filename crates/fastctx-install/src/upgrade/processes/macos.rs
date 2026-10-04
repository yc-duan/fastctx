use super::Process;
use std::io;
use std::path::{Path, PathBuf};

pub(super) fn enumerate() -> io::Result<(Vec<Process>, Vec<String>)> {
    // SAFETY: allocations are derived from the kernel's size query; buffers
    // and process records are checked before their contents are consumed.
    unsafe {
        let count = libc::proc_listallpids(std::ptr::null_mut(), 0);
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        // One extra slot distinguishes an exact fit from growth after the query.
        let mut pids = vec![0i32; count as usize + 1];
        let filled = libc::proc_listallpids(pids.as_mut_ptr().cast(), std::mem::size_of_val(pids.as_slice()) as i32);
        if filled < 0 {
            return Err(io::Error::last_os_error());
        }
        if filled as usize >= pids.len() {
            return Err(io::Error::other("process list changed while enumerating; cleanup deferred"));
        }
        pids.truncate(filled as usize);
        let mut result = Vec::new();
        let mut unavailable = Vec::new();
        for pid in pids {
            if pid <= 0 || pid as u32 == std::process::id() {
                continue;
            }
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let received = libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&raw mut info).cast(),
                std::mem::size_of_val(&info) as i32,
            );
            if received != std::mem::size_of_val(&info) as i32 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    unavailable.push(format!("PID {pid}: {error}"));
                }
                continue;
            }
            if info.pbi_uid != libc::geteuid() || info.pbi_status == 5 {
                continue;
            }
            let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
            let length = libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32);
            if length <= 0 {
                unavailable.push(format!("PID {pid}: {}", io::Error::last_os_error()));
                continue;
            }
            use std::os::unix::ffi::OsStrExt;
            let end = path.iter().position(|byte| *byte == 0).unwrap_or(length as usize);
            let image = PathBuf::from(std::ffi::OsStr::from_bytes(&path[..end]));
            result.push(Process { pid: pid as u32, image_source: image.clone(), image, legacy: false });
        }
        Ok((result, unavailable))
    }
}

pub(super) fn token(pid: u32) -> io::Result<Option<String>> {
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    // SAFETY: the output is initialized and exactly sized for PROC_PIDTBSDINFO.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let length = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            std::mem::size_of_val(&info) as i32,
        );
        if length == std::mem::size_of_val(&info) as i32 {
            return Ok((info.pbi_status != 5).then(|| format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec)));
        }
        // Signal zero only queries existence; no signal is delivered. A short
        // libproc response or denied query must not become permission to retire.
        if libc::kill(pid, 0) == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            Ok(None)
        } else {
            Err(io::Error::other("cannot determine legacy macOS process creation time"))
        }
    }
}

pub(super) fn references(process: &Process, path: &Path) -> io::Result<bool> {
    let pid = process.pid;
    // SAFETY: KERN_PROCARGS2 is sized by sysctl. Only argc arguments are
    // consumed; the environment following them is never inspected.
    unsafe {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
        let mut size = 0;
        if libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) != 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) { Ok(false) } else { Err(error) };
        }
        let mut bytes = vec![0u8; size];
        if libc::sysctl(mib.as_mut_ptr(), 3, bytes.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) != 0 {
            return Err(io::Error::last_os_error());
        }
        bytes.truncate(size);
        if bytes.len() < 4 {
            return Err(io::Error::other("invalid process argument snapshot"));
        }
        let argc = i32::from_ne_bytes(bytes[..4].try_into().expect("four bytes"));
        let mut rest = &bytes[4..];
        let executable =
            rest.iter().position(|byte| *byte == 0).ok_or_else(|| io::Error::other("missing executable terminator"))?;
        rest = &rest[executable..];
        while rest.first() == Some(&0) {
            rest = &rest[1..];
        }
        for _ in 0..argc {
            let end = rest
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| io::Error::other("missing argument terminator"))?;
            if super::command_references(&String::from_utf8_lossy(&rest[..end]), path) {
                return Ok(true);
            }
            rest = &rest[end + 1..];
        }
        Ok(false)
    }
}
