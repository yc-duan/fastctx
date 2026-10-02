//! OS detachment and process identities without kill-on-drop ownership.

use std::io;
use std::path::Path;
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;

pub(super) fn launch(
    executable: &Path,
    request: &Path,
    diagnostics: &std::fs::File,
) -> io::Result<()> {
    #[cfg(windows)]
    {
        let _ = diagnostics;
        super::windows_launch::launch(executable, request)
    }
    #[cfg(unix)]
    {
        let mut command = Command::new(executable);
        command.arg("run-bootstrap");
        command
            .arg(request)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(diagnostics.try_clone()?);
        let mut child = command.spawn()?;
        std::thread::spawn(move || {
            if let Err(error) = child.wait() {
                eprintln!("cannot reap run bootstrap: {error}");
            }
        });
        Ok(())
    }
}

#[cfg(unix)]
pub(super) fn bootstrap(request: &Path) -> io::Result<()> {
    // SAFETY: this fresh exec process is not a process-group leader. setsid detaches its TTY.
    if unsafe { libc::setsid() } < 0 {
        return Err(io::Error::last_os_error());
    }
    Command::new(std::env::current_exe()?)
        .arg("run-host")
        .arg(request)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    // The final supervisor is not a session leader, cannot regain a controlling TTY,
    // and is reparented when this bootstrap exits. No parent-death signal is installed.
    Ok(())
}

pub(super) fn configure_bash(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    crate::process_policy::apply_noninteractive_policy(command);
}

pub(super) fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().expect("exit has a code or signal"))
    }
    #[cfg(windows)]
    {
        status.code().expect("Windows exits have a status code")
    }
}

#[cfg(windows)]
pub(super) fn identity(pid: u32) -> io::Result<Option<String>> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, WAIT_FAILED, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        WaitForSingleObject,
    };
    // SAFETY: the process handle is owned here and closed after the queries.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let result = unsafe {
        let state = WaitForSingleObject(handle, 0);
        if state == WAIT_FAILED
            || GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) == 0
        {
            Err(io::Error::last_os_error())
        } else if state == WAIT_OBJECT_0 {
            Ok(None)
        } else {
            Ok(Some(format!(
                "{}:{}",
                created.dwHighDateTime, created.dwLowDateTime
            )))
        }
    };
    unsafe {
        CloseHandle(handle);
    }
    result
}

pub(super) struct Process {
    child: std::process::Child,
    #[cfg(windows)]
    job: std::os::windows::io::OwnedHandle,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        let status = self.child.wait()?;
        #[cfg(target_os = "linux")]
        while group_alive(self.child.id())? {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::{
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
                QueryInformationJobObject,
            };
            loop {
                let mut information = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
                // SAFETY: the job and output buffer remain live throughout the query.
                if unsafe {
                    QueryInformationJobObject(
                        self.job.as_raw_handle().cast(),
                        JobObjectBasicAccountingInformation,
                        (&mut information as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                        std::mem::size_of_val(&information) as u32,
                        std::ptr::null_mut(),
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                if information.ActiveProcesses == 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        Ok(status)
    }
}

#[cfg(target_os = "linux")]
fn group_alive(group: u32) -> io::Result<bool> {
    // The shell can exit before a background child that closed stdout/stderr. Waiting
    // for the process group matches Windows Job accounting without killing that child.
    // Zombies have already ended and may remain until the machine's reaper collects them.
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let mut fields = stat
            .rsplit_once(')')
            .ok_or_else(|| io::Error::other("invalid /proc stat"))?
            .1
            .split_whitespace();
        let state = fields.next();
        let id = fields
            .nth(1)
            .ok_or_else(|| io::Error::other("missing /proc process group"))?
            .parse::<u32>()
            .map_err(io::Error::other)?;
        if id == group && !matches!(state, Some("Z" | "X")) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn spawn(command: &mut Command) -> io::Result<Process> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
        command.creation_flags(crate::process_policy::noninteractive_creation_flags(
            CREATE_SUSPENDED,
        ));
        let mut child = command.spawn()?;
        let setup = (|| {
            let job = crate::shell::process::create_kill_on_close_job(child.as_raw_handle())?;
            crate::shell::process::resume_process_threads(child.id())?;
            Ok::<_, io::Error>(job)
        })();
        match setup {
            Ok(job) => Ok(Process { child, job }),
            Err(error) => {
                child.kill()?;
                child.wait()?;
                Err(error)
            }
        }
    }
    #[cfg(unix)]
    {
        Ok(Process {
            child: command.spawn()?,
        })
    }
}

#[cfg(target_os = "linux")]
pub(super) fn identity(pid: u32) -> io::Result<Option<String>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .ok_or_else(|| io::Error::other("invalid /proc stat"))?
        .1
        .split_whitespace()
        .collect();
    if matches!(fields.first(), Some(&"Z" | &"X")) {
        return Ok(None);
    }
    let started = fields
        .get(19)
        .ok_or_else(|| io::Error::other("missing /proc start time"))?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok(Some(format!("{}:{started}", boot.trim())))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn identity(pid: u32) -> io::Result<Option<String>> {
    Ok(crate::process_identity::process_identity(pid).map(|identity| identity.started))
}
