//! Starting a process that outlives its starter.
//!
//! A detached process shares nothing with the process that started it: not
//! its session or console, not its environment, not a single inherited
//! handle, and nothing to be reaped. The environment is given whole by the
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

use crate::ProcessIdentity;

/// A process started on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetachedProcess {
    /// `None` when it had already ended by the time it was looked at.
    pub identity: Option<ProcessIdentity>,
    /// Windows put the starter in a job that forbids leaving it (the Task
    /// Scheduler does this to its tasks): the new process is in that job
    /// too and ends when the job ends.
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
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "program and working directory must be absolute paths"));
    }
    check_environment(env)?;
    imp::spawn_detached(program, args, cwd, env)
}

/// Rejects what the system would misread: an empty name, a name holding `=`
/// past its first character (Windows keeps per-drive directories in names
/// that start with one), a NUL anywhere, and a name given twice, which on
/// Windows means twice regardless of case.
fn check_environment(env: &[(OsString, OsString)]) -> io::Result<()> {
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
    use crate::ProcessIdentity;
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
        Ok(DetachedProcess { identity: ProcessIdentity::of(u32::from_ne_bytes(pid))?, ends_with_starters_job: false })
    }
}

#[cfg(windows)]
mod imp {
    use std::cmp::Ordering;
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED};
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CSTR_GREATER_THAN, CompareStringOrdinal};
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
        DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
    };

    use super::DetachedProcess;
    use crate::ProcessIdentity;
    use crate::command_line::windows_command_line;

    pub fn spawn_detached(
        program: &Path,
        args: &[&OsStr],
        cwd: &Path,
        env: &[(OsString, OsString)],
    ) -> io::Result<DetachedProcess> {
        let mut command_line = windows_command_line(program.as_os_str(), args)?;
        command_line.push(0);
        let block = environment_block(env);
        let (identity, ends_with_starters_job) = match create(program, &command_line, cwd, &block, true) {
            Ok(identity) => (identity, false),
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                (create(program, &command_line, cwd, &block, false)?, true)
            }
            Err(error) => return Err(error),
        };
        Ok(DetachedProcess { identity: Some(identity), ends_with_starters_job })
    }

    /// `NAME=value` strings, each NUL-terminated, sorted by name without
    /// regard to case as Windows requires, ending with one more NUL.
    fn environment_block(env: &[(OsString, OsString)]) -> Vec<u16> {
        let mut vars: Vec<(Vec<u16>, Vec<u16>)> =
            env.iter().map(|(name, value)| (name.encode_wide().collect(), value.encode_wide().collect())).collect();
        vars.sort_by(|(a, _), (b, _)| compare_names(a, b));
        let mut block = Vec::new();
        for (name, value) in vars {
            block.extend(name);
            block.push(u16::from(b'='));
            block.extend(value);
            block.push(0);
        }
        if block.is_empty() {
            block.push(0);
        }
        block.push(0);
        block
    }

    fn compare_names(a: &[u16], b: &[u16]) -> Ordering {
        // SAFETY: both slices are valid for their lengths, which fit in i32
        // for any name the system accepts.
        match unsafe { CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) } {
            CSTR_EQUAL => Ordering::Equal,
            CSTR_GREATER_THAN => Ordering::Greater,
            _ => Ordering::Less,
        }
    }

    fn create(program: &Path, command_line: &[u16], cwd: &Path, block: &[u16], breakaway: bool) -> io::Result<ProcessIdentity> {
        let application: Vec<u16> = program.as_os_str().encode_wide().chain([0]).collect();
        let mut command_line = command_line.to_vec();
        let directory: Vec<u16> = cwd.as_os_str().encode_wide().chain([0]).collect();
        let flags = DETACHED_PROCESS
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
                command_line.as_mut_ptr(),
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
        // The handle keeps the creation time readable even if the process
        // has already exited.
        let identity = crate::identity::of_handle(info.hProcess, info.dwProcessId);
        // SAFETY: handles returned by CreateProcessW, closed once; closing
        // them does not affect the process.
        unsafe {
            CloseHandle(info.hThread);
            CloseHandle(info.hProcess);
        }
        identity
    }
}
