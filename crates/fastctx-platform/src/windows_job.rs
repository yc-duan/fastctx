//! Creating a program inside a job object: how the World starts a program
//! on Windows when it answers for everything that program starts.
//!
//! The program is created in the job, so no instant passes in which it
//! could start anything outside it, and the job ends whatever is left in it
//! when its last handle closes, so a holder that dies takes the job's
//! processes with it. The program inherits the three standard handles it is
//! given and no other: the handle list names them explicitly.

use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    GENERIC_READ, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECTINFOCLASS,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::detach::check_environment;
use crate::windows_process::{command_line, environment_block, wide};

/// The standard handles a program is created with.
pub struct Stdio {
    pub input: HANDLE,
    pub output: HANDLE,
    pub error: HANDLE,
}

/// A new job that ends every process in it when its last handle closes.
pub fn new_job() -> io::Result<OwnedHandle> {
    // SAFETY: an unnamed job with default security, owned from here.
    let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a new handle.
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: zeroed is a valid empty limit structure.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    set(&job, JobObjectExtendedLimitInformation, &limits)?;
    Ok(job)
}

/// Creates `program` with `args` in `cwd`, with exactly the environment
/// `env`, inside `job`, with the standard handles `stdio`; the program
/// gets a console of its own with no window, which console programs it
/// starts share, so none of them opens a window on the desktop. Returns the
/// program's process handle and PID.
pub fn create_in_job(
    job: &OwnedHandle,
    program: &Path,
    args: &[&OsStr],
    cwd: &Path,
    env: &[(OsString, OsString)],
    stdio: &Stdio,
) -> io::Result<(OwnedHandle, u32)> {
    if !program.is_absolute() || !cwd.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "program and working directory must be absolute paths",
        ));
    }
    check_environment(env)?;
    let mut handles: Vec<HANDLE> = Vec::new();
    for handle in [stdio.input, stdio.output, stdio.error] {
        if !handles.contains(&handle) {
            set_inheritable(handle)?;
            handles.push(handle);
        }
    }
    let jobs = [job.as_raw_handle()];
    let mut attributes = Attributes::new(2)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST as usize, &jobs)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, &handles)?;

    let application = wide(program);
    let mut line = command_line(program.as_os_str(), args)?;
    let directory = wide(cwd);
    let block = environment_block(env);
    // SAFETY: zeroed is a valid empty STARTUPINFOEXW.
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdio.input;
    startup.StartupInfo.hStdOutput = stdio.output;
    startup.StartupInfo.hStdError = stdio.error;
    startup.lpAttributeList = attributes.as_ptr();
    // SAFETY: zeroed is valid for an out-parameter.
    let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
    // SAFETY: every string, array and the attribute list outlive the call.
    // The job is assigned before any code of the program runs.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            block.as_ptr().cast(),
            directory.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: handles returned by CreateProcessW, owned from here.
    let process = unsafe {
        drop(OwnedHandle::from_raw_handle(info.hThread));
        OwnedHandle::from_raw_handle(info.hProcess)
    };
    Ok((process, info.dwProcessId))
}

pub fn set<T>(job: &OwnedHandle, class: JOBOBJECTINFOCLASS, value: &T) -> io::Result<()> {
    // SAFETY: each caller pairs the class with its documented type.
    let done = unsafe {
        SetInformationJobObject(job.as_raw_handle(), class, (value as *const T).cast(), size_of::<T>() as u32)
    };
    if done == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn query<T>(job: &OwnedHandle, class: JOBOBJECTINFOCLASS) -> io::Result<T> {
    // SAFETY: each caller pairs the class with its documented type, for which
    // zeroed memory is a valid starting value.
    unsafe {
        let mut value: T = zeroed();
        if QueryInformationJobObject(
            job.as_raw_handle(),
            class,
            (&raw mut value).cast(),
            size_of::<T>() as u32,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(value)
    }
}

/// Waits for a process to exit.
pub fn wait(process: &OwnedHandle) -> io::Result<()> {
    // SAFETY: a valid process handle.
    match unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) } {
        WAIT_OBJECT_0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

/// The null device, opened for reading, as a program's standard input.
pub fn open_null_for_reading() -> io::Result<OwnedHandle> {
    let name: Vec<u16> = "NUL".encode_utf16().chain([0]).collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: a NUL-terminated name; the handle is owned from here.
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &attributes,
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a new handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn set_inheritable(handle: HANDLE) -> io::Result<()> {
    // SAFETY: a valid handle this process owns.
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A process-creation attribute list.
struct Attributes(Vec<usize>);

impl Attributes {
    fn new(count: u32) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: the first call only reports the size needed.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &mut bytes) };
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        // SAFETY: storage of the reported size, aligned for the list.
        if unsafe { InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), count, 0, &mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(storage))
    }

    /// Sets `attribute` to `handles`, which must outlive the process
    /// creation that uses this list.
    fn set(&mut self, attribute: usize, handles: &[HANDLE]) -> io::Result<()> {
        // SAFETY: an initialized list and an array the caller keeps alive.
        let done = unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                attribute,
                handles.as_ptr().cast(),
                size_of_val(handles),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if done == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn as_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: an initialized list whose storage is still alive.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) }
    }
}
