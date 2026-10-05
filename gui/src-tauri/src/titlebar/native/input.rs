//! A paint-free input child owned directly by the top-level window, following
//! Windows Terminal's non-client input sink. This must sit above WebView2:
//! its renderer HWND belongs to another process and cannot be subclassed.

use super::*;
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, DeleteObject, HRGN, RGN_DIFF, RGN_OR, SetWindowRgn, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::RevokeDragDrop;
use windows::core::w;

const CLASS: windows::core::PCWSTR = w!("WorldTitlebarInput");

fn create(frame: &Rc<Frame>) -> Result<HWND, String> {
    unsafe {
        let instance = HINSTANCE(GetModuleHandleW(None).map_err(|e| e.to_string())?.0);
        let mut existing = WNDCLASSW::default();
        if GetClassInfoW(Some(instance), CLASS, &mut existing).is_err() {
            let class = WNDCLASSW {
                style: CS_DBLCLKS,
                lpfnWndProc: Some(procedure),
                hInstance: instance,
                lpszClassName: CLASS,
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(windows::core::Error::from_thread().to_string());
            }
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!(""),
            WS_CHILD | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(frame.hwnd),
            None,
            Some(instance),
            Some(std::ptr::from_ref(frame).cast()),
        )
        .map_err(|error| error.to_string())?;
        if let Err(error) = super::drop::register(hwnd, frame.window.clone()) {
            DestroyWindow(hwnd).map_err(|e| e.to_string())?;
            return Err(error.to_string());
        }
        Ok(hwnd)
    }
}

fn combine(
    region: HRGN,
    rect: &super::super::Rect,
    scale: f64,
    mode: windows::Win32::Graphics::Gdi::RGN_COMBINE_MODE,
) -> Result<(), String> {
    unsafe {
        // Ceil both ends so integer pointer coordinates match Rect::contains
        // exactly, including fractional CSS bounds and fractional DPI scales.
        let part = CreateRectRgn(
            (rect.x * scale).ceil() as i32,
            (rect.y * scale).ceil() as i32,
            ((rect.x + rect.width) * scale).ceil() as i32,
            ((rect.y + rect.height) * scale).ceil() as i32,
        );
        if part.is_invalid() {
            return Err("creating a titlebar input rectangle failed".into());
        }
        let result = CombineRgn(Some(region), Some(region), Some(part), mode);
        let _ = DeleteObject(part.into());
        if result.0 == 0 {
            return Err("combining titlebar input rectangles failed".into());
        }
    }
    Ok(())
}

pub(super) fn update(frame: &Rc<Frame>) -> Result<(), String> {
    let regions = frame.regions.borrow();
    let Some(regions) = regions.as_ref() else {
        return Ok(());
    };
    let hwnd = if frame.input.get().is_invalid() { create(frame)? } else { frame.input.get() };
    unsafe {
        let scale = regions.scale * GetDpiForWindow(frame.hwnd) as f64 / frame.region_dpi.get() as f64;
        let region = CreateRectRgn(0, 0, 0, 0);
        if region.is_invalid() {
            return Err("creating the titlebar input region failed".into());
        }
        let result: Result<(), String> = (|| {
            for rect in &regions.drag {
                combine(region, rect, scale, RGN_OR)?;
            }
            for rect in &regions.client {
                combine(region, rect, scale, RGN_DIFF)?;
            }
            if let Some(rect) = &regions.maximize {
                combine(region, rect, scale, RGN_OR)?;
            }
            if SetWindowRgn(hwnd, Some(region), true) == 0 {
                return Err("installing the titlebar input region failed".into());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = DeleteObject(region.into());
        }
        result?;
        let mut bounds = windows::Win32::Foundation::RECT::default();
        GetClientRect(frame.hwnd, &mut bounds).map_err(|e| e.to_string())?;
        SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, bounds.right, bounds.bottom, SWP_NOACTIVATE | SWP_SHOWWINDOW)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        let frame = unsafe { (&*(create.lpCreateParams as *const Rc<Frame>)).clone() };
        // Publish the HWND before creation calls back into window/COM code.
        frame.input.set(hwnd);
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(Box::new(frame)) as isize);
        }
    }
    let data = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if data == 0 {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let frame = unsafe { (&*(data as *const Rc<Frame>)).clone() };
    match message {
        WM_NCHITTEST => {
            let hit = unsafe { SendMessageW(frame.hwnd, message, Some(wparam), Some(lparam)) };
            if hit.0 == HTCLIENT as isize { LRESULT(HTTRANSPARENT as _) } else { hit }
        }
        WM_NCMOUSEMOVE => {
            frame.pointer(wparam.0 == HTMAXBUTTON as usize, frame.pressed.get());
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as _,
                dwFlags: TME_LEAVE | TME_NONCLIENT,
                hwndTrack: hwnd,
                ..Default::default()
            };
            if let Err(error) = unsafe { TrackMouseEvent(&mut track) } {
                frame.error(error);
            }
            LRESULT(0)
        }
        WM_NCMOUSELEAVE => {
            frame.pointer(false, frame.pressed.get());
            LRESULT(0)
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONUP | WM_NCLBUTTONDBLCLK | WM_NCRBUTTONDOWN | WM_NCRBUTTONUP
        | WM_NCRBUTTONDBLCLK => unsafe { SendMessageW(frame.hwnd, message, Some(wparam), Some(lparam)) },
        WM_SETCURSOR => unsafe { SendMessageW(frame.hwnd, message, Some(WPARAM(frame.hwnd.0 as usize)), Some(lparam)) },
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            frame.input.set(HWND::default());
            unsafe {
                let _ = RevokeDragDrop(hwnd);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(data as *mut Rc<Frame>));
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}
