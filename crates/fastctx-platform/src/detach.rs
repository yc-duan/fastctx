//! Starting a process that outlives its starter.
//!
//! A detached process inherits no console, environment or handles and
//! leaves nothing to be reaped. Windows host job restrictions still apply:
//! detaching from a console does not detach from a job. The environment is
//! given whole by the
//! caller because the starter's own may belong to someone else: a client
//! started inside an agent's session carries that session's variables, and
//! a node that inherited them would hand them to every command it runs.
//! Handles matter on Windows above all: a pipe writer inherited by accident
//! keeps whoever reads that pipe waiting for an end of file that comes only
//! when the detached process exits. The detached process opens whatever
//! files it needs by itself.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;

use fastctx_types::ProcessIdentity;

/// A process started on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetachedProcess {
    /// `None` when it had already ended by the time it was looked at.
    pub identity: Option<ProcessIdentity>,
    /// The new process still belongs to a Windows job. A successful
    /// breakaway may leave it in an ancestor job; killing that job kills
    /// this process too. Closing a handle kills it only with kill-on-close.
    pub ends_with_starters_job: bool,
}

/// Starts `program` with `args` in `cwd` with exactly the environment `env`,
/// and returns once it is running on its own. `program` and `cwd` must be
/// absolute; no search path is consulted.
pub fn spawn_detached(
    program: &Path,
    args: &[&OsStr],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> io::Result<DetachedProcess> {
    if !program.is_absolute() || !cwd.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "program and working directory must be absolute paths",
        ));
    }
    check_environment(env)?;
    imp::spawn_detached(program, args, cwd, env)
}

/// Rejects what the system would misread: an empty name, a name holding `=`
/// past its first character (Windows keeps per-drive directories in names
/// that start with one), a NUL anywhere, and a name given twice, which on
/// Windows means twice regardless of case.
pub(crate) fn check_environment(env: &[(OsString, OsString)]) -> io::Result<()> {
    let invalid = |what: &str| Err(io::Error::new(io::ErrorKind::InvalidInput, format!("environment: {what}")));
    let mut seen = std::collections::HashSet::new();
    for (name, value) in env {
        let name_bytes = name.as_encoded_bytes();
        if name_bytes.is_empty() || name_bytes[1..].contains(&b'=') {
            return invalid("a name is empty or contains '='");
        }
        if name_bytes.contains(&0) || value.as_encoded_bytes().contains(&0) {
            return invalid("a name or value contains NUL");
        }
        let key = if cfg!(windows) { name.to_string_lossy().to_uppercase().into() } else { name.clone() };
        if !seen.insert(key) {
            return invalid("a name is given twice");
        }
    }
    Ok(())
}

#[cfg(unix)]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::io::{self, Read as _};
    use std::os::fd::AsRawFd as _;
    use std::os::unix::process::CommandExt as _;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use super::DetachedProcess;
    use crate::identity::identity_of;
    use crate::unix_fds::close_on_exec_from;

    pub fn spawn_detached(
        program: &Path,
        args: &[&OsStr],
        cwd: &Path,
        env: &[(OsString, OsString)],
    ) -> io::Result<DetachedProcess> {
        // The first child tells the starter its own child's PID through
        // this pipe; both ends close on exec.
        let (mut pid_reader, pid_writer) = io::pipe()?;
        let pid_fd = pid_writer.as_raw_fd();
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: only async-signal-safe calls between fork and exec. The
        // first child starts a new session, forks once more, reports the new
        // PID and exits, so the process that execs belongs to init, not to
        // the starter, and, not being a session leader, never gains a
        // controlling terminal. The grandchild inherits the standard
        // library's close-on-exec error pipe, so a failed exec is still
        // reported to the starter.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                match libc::fork() {
                    -1 => Err(io::Error::last_os_error()),
                    0 => close_on_exec_from(3),
                    grandchild => {
                        let pid = (grandchild as u32).to_ne_bytes();
                        libc::write(pid_fd, pid.as_ptr().cast(), pid.len());
                        libc::_exit(0)
                    }
                }
            });
        }
        let spawned = command.spawn();
        drop(pid_writer);
        // The first child exits at once; reaping it leaves nothing behind.
        spawned?.wait()?;
        let mut pid = [0u8; 4];
        pid_reader.read_exact(&mut pid)?;
        Ok(DetachedProcess { identity: identity_of(u32::from_ne_bytes(pid))?, ends_with_starters_job: false })
    }
}

#[cfg(windows)]
pub(crate) mod imp {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED};
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
        CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    use super::DetachedProcess;
    use crate::windows_process::{command_line, environment_block, wide};

    pub fn spawn_detached(
        program: &Path,
        args: &[&OsStr],
        cwd: &Path,
        env: &[(OsString, OsString)],
    ) -> io::Result<DetachedProcess> {
        let line = command_line(program.as_os_str(), args)?;
        let block = environment_block(env);
        let info = match create(program, &line, cwd, &block, true) {
            Ok(info) => info,
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                create(program, &line, cwd, &block, false)?
            }
            Err(error) => return Err(error),
        };
        observe(info)
    }

    fn create(
        program: &Path,
        line: &[u16],
        cwd: &Path,
        block: &[u16],
        breakaway: bool,
    ) -> io::Result<PROCESS_INFORMATION> {
        let application = wide(program);
        let mut line = line.to_vec();
        let directory = wide(cwd);
        // A console of its own that has no window, rather than none: every
        // console program it starts, and those start in turn, shares that
        // console instead of opening a window of its own on the desktop.
        let flags = CREATE_NO_WINDOW
            | CREATE_NEW_PROCESS_GROUP
            | CREATE_UNICODE_ENVIRONMENT
            | if breakaway { CREATE_BREAKAWAY_FROM_JOB } else { 0 };
        // SAFETY: zeroed STARTUPINFOW with its size set is a valid default.
        let mut startup: STARTUPINFOW = unsafe { zeroed() };
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        // SAFETY: zeroed is valid for an out-parameter.
        let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
        // SAFETY: NUL-terminated strings and a double-NUL-terminated
        // environment block; no handle is inherited (FALSE), so nothing of
        // the starter's reaches the new process.
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                flags,
                block.as_ptr().cast(),
                directory.as_ptr(),
                &startup,
                &mut info,
            )
        };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }

    // A failed observation after creation must never retry process creation.
    pub(crate) fn observe(info: PROCESS_INFORMATION) -> io::Result<DetachedProcess> {
        // The handle keeps the creation time readable even if the process
        // has already exited.
        let identity = crate::identity::of_handle(info.hProcess, info.dwProcessId);
        let mut in_job = 0;
        // SAFETY: the process handle remains open; NULL asks about any job
        // in the child's hierarchy, including a nonbreakaway ancestor.
        let membership = if unsafe { IsProcessInJob(info.hProcess, ptr::null_mut(), &mut in_job) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(in_job != 0)
        };
        // SAFETY: handles returned by CreateProcessW, closed once; closing
        // them does not affect the process.
        unsafe {
            CloseHandle(info.hThread);
            CloseHandle(info.hProcess);
        }
        Ok(DetachedProcess { identity: Some(identity?), ends_with_starters_job: membership? })
    }
}
