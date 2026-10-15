//! A public CLI mutation must not outlive the installer and race its recovery.

use std::io;
use std::path::Path;
use std::process::Output;

#[cfg(windows)]
pub(crate) fn output(program: &Path, args: &[String], cwd: &Path, settings_home: &Path) -> io::Result<Output> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtResumeProcess(process: HANDLE) -> i32;
    }

    // SAFETY: an unnamed, non-inheritable job with default security.
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: this function owns the new handle.
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: initialized structure and a live owned job handle.
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // std::Command retains its checked .cmd argument quoting. Suspending the
    // initial thread prevents even a batch wrapper from escaping the new job.
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("CLAUDE_CONFIG_DIR", ordinary_path(settings_home))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
        .spawn()?;
    // SAFETY: the child is suspended and both process/job handles are live.
    if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
        let error = io::Error::last_os_error();
        child.kill()?;
        child.wait()?;
        return Err(error);
    }
    // SAFETY: only this newly spawned process is resumed, after job assignment.
    let status = unsafe { NtResumeProcess(child.as_raw_handle()) };
    if status != 0 {
        drop(job);
        child.wait()?;
        return Err(io::Error::other(format!("resuming Claude returned NTSTATUS {status:#x}")));
    }
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    std::thread::scope(|scope| {
        let out = scope.spawn(|| read(&mut stdout));
        let err = scope.spawn(|| read(&mut stderr));
        let status = child.wait();
        // Closing the job also closes streams inherited by residual children.
        drop(job);
        Ok(Output { status: status?, stdout: joined(out.join())?, stderr: joined(err.join())? })
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn output(program: &Path, args: &[String], cwd: &Path, settings_home: &Path) -> io::Result<Output> {
    use std::ffi::OsString;
    let mut environment: Vec<(OsString, OsString)> =
        std::env::vars_os().filter(|(key, _)| key != "CLAUDE_CONFIG_DIR").collect();
    environment.push(("CLAUDE_CONFIG_DIR".into(), settings_home.into()));
    let arguments: Vec<_> = args.iter().map(std::ffi::OsStr::new).collect();
    let mut child = fastctx_platform::Child::spawn(program, &arguments, cwd, &environment)?;
    drop(child.stdin.take());
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    std::thread::scope(|scope| {
        let out = scope.spawn(|| read(&mut stdout));
        let err = scope.spawn(|| read(&mut stderr));
        let status = child.wait();
        drop(child);
        Ok(Output { status: status?, stdout: joined(out.join())?, stderr: joined(err.join())? })
    })
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) fn output(_program: &Path, _args: &[String], _cwd: &Path, _settings_home: &Path) -> io::Result<Output> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "installer-owned CLI processes are not implemented on this platform",
    ))
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn read(stream: &mut impl io::Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn joined(result: std::thread::Result<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    result.map_err(|_| io::Error::other("Claude output reader panicked"))?
}

#[cfg(windows)]
fn ordinary_path(path: &Path) -> std::path::PathBuf {
    use std::path::{Component, Prefix};
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else { return path.to_owned() };
    let mut result = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => std::path::PathBuf::from(format!("{}:\\", drive as char)),
        Prefix::VerbatimUNC(server, share) => std::path::PathBuf::from(r"\\").join(server).join(share),
        _ => return path.to_owned(),
    };
    for component in components {
        if !matches!(component, Component::RootDir) {
            result.push(component);
        }
    }
    result
}
