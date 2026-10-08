//! Keeping an `on` follower with the shell function that owns it.

use std::io;

/// On Linux, terminates this process when its shell parent terminates.
/// The supplied PID also detects a parent that exited before registration.
/// This must be called on the process's main thread before following a job.
///
/// MSYS PIDs are not Windows process identifiers. Windows retains its
/// existing process ownership; this Linux mechanism is not applied there.
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
    #[cfg(not(target_os = "linux"))]
    let _ = parent;
    Ok(())
}
