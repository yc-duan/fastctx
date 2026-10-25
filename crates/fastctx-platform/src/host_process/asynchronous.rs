//! Cancellable process observation: dropping the future releases its OS registration.

use fastctx_types::ProcessIdentity;
use std::io;

#[cfg(windows)]
pub async fn exited(identity: &ProcessIdentity) -> io::Result<()> {
    use std::ffi::c_void;
    use std::sync::Mutex;
    use tokio::sync::oneshot;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, RegisterWaitForSingleObject,
        UnregisterWaitEx, WT_EXECUTEONLYONCE,
    };

    type Completion = Mutex<Option<oneshot::Sender<()>>>;
    unsafe extern "system" fn complete(context: *mut c_void, _: bool) {
        // SAFETY: Wait owns this allocation until UnregisterWaitEx has joined the callback.
        let completion = unsafe { &*context.cast::<Completion>() };
        if let Some(sender) = completion.lock().expect("process completion poisoned").take() {
            let _ = sender.send(());
        }
    }

    struct Wait {
        process: HANDLE,
        registration: HANDLE,
        completion: Box<Completion>,
    }
    // SAFETY: these owned OS handles may be closed on any thread; the callback uses a mutex.
    unsafe impl Send for Wait {}
    impl Drop for Wait {
        fn drop(&mut self) {
            // SAFETY: unregister waits for the callback before its context and process are freed.
            unsafe {
                if !self.registration.is_null() && UnregisterWaitEx(self.registration, INVALID_HANDLE_VALUE) == 0 {
                    eprintln!("could not release a process wait: {}", io::Error::last_os_error());
                    // The callback still owns its context if cancellation could not be proved.
                    std::process::abort();
                }
                CloseHandle(self.process);
            }
        }
    }

    // SAFETY: a query/synchronize handle is owned by Wait immediately after successful opening.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, identity.pid) };
    if process.is_null() {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) { Ok(()) } else { Err(error) };
    }
    let (sender, receiver) = oneshot::channel();
    let mut wait = Wait { process, registration: std::ptr::null_mut(), completion: Box::new(Mutex::new(Some(sender))) };
    if crate::identity::of_handle(process, identity.pid)? != *identity {
        return Ok(());
    }
    // SAFETY: both the process and the stable boxed callback context outlive this registration.
    if unsafe {
        RegisterWaitForSingleObject(
            &mut wait.registration,
            process,
            Some(complete),
            (&mut *wait.completion as *mut Completion).cast(),
            INFINITE,
            WT_EXECUTEONLYONCE,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    receiver.await.map_err(|error| io::Error::other(format!("process observation stopped: {error}")))
}

#[cfg(target_os = "linux")]
pub async fn exited(identity: &ProcessIdentity) -> io::Result<()> {
    use std::os::fd::{FromRawFd, OwnedFd};

    // SAFETY: pidfd_open returns an owned descriptor, adopted immediately below.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) } as libc::c_int;
    if fd == -1 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(()),
            // Linux opens a process as a descriptor from 5.3 on. An older
            // kernel, or a sandbox that refuses the call, is asked again
            // and again.
            // EINVAL also occurs while Linux retires a PID's task. Confirm
            // the identity's exit through procfs instead of losing the watch.
            Some(libc::ENOSYS | libc::EPERM | libc::EINVAL) => {
                while !crate::has_ended(identity)? {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Ok(())
            }
            _ => Err(error),
        };
    }
    // SAFETY: fd was just opened successfully and has no other owner.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if crate::has_ended(identity)? {
        return Ok(());
    }
    let watched = tokio::io::unix::AsyncFd::new(fd)?;
    let _ready = watched.readable().await?;
    Ok(())
}

/// On macOS the kqueue watching the process becomes readable when it exits.
/// A kqueue takes no write filter, so it is watched for reading alone.
#[cfg(target_os = "macos")]
pub async fn exited(identity: &ProcessIdentity) -> io::Result<()> {
    let Some(queue) = super::exit_queue(identity.pid)? else { return Ok(()) };
    if crate::has_ended(identity)? {
        return Ok(());
    }
    let watched = tokio::io::unix::AsyncFd::with_interest(queue, tokio::io::Interest::READABLE)?;
    let _ready = watched.readable().await?;
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub async fn exited(_identity: &ProcessIdentity) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "host process observation is not implemented on this platform"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_process_observation_releases_handles_while_the_process_lives() {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
        fn handles() -> u32 {
            let mut count = 0;
            // SAFETY: pseudo-handle is valid and count is an initialized output parameter.
            assert_ne!(unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) }, 0);
            count
        }
        let identity = crate::current_identity().unwrap();
        let mut baseline = None;
        for index in 0..1025 {
            {
                let wait = exited(&identity);
                tokio::pin!(wait);
                tokio::select! {
                    biased;
                    result = &mut wait => panic!("the test process cannot have exited: {result:?}"),
                    () = tokio::task::yield_now() => {},
                }
            }
            if index == 0 {
                baseline = Some(handles());
            }
        }
        assert_eq!(handles(), baseline.unwrap());
        assert!(crate::is_alive(&identity).unwrap());
    }
}
