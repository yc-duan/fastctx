//! Starting outside a caller's job with the interactive desktop shell as
//! the parent. Windows inherits that parent's token and job membership:
//! https://devblogs.microsoft.com/oldnewthing/20190425-00/?p=102443

use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::{size_of, zeroed};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcessId,
    InitializeProcThreadAttributeList, OpenProcess, OpenProcessToken, PROC_THREAD_ATTRIBUTE_PARENT_PROCESS,
    PROCESS_CREATE_PROCESS, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOEXW,
    UpdateProcThreadAttribute,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

use crate::DetachedProcess;
use crate::windows_process::{command_line, environment_block, wide};

/// Starts with the caller's explicit environment and the desktop shell's
/// process ownership. Returns `None` only when this desktop has no shell.
/// A shell from a different user or session is rejected, as is any launch
/// failure; no second process is started on failure. No handles are inherited.
pub fn spawn(
    program: &Path,
    args: &[&OsStr],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> io::Result<Option<DetachedProcess>> {
    if !program.is_absolute() || !cwd.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "program and working directory must be absolute paths",
        ));
    }
    crate::detach::check_environment(env)?;
    // SAFETY: queries the shell on the caller's desktop without changing it.
    let window = unsafe { GetShellWindow() };
    if window.is_null() {
        return Ok(None);
    }
    let mut pid = 0;
    // SAFETY: a shell window and a writable process-ID buffer.
    if unsafe { GetWindowThreadProcessId(window, &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // Keep this exact shell process alive as an object across PID reuse.
    let shell =
        Handle::new(unsafe { OpenProcess(PROCESS_CREATE_PROCESS | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    let mut token = ptr::null_mut();
    // SAFETY: a live process handle and writable token handle.
    if unsafe { OpenProcessToken(shell.0, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle::new(token)?;
    if crate::windows_user::token_user_sid(token.0)? != crate::windows_user::current_user_sid()?
        || session(pid)? != session(unsafe { GetCurrentProcessId() })?
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the desktop shell belongs to another user or session",
        ));
    }

    let mut attributes = Attributes::new(&shell.0)?;
    // SAFETY: zeroed startup info with its size set is a valid default.
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes.list();
    let application = wide(program);
    let mut line = command_line(program.as_os_str(), args)?;
    let directory = wide(cwd);
    let block = environment_block(env);
    // SAFETY: all strings and the single parent attribute outlive creation;
    // FALSE prevents inheriting handles from either parent. The explicit
    // parent supplies the job, so no node is first born in the caller's job.
    // Its console has no window, and the console programs it starts share it
    // (`crate::detach`).
    let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            block.as_ptr().cast(),
            directory.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    crate::detach::imp::observe(info).map(Some)
}

fn session(pid: u32) -> io::Result<u32> {
    let mut session = 0;
    // SAFETY: queries an existing process into a writable buffer.
    if unsafe { ProcessIdToSessionId(pid, &mut session) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(session)
}

struct Handle(HANDLE);

impl Handle {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() { Err(io::Error::last_os_error()) } else { Ok(Self(handle)) }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: an owned handle, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

struct Attributes(Vec<usize>);

impl Attributes {
    fn new(parent: &HANDLE) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: size query for exactly one attribute.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes) };
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        // SAFETY: aligned storage sized by the preceding query.
        if unsafe { InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = Self(storage);
        // SAFETY: the parent's handle value outlives this list and creation.
        if unsafe {
            UpdateProcThreadAttribute(
                storage.list(),
                0,
                PROC_THREAD_ATTRIBUTE_PARENT_PROCESS as usize,
                (parent as *const HANDLE).cast(),
                size_of::<HANDLE>(),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(storage)
    }

    fn list(&mut self) -> windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: initialized attribute list, deleted before storage is freed.
        unsafe { DeleteProcThreadAttributeList(self.list()) };
    }
}
