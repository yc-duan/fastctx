//! Detached launch with no inherited handles, including the caller's response pipes.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS,
    PROCESS_INFORMATION, STARTUPINFOW,
};

pub(super) fn launch(executable: &Path, request: &Path) -> io::Result<()> {
    let application: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut arguments = Vec::new();
    quoted(&mut arguments, executable)?;
    arguments.extend(" run-host ".encode_utf16());
    quoted(&mut arguments, request)?;
    arguments.push(0);
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // Rust's Command inherits every inheritable handle on Windows, even with Stdio::null().
    // A parent's original response pipe can therefore keep EOF open until the job ends.
    // This launch inherits no handles at all; the supervisor opens its diagnostics itself.
    // SAFETY: application/arguments are terminated UTF-16; all buffers outlive CreateProcessW.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            arguments.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful CreateProcessW transfers both handles to this function. Closing
    // these observation handles does not terminate either the process or its initial thread.
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(())
}

fn quoted(output: &mut Vec<u16>, path: &Path) -> io::Result<()> {
    output.push(b'"' as u16);
    for unit in path.as_os_str().encode_wide() {
        if unit == 0 || unit == b'"' as u16 {
            return Err(io::Error::other("invalid Windows launch path"));
        }
        output.push(unit);
    }
    // Both operands are file paths ending in .exe or .request, so no trailing slash
    // can escape the closing quote in Windows command-line decoding.
    output.push(b'"' as u16);
    Ok(())
}
