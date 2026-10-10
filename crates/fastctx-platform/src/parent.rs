//! Keeping an `on` follower with the shell function that owns it.

use std::io;

/// Ends this process when the shell that runs the `on` function ends, as
/// when the script kills it: leaving is how an `on` is stopped.
///
/// On Linux `parent` is the shell's process id, this process's parent: it
/// is checked, then the kernel ends this process when the parent ends. On
/// Windows it is the shell's native process id (Git Bash gives it in
/// `/proc/PID/winpid`): Git Bash starts native programs through stubs that
/// outlive the shell, so its end is waited for directly. This must be called
/// before following a job.
pub fn follow_shell(parent: u32) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: prctl changes only this process's parent-death signal;
        // getppid observes its parent, without accessing shared memory.
        unsafe {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() as u32 != parent {
                return Err(io::Error::other("the on shell stopped before its follower started"));
            }
        }
    }
    #[cfg(windows)]
    windows::follow(parent, "the on shell stopped before its follower started")?;
    #[cfg(not(any(target_os = "linux", windows)))]
    let _ = parent;
    Ok(())
}

/// Ends this process when the entry that forwarded it to the World program
/// ends. Windows has no exec, so a forwarded command runs as that entry's
/// child; following it makes the pair end together, as a program that had
/// replaced the entry would, and a caller that kills what it started leaves
/// nothing behind.
#[cfg(windows)]
pub fn follow_forwarder(forwarder: u32) -> io::Result<()> {
    windows::follow(forwarder, "the entry that forwarded this command ended before it started")
}

#[cfg(windows)]
mod windows {
    use std::io;

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    /// A process handle that may be moved to the thread waiting on it.
    struct Shell(HANDLE);
    // SAFETY: a process handle may be waited on from any thread.
    unsafe impl Send for Shell {}

    pub fn follow(pid: u32, gone: &'static str) -> io::Result<()> {
        let gone = || io::Error::other(gone);
        // SAFETY: the handle is owned here; it is closed on every early
        // return and otherwise kept open by the waiting thread.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                return Err(gone());
            }
            // The id may already name another process: the shell started
            // this one, so it was created first.
            let checked = match (created(handle), created(GetCurrentProcess())) {
                (Ok(shell), Ok(this)) if shell <= this && WaitForSingleObject(handle, 0) != WAIT_OBJECT_0 => Ok(()),
                (Err(error), _) | (_, Err(error)) => Err(error),
                _ => Err(gone()),
            };
            if let Err(error) = checked {
                CloseHandle(handle);
                return Err(error);
            }
            let shell = Shell(handle);
            std::thread::spawn(move || {
                let shell = shell;
                WaitForSingleObject(shell.0, INFINITE);
                // Nobody reads this code: the shell that would is gone.
                std::process::exit(1);
            });
        }
        Ok(())
    }

    fn created(handle: HANDLE) -> io::Result<u64> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: a process handle with query access and four out-parameters.
        if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(u64::from(creation.dwHighDateTime) << 32 | u64::from(creation.dwLowDateTime))
    }
}
