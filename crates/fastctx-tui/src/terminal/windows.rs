//! The native Console path; a PTY pipe is handled by its terminal description.

mod winpty;

use std::io::{self, IsTerminal};
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;

use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::System::Console::{
    ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleWindow, SetConsoleMode,
};
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;

use crate::design::Depth;

pub(super) fn has_visible_window() -> bool {
    // SAFETY: neither call takes ownership. ConPTY's message-only window
    // is not visible; a real Console window identifies the local renderer.
    unsafe { IsWindowVisible(GetConsoleWindow()) != 0 }
}

pub(super) fn console_depth() -> io::Result<Option<Depth>> {
    let stdout = io::stdout();
    if !stdout.is_terminal() {
        return Ok(None);
    }
    let handle = stdout.as_raw_handle();
    let mut mode = 0;
    // SAFETY: stdout owns the handle and mode is writable.
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return Ok(None);
    }
    if winpty::attached()? {
        return Ok(Some(Depth::Ansi16));
    }
    let vt = mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
    // SAFETY: the handle was verified as a console output buffer above.
    if unsafe { SetConsoleMode(handle, vt) } == 0 {
        return Ok(Some(Depth::Ansi16));
    }
    // SAFETY: restore exactly the mode that was read from this handle.
    if unsafe { SetConsoleMode(handle, mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut version = OSVERSIONINFOW { dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32, ..Default::default() };
    // SAFETY: version is initialized, writable, and carries its actual size.
    let status = unsafe { RtlGetVersion(&mut version) };
    if status < 0 {
        return Err(io::Error::other(format!("RtlGetVersion failed: {status:#x}")));
    }
    // RGB arrived in Windows 10 Insider build 14931 (released in 1703).
    // Earlier VT consoles quantize extended colours to their 16-colour table.
    Ok(Some(if (version.dwMajorVersion, version.dwBuildNumber) >= (10, 14931) {
        Depth::TrueColor
    } else {
        Depth::Ansi16
    }))
}
