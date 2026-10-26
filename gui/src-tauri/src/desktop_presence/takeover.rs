//! A stalled pipe owner may be retired only when its exact executable and
//! process identity are known and it has no visible window in this session.

use fastctx_types::ProcessIdentity;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible};
use windows::core::{BOOL, PWSTR};

pub fn retire(owner: &ProcessIdentity) -> Result<(), String> {
    if !fastctx_platform::is_alive(owner).map_err(|e| e.to_string())? {
        return Ok(());
    }
    unsafe {
        let mut ours = 0;
        let mut theirs = 0;
        ProcessIdToSessionId(std::process::id(), &mut ours).map_err(|e| e.to_string())?;
        ProcessIdToSessionId(owner.pid, &mut theirs).map_err(|e| e.to_string())?;
        if ours != theirs {
            return Err(format!("unresponsive Fastctx {} is in another Windows session", owner.pid));
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, owner.pid).map_err(|e| e.to_string())?;
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        let queried = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length);
        let _ = CloseHandle(handle);
        queried.map_err(|e| format!("identifying pipe owner {}: {e}", owner.pid))?;
        let path = std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
        let ours = std::env::current_exe().map_err(|e| e.to_string())?;
        if std::fs::canonicalize(&path).map_err(|e| e.to_string())?
            != std::fs::canonicalize(&ours).map_err(|e| e.to_string())?
        {
            return Err(format!(
                "unresponsive pipe owner {} runs {}; expected {}",
                owner.pid,
                path.display(),
                ours.display()
            ));
        }
        let mut windows = Visible { pid: owner.pid, found: false };
        EnumWindows(Some(visible), LPARAM((&mut windows as *mut Visible) as isize)).map_err(|e| e.to_string())?;
        if windows.found {
            return Err(format!("unresponsive Fastctx {} has a visible window; it was left running", owner.pid));
        }
    }
    // The termination API checks the birth identity again while opening its
    // handle; a reused PID can never be terminated by this request.
    fastctx_platform::terminate::terminate(owner)
        .map_err(|e| format!("ending unresponsive windowless Fastctx {}: {e}", owner.pid))?;
    let began = std::time::Instant::now();
    while fastctx_platform::is_alive(owner).map_err(|e| e.to_string())? {
        if began.elapsed() >= std::time::Duration::from_secs(5) {
            return Err(format!("Fastctx {} did not exit after termination", owner.pid));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Ok(())
}

struct Visible {
    pid: u32,
    found: bool,
}
unsafe extern "system" fn visible(window: HWND, data: LPARAM) -> BOOL {
    unsafe {
        let state = &mut *(data.0 as *mut Visible);
        let mut pid = 0;
        GetWindowThreadProcessId(window, Some(&mut pid));
        if pid == state.pid && IsWindowVisible(window).as_bool() {
            state.found = true;
        }
    }
    BOOL(1)
}
