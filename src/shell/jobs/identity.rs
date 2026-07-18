//! Detached-supervisor isolation checks plus shared process-identity access.

pub(crate) use crate::process_identity::{identity_is_alive, process_identity};

/// Returns a warning when a Windows supervisor could not break away from an outer Job Object.
pub(crate) fn supervisor_isolation_warning() -> Option<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let mut in_job = 0;
        let checked =
            unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) };
        if checked == 0 {
            return Some(format!(
                "Cannot verify whether the supervisor is inside an outer Windows Job Object: {}.",
                std::io::Error::last_os_error()
            ));
        }
        if in_job != 0 {
            return Some(
                "The supervisor is inside an outer Windows Job Object; survival across host shutdown may be impaired."
                    .to_string(),
            );
        }
    }
    None
}
