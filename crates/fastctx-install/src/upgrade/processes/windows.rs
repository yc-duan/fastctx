use std::ffi::OsString;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::RemoteDesktop::*;
use windows_sys::Win32::System::Threading::*;

use super::Process;

pub(super) fn enumerate() -> io::Result<(Vec<Process>, Vec<String>)> {
    // SAFETY: WTS supplies owner SIDs independently of image-query access.
    // All allocated OS objects are released after copying the needed fields.
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut length = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut length);
        let mut user = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        let success = GetTokenInformation(token, TokenUser, user.as_mut_ptr().cast(), length, &mut length);
        let error = io::Error::last_os_error();
        CloseHandle(token);
        if success == 0 {
            return Err(error);
        }
        let sid = (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        let mut entries: *mut WTS_PROCESS_INFOW = ptr::null_mut();
        let mut count = 0;
        if WTSEnumerateProcessesW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut entries, &mut count) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut processes = Vec::new();
        let mut unavailable = Vec::new();
        for entry in std::slice::from_raw_parts(entries, count as usize) {
            if entry.ProcessId == std::process::id() || entry.pUserSid.is_null() || EqualSid(sid, entry.pUserSid) == 0 {
                continue;
            }
            match image(entry.ProcessId) {
                Ok(Some((path, handle))) => processes.push(Process {
                    pid: entry.ProcessId,
                    image_source: path.clone(),
                    image: path,
                    legacy: false,
                    handle: Some(handle),
                }),
                Ok(None) => {}
                Err(error) => unavailable.push((entry.ProcessId, error)),
            }
        }
        WTSFreeMemory(entries.cast());
        if !unavailable.is_empty() {
            // A process can disappear between WTS enumeration and OpenProcess,
            // which Windows can report as ACCESS_DENIED. Reconcile only these
            // unresolved PIDs with a second OS observation, never guess that a
            // still-listed process is dead from its access error.
            let mut entries = ptr::null_mut();
            let mut count = 0;
            if WTSEnumerateProcessesW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut entries, &mut count) == 0 {
                return Err(io::Error::last_os_error());
            }
            let present: std::collections::HashSet<u32> =
                std::slice::from_raw_parts(entries, count as usize).iter().map(|entry| entry.ProcessId).collect();
            WTSFreeMemory(entries.cast());
            unavailable.retain(|(pid, _)| present.contains(pid));
        }
        Ok((processes, unavailable.into_iter().map(|(pid, error)| format!("PID {pid}: {error}")).collect()))
    }
}

fn image(pid: u32) -> io::Result<Option<(PathBuf, OwnedHandle)>> {
    // SAFETY: each successfully opened handle is closed before returning.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) { Ok(None) } else { Err(error) };
        }
        let owned = OwnedHandle::from_raw_handle(process);
        if WaitForSingleObject(process, 0) == WAIT_OBJECT_0 {
            Ok(None)
        } else {
            let mut buffer = vec![0u16; MAX_PATH as usize];
            loop {
                let mut length = buffer.len() as u32;
                if QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) != 0 {
                    break Ok(Some((PathBuf::from(OsString::from_wide(&buffer[..length as usize])), owned)));
                }
                let error = io::Error::last_os_error();
                if WaitForSingleObject(process, 0) == WAIT_OBJECT_0 {
                    break Ok(None);
                }
                if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
                    break Err(error);
                }
                buffer.resize(buffer.len() * 2, 0);
            }
        }
    }
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: u32,
        information: *mut core::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

fn handle(process: &Process) -> io::Result<HANDLE> {
    if let Some(handle) = &process.handle {
        Ok(handle.as_raw_handle())
    } else if process.pid == std::process::id() {
        // SAFETY: the pseudo handle belongs to this process and is never closed.
        Ok(unsafe { GetCurrentProcess() })
    } else {
        Err(io::Error::other("process observation has no lifetime handle"))
    }
}

pub(super) fn exited(process: &Process) -> io::Result<bool> {
    // SAFETY: the inventory retains this handle until every observation ends.
    match unsafe { WaitForSingleObject(handle(process)?, 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

pub(super) fn confirmed_legacy(process: &Process) -> io::Result<bool> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    };
    let mut file = std::fs::OpenOptions::new()
        .access_mode(FILE_GENERIC_READ | FILE_GENERIC_EXECUTE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&process.image_source)?;
    // A pathname can be replaced while its old image is still running. Class 44
    // compares this file handle with the process's actual image section (phnt's
    // ProcessImageFileMapping), before hashing this same handle.
    // SAFETY: the process handle is owned and the input buffer is one HANDLE.
    unsafe {
        let query = OpenProcess(PROCESS_QUERY_INFORMATION, 0, process.pid);
        if query.is_null() {
            return Err(io::Error::last_os_error());
        }
        let query = OwnedHandle::from_raw_handle(query);
        let mut image = file.as_raw_handle();
        let status = NtQueryInformationProcess(
            query.as_raw_handle(),
            44,
            (&raw mut image).cast(),
            size_of::<HANDLE>() as u32,
            ptr::null_mut(),
        );
        if exited(process)? {
            return Ok(false);
        }
        if status < 0 {
            return Err(io::Error::other(format!(
                "cannot bind executable file to running image: {}",
                io::Error::from_raw_os_error(RtlNtStatusToDosError(status) as i32)
            )));
        }
    }
    Ok(super::known_binary(&super::super::archive::hash_handle(&mut file)?))
}

#[cfg(test)]
#[test]
fn image_binding_distinguishes_a_copy_from_the_running_file() {
    let image = std::env::current_exe().unwrap();
    let mut process =
        Process { pid: std::process::id(), image: image.clone(), image_source: image, legacy: false, handle: None };
    assert!(!confirmed_legacy(&process).unwrap());
    let home = tempfile::tempdir().unwrap();
    let copy = home.path().join("other.exe");
    std::fs::copy(&process.image_source, &copy).unwrap();
    process.image_source = copy;
    assert!(confirmed_legacy(&process).unwrap_err().to_string().contains("cannot bind executable file"));
}

pub(super) fn references(observation: &Process, path: &Path) -> io::Result<bool> {
    // SAFETY: ProcessCommandLineInformation returns a UNICODE_STRING within
    // the supplied allocation. Bounds are checked before reading its buffer.
    // Contents are used only for path matching and are never logged.
    unsafe {
        // Reopening a PID after enumeration races both exit and PID reuse.
        // The retained handle names exactly the lifetime whose image we read.
        let process = handle(observation)?;
        let result = (|| {
            if WaitForSingleObject(process, 0) == WAIT_OBJECT_0 {
                return Ok(false);
            }
            let mut length = 0;
            let status = NtQueryInformationProcess(process, 60, ptr::null_mut(), 0, &mut length);
            if length == 0 {
                return Err(io::Error::from_raw_os_error(RtlNtStatusToDosError(status) as i32));
            }
            let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
            let status = NtQueryInformationProcess(process, 60, buffer.as_mut_ptr().cast(), length, &mut length);
            if status < 0 {
                return Err(io::Error::from_raw_os_error(RtlNtStatusToDosError(status) as i32));
            }
            let text = &*buffer.as_ptr().cast::<UnicodeString>();
            let start = text.buffer as usize;
            let end = start
                .checked_add(text.length as usize)
                .ok_or_else(|| io::Error::other("invalid command line length"))?;
            let base = buffer.as_ptr() as usize;
            if start < base || end > base + buffer.len() * size_of::<usize>() || text.length % 2 != 0 {
                return Err(io::Error::other("process command line buffer is invalid"));
            }
            let command = String::from_utf16_lossy(std::slice::from_raw_parts(text.buffer, text.length as usize / 2));
            Ok(super::command_references(&command, path))
        })();
        let ended = WaitForSingleObject(process, 0) == WAIT_OBJECT_0;
        if ended { Ok(false) } else { result }
    }
}
