//! Exit status when the operating system exposes it for an observed process.

use fastctx_types::ProcessIdentity;
use std::io;

/// Waits for one process lifetime. Windows exposes its exit code through
/// the retained handle; Unix does not expose a non-child's exit status.
pub async fn exit_status(identity: &ProcessIdentity) -> io::Result<Option<u32>> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, HANDLE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        struct Process(HANDLE);
        // SAFETY: the owned query handle can be used and closed on any thread.
        unsafe impl Send for Process {}
        unsafe impl Sync for Process {}
        impl Drop for Process {
            fn drop(&mut self) {
                // SAFETY: this handle was opened here and has one owner.
                unsafe { CloseHandle(self.0) };
            }
        }
        // SAFETY: opens only a query handle, never sends a signal.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, identity.pid) };
        if handle.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) { Ok(None) } else { Err(error) };
        }
        let process = Process(handle);
        if crate::identity::of_handle(process.0, identity.pid)? != *identity {
            return Ok(None);
        }
        super::exited(identity).await?;
        let mut code = 0;
        // SAFETY: the retained handle names the same lifetime, even after exit.
        if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(code))
    }
    #[cfg(not(windows))]
    {
        super::exited(identity).await?;
        Ok(None)
    }
}
