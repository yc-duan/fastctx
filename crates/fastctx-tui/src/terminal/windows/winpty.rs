//! winpty transports a Console's CHAR_INFO cells, which only retain the
//! 16-colour attributes. Its inner conhost can still acknowledge RGB.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, MAX_PATH};
use windows_sys::Win32::System::Console::GetConsoleProcessList;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

pub(super) fn attached() -> io::Result<bool> {
    // The API requires at least one slot and reports the required count.
    let mut processes = vec![0u32; 1];
    loop {
        // SAFETY: processes is writable for the supplied count.
        let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) };
        if count == 0 {
            return Err(io::Error::last_os_error());
        }
        if count as usize > processes.len() {
            processes.resize(count as usize, 0);
        } else {
            processes.truncate(count as usize);
            break;
        }
    }
    let mut failure = None;
    for pid in processes {
        match image_name(pid) {
            Ok(path)
                if Path::new(&path).file_name().is_some_and(|name| name.eq_ignore_ascii_case("winpty-agent.exe")) =>
            {
                return Ok(true);
            }
            Ok(_) => {}
            Err(error) => failure = Some(error),
        }
    }
    // An unqueryable attached process is not evidence that no bridge exists.
    failure.map_or(Ok(false), Err)
}

fn image_name(pid: u32) -> io::Result<OsString> {
    // SAFETY: queries only an executable path; the handle is owned below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcess returned a new owned handle.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut path = vec![0u16; MAX_PATH as usize];
    loop {
        let mut size = u32::try_from(path.len()).map_err(io::Error::other)?;
        // SAFETY: path is writable for size UTF-16 units; the handle is live.
        if unsafe { QueryFullProcessImageNameW(handle.as_raw_handle(), 0, path.as_mut_ptr(), &mut size) } != 0 {
            return Ok(OsString::from_wide(&path[..size as usize]));
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(error);
        }
        // MAX_PATH is the initial allocation, not a limit on executable paths.
        let size = path.len().checked_mul(2).ok_or_else(|| io::Error::other("executable path allocation overflow"))?;
        path.resize(size, 0);
    }
}
