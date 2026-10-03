//! A headless node follows an exact task-host process object, never a recycled PID.

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
pub(super) type ServiceParent = OwnedHandle;
#[cfg(not(windows))]
pub(super) type ServiceParent = ();

#[cfg(windows)]
struct WatchGuard(Arc<AtomicBool>);
#[cfg(windows)]
impl Drop for WatchGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Opens and verifies the process object before any wait; absence includes PID reuse.
#[cfg(windows)]
pub(super) fn process_handle(
    identity: &crate::process_identity::ProcessIdentity,
) -> Result<Option<OwnedHandle>, String> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: OpenProcess returns a new handle with the requested observation-only rights.
    let raw = unsafe {
        OpenProcess(
            SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            identity.pid,
        )
    };
    if raw.is_null() {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(87) {
            Ok(None)
        } else {
            Err(format!("Cannot observe process {}: {error}", identity.pid))
        };
    }
    // SAFETY: OpenProcess transferred a non-null owned handle to this scope.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the handle grants query rights and every output slot is writable.
    if unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(format!(
            "Cannot verify process {}: {}",
            identity.pid,
            std::io::Error::last_os_error()
        ));
    }
    let token =
        ((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)).to_string();
    Ok((token == identity.started).then_some(process))
}

pub(super) fn capture(enabled: bool) -> Result<Option<ServiceParent>, String> {
    if !enabled {
        return Ok(None);
    }
    #[cfg(windows)]
    {
        let identity = crate::process_identity::direct_parent_identity()?
            .flatten()
            .ok_or_else(|| "The node service host exited before startup.".to_string())?;
        process_handle(&identity)?
            .map(Some)
            .ok_or_else(|| "The node service host exited during startup.".into())
    }
    #[cfg(not(windows))]
    {
        Err("A headless task host is supported only on Windows.".into())
    }
}

pub(super) async fn wait(parent: Option<ServiceParent>) -> Result<(), String> {
    let Some(parent) = parent else {
        return std::future::pending().await;
    };
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        let cancelled = Arc::new(AtomicBool::new(false));
        let _guard = WatchGuard(Arc::clone(&cancelled));
        tokio::task::spawn_blocking(move || {
            while !cancelled.load(Ordering::Acquire) {
                // SAFETY: the owned, verified handle grants synchronization rights.
                match unsafe { WaitForSingleObject(parent.as_raw_handle(), 500) } {
                    WAIT_OBJECT_0 => return Ok(()),
                    WAIT_TIMEOUT => {}
                    _ => {
                        return Err(format!(
                            "Cannot monitor the node service host: {}",
                            std::io::Error::last_os_error()
                        ));
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|error| format!("The node service monitor failed: {error}"))?
    }
    #[cfg(not(windows))]
    {
        let () = parent;
        Err("A headless task host is supported only on Windows.".into())
    }
}
