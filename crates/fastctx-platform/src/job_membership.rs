//! Read-only observations of the calling process's Windows job.

use std::io;

use fastctx_types::node_lifetime::WindowsJob;

/// The immediate job's limits, or `None` when outside all jobs (always on
/// Unix). This does not enumerate ancestors or alter any host restriction.
pub fn current_job() -> io::Result<Option<WindowsJob>> {
    #[cfg(windows)]
    {
        use std::mem::{size_of, zeroed};
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        };

        if !crate::in_job()? {
            return Ok(None);
        }
        // SAFETY: correctly sized writable structure; NULL queries the
        // calling process's immediate job, without acquiring a job handle.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        if unsafe {
            QueryInformationJobObject(
                std::ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let flags = info.BasicLimitInformation.LimitFlags;
        Ok(Some(WindowsJob {
            kill_on_close: flags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0,
            breakaway_allowed: flags & JOB_OBJECT_LIMIT_BREAKAWAY_OK != 0,
            silent_breakaway: flags & JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK != 0,
        }))
    }
    #[cfg(unix)]
    Ok(None)
}
