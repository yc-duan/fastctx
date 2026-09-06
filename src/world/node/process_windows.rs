//! Detached node startup without inheriting a control helper's capture pipes.
//!
//! Redirecting stdio with Command also enables Windows handle inheritance; unrelated pipe
//! writers can then keep the helper caller waiting for EOF for the daemon's lifetime. The
//! node opens its own log after startup instead. (2026-09-06)

use crate::control::paths::ControlPaths;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW,
    DETACHED_PROCESS, GetCurrentProcess, PROCESS_INFORMATION, STARTUPINFOW,
};

pub(super) fn start(paths: &ControlPaths) -> Result<u32, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("Cannot locate FastCtx: {error}"))?;
    let application = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let directory = paths
        .home
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut arguments = Vec::new();
    for argument in [
        executable.as_os_str(),
        OsStr::new("node"),
        OsStr::new("run"),
        OsStr::new("--home"),
        paths.home.as_os_str(),
    ] {
        if !arguments.is_empty() {
            arguments.push(b' ' as u16);
        }
        quote(argument, &mut arguments);
    }
    arguments.push(0);
    let spawn = |flags| -> std::io::Result<u32> {
        let mut command = arguments.clone();
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: all strings are live NUL-terminated buffers; no handle is inherited.
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                flags,
                std::ptr::null(),
                directory.as_ptr(),
                &startup,
                &mut process,
            )
        };
        if created == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let pid = process.dwProcessId;
        // SAFETY: successful creation transfers ownership of both process handles here.
        unsafe {
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
        }
        Ok(pid)
    };
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
    match spawn(flags | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(pid) => Ok(pid),
        Err(error) if error.raw_os_error() == Some(5) => {
            let mut in_job = 0;
            // A child inside a non-breakaway job dies with its caller even when detached.
            // Reporting independent startup here would be false success (2026-09-06).
            if unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) }
                == 0
            {
                return Err(format!(
                    "Cannot inspect the caller's process job: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if in_job != 0 {
                return Err("Permission denied: Windows does not allow the node to leave the caller's process job. Install the node service from Services to start it independently.".into());
            }
            spawn(flags).map_err(|error| format!("Cannot start the detached node: {error}"))
        }
        Err(error) => Err(format!("Cannot start the detached node: {error}")),
    }
}

fn quote(argument: &OsStr, output: &mut Vec<u16>) {
    output.push(b'"' as u16);
    let mut slashes = 0;
    for unit in argument.encode_wide() {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        output.extend(std::iter::repeat_n(
            b'\\' as u16,
            if unit == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        slashes = 0;
        output.push(unit);
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
}
