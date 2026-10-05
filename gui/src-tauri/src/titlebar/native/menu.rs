//! Use the window's real system menu at a custom caption's screen position.

use windows::Win32::Foundation::{ERROR_SUCCESS, GetLastError, HWND, LPARAM, POINT, SetLastError, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

pub(super) fn show(hwnd: HWND, point: POINT) -> Result<(), String> {
    unsafe {
        let menu = GetSystemMenu(hwnd, false);
        if menu.is_invalid() {
            return Err("the window has no system menu".into());
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let maximized = IsZoomed(hwnd).as_bool();
        let minimized = IsIconic(hwnd).as_bool();
        // TrackPopupMenuEx does not perform DefWindowProc's system-command
        // preparation. Preserve the menu (including Close's existing policy)
        // and update only the standard sizing commands from the actual HWND.
        for (command, enabled) in [
            (SC_RESTORE, maximized || minimized),
            (SC_MOVE, !maximized && !minimized),
            (SC_SIZE, !maximized && !minimized && style & WS_SIZEBOX.0 != 0),
            (SC_MINIMIZE, !minimized && style & WS_MINIMIZEBOX.0 != 0),
            (SC_MAXIMIZE, !maximized && style & WS_MAXIMIZEBOX.0 != 0),
        ] {
            if EnableMenuItem(menu, command, MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED }).0 == -1 {
                return Err(format!("updating system menu command {command:#x} failed"));
            }
        }
        SetLastError(ERROR_SUCCESS);
        let command =
            TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, point.x, point.y, hwnd, None).0 as u32;
        if command != 0 {
            PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(command as usize), LPARAM(0))
                .map_err(|error| error.to_string())?;
        } else if GetLastError() != ERROR_SUCCESS {
            return Err(windows::core::Error::from_thread().to_string());
        }
    }
    Ok(())
}
