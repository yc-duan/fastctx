use std::cell::{Cell, RefCell};
use std::rc::Rc;

use tauri::Emitter;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmDefWindowProc, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, ScreenToClient,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETAUTOHIDEBAREX, APPBARDATA, DefSubclassProc, GetWindowSubclass,
    RemoveWindowSubclass, SHAppBarMessage, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{Action, Regions, State};

mod drop;
mod input;
mod menu;

// Helper subclass IDs are scoped by callback address, not globally by number.
const SUBCLASS: usize = 1;

struct Frame {
    window: tauri::Window,
    hwnd: HWND,
    regions: RefCell<Option<Regions>>,
    region_dpi: Cell<u32>,
    hovered: Cell<bool>,
    pressed: Cell<bool>,
    input: Cell<HWND>,
}

impl Frame {
    fn state(&self) -> State {
        State {
            maximized: unsafe { IsZoomed(self.hwnd).as_bool() },
            hovered: self.hovered.get(),
            pressed: self.pressed.get(),
        }
    }

    fn publish(&self) {
        if let Err(error) = self.window.emit("world-titlebar-state", self.state()) {
            eprintln!("reporting titlebar state: {error}");
        }
    }

    fn pointer(&self, hovered: bool, pressed: bool) {
        let changed = self.hovered.replace(hovered) != hovered;
        let changed = self.pressed.replace(pressed) != pressed || changed;
        if changed {
            self.publish();
        }
    }

    fn error(&self, error: impl std::fmt::Display) {
        let message = error.to_string();
        eprintln!("World titlebar: {message}");
        if let Err(error) = self.window.emit("world-titlebar-error", message) {
            eprintln!("reporting titlebar error: {error}");
        }
    }

    fn hit(&self, lparam: LPARAM, native: LRESULT) -> LRESULT {
        let regions = self.regions.borrow();
        let Some(regions) = regions.as_ref() else {
            return native;
        };
        // Keep tao's actual resize borders, including the DWM side/bottom frame.
        if (HTLEFT as isize..=HTBOTTOMRIGHT as isize).contains(&native.0) {
            return native;
        }
        let mut point = point(lparam);
        if !unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool() {
            self.error("mapping titlebar hit-test coordinates failed");
            return LRESULT(HTCLIENT as _);
        }
        // Between WM_DPICHANGED and the page's next frame, logical rectangles
        // still describe the same controls. Browser zoom remains in `scale`.
        let scale = regions.scale * unsafe { GetDpiForWindow(self.hwnd) } as f64 / self.region_dpi.get() as f64;
        let (x, y) = (point.x as f64 / scale, point.y as f64 / scale);
        if regions.maximize.as_ref().is_some_and(|rect| rect.contains(x, y)) {
            LRESULT(HTMAXBUTTON as _)
        } else if regions.client.iter().any(|rect| rect.contains(x, y)) {
            LRESULT(HTCLIENT as _)
        } else if regions.drag.iter().any(|rect| rect.contains(x, y)) {
            LRESULT(HTCAPTION as _)
        } else {
            // The invisible system caption buttons must never take input from
            // the page. Only its explicitly registered rectangles are captions.
            LRESULT(HTCLIENT as _)
        }
    }
}

fn point(lparam: LPARAM) -> POINT {
    POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 }
}

fn packed(point: POINT) -> LPARAM {
    LPARAM(((point.y as u16 as u32) << 16 | point.x as u16 as u32) as isize)
}

fn hwnd(window: &tauri::Window) -> Result<HWND, String> {
    window.hwnd().map(|value| HWND(value.0)).map_err(|error| error.to_string())
}

unsafe fn frame(hwnd: HWND) -> Option<Rc<Frame>> {
    let mut data = 0;
    if unsafe { GetWindowSubclass(hwnd, Some(parent_proc), SUBCLASS, Some(&mut data)) }.as_bool() {
        Some(unsafe { (&*(data as *const Rc<Frame>)).clone() })
    } else {
        None
    }
}

pub(super) fn update(window: tauri::WebviewWindow, regions: Regions) -> Result<State, String> {
    let window = window.as_ref().window();
    let hwnd = hwnd(&window)?;
    let frame = if let Some(frame) = unsafe { frame(hwnd) } {
        frame
    } else {
        let frame = Rc::new(Frame {
            window: window.clone(),
            hwnd,
            regions: RefCell::new(None),
            region_dpi: Cell::new(unsafe { GetDpiForWindow(hwnd) }),
            hovered: Cell::new(false),
            pressed: Cell::new(false),
            input: Cell::new(HWND::default()),
        });
        unsafe {
            let data = Box::into_raw(Box::new(frame.clone()));
            if !SetWindowSubclass(hwnd, Some(parent_proc), SUBCLASS, data as usize).as_bool() {
                drop(Box::from_raw(data));
                return Err("installing native titlebar hit testing failed".into());
            }
        }
        frame
    };
    let enabling = frame.regions.borrow().is_none();
    frame.region_dpi.set(unsafe { GetDpiForWindow(hwnd) });
    *frame.regions.borrow_mut() = Some(regions);
    if enabling {
        window.set_decorations(false).map_err(|error| error.to_string())?;
        window.set_shadow(true).map_err(|error| error.to_string())?;
    }
    // Windows 10 does not expose this Windows 11 attribute. E_INVALIDARG is
    // its documented unsupported-attribute result, not a styling failure.
    if enabling {
        unsafe {
            DwmExtendFrameIntoClientArea(hwnd, &windows::Win32::UI::Controls::MARGINS::default())
                .map_err(|error| error.to_string())?;
            if let Err(error) = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                std::ptr::from_ref(&DWMWCP_ROUND).cast(),
                std::mem::size_of_val(&DWMWCP_ROUND) as u32,
            ) && error.code() != windows::Win32::Foundation::E_INVALIDARG
            {
                return Err(error.to_string());
            }
        }
    }
    input::update(&frame)?;
    Ok(frame.state())
}

pub(super) fn clear(window: &tauri::Window) -> Result<(), String> {
    if let Some(frame) = unsafe { frame(hwnd(window)?) } {
        *frame.regions.borrow_mut() = None;
        if !frame.input.get().is_invalid() {
            unsafe {
                let _ = ShowWindow(frame.input.get(), SW_HIDE);
            }
        }
        frame.pointer(false, false);
        if unsafe { GetCapture() } == frame.hwnd {
            unsafe {
                ReleaseCapture().map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

pub(super) fn detach(window: &tauri::WebviewWindow) -> Result<(), String> {
    clear(&window.as_ref().window())
}

pub(super) fn action(window: &tauri::WebviewWindow, action: Action) -> Result<(), String> {
    let hwnd = hwnd(&window.as_ref().window())?;
    let command = match action {
        Action::Minimize => SC_MINIMIZE,
        Action::ToggleMaximize => {
            if unsafe { IsZoomed(hwnd).as_bool() } {
                SC_RESTORE
            } else {
                SC_MAXIMIZE
            }
        }
        Action::Close => SC_CLOSE,
    };
    // Keep tao and desktop_presence in the chain: CloseRequested hides to the
    // tray, and system commands update tao's minimize/maximize state.
    unsafe {
        PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(command as usize), LPARAM(0)).map_err(|error| error.to_string())
    }
}

unsafe extern "system" fn parent_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let frame = unsafe { (&*(data as *const Rc<Frame>)).clone() };
    if message == WM_NCDESTROY {
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(parent_proc), SUBCLASS);
            drop(Box::from_raw(data as *mut Rc<Frame>));
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
    }
    if frame.regions.borrow().is_none() {
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }
    match message {
        WM_NCHITTEST => frame.hit(lparam, unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }),
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
            let mut result = LRESULT(0);
            if unsafe { DwmDefWindowProc(hwnd, message, wparam, lparam, &mut result) }.as_bool() {
                result
            } else {
                unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
            }
        }
        WM_NCMOUSELEAVE => {
            frame.pointer(false, frame.pressed.get());
            let mut result = LRESULT(0);
            unsafe {
                let _ = DwmDefWindowProc(hwnd, message, wparam, lparam, &mut result);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if wparam.0 == HTMAXBUTTON as usize => {
            unsafe {
                SetCapture(hwnd);
            }
            frame.pointer(true, true);
            LRESULT(0)
        }
        WM_NCRBUTTONDOWN if wparam.0 == HTCAPTION as usize => LRESULT(0),
        WM_NCRBUTTONUP if wparam.0 == HTCAPTION as usize => {
            if let Err(error) = menu::show(hwnd, point(lparam)) {
                frame.error(error);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE | WM_LBUTTONUP if frame.pressed.get() => {
            let mut point = point(lparam);
            if !unsafe { ClientToScreen(hwnd, &mut point) }.as_bool() {
                frame.error("mapping captured pointer coordinates failed");
            }
            let hovered = frame.hit(packed(point), LRESULT(HTCLIENT as _)).0 == HTMAXBUTTON as isize;
            frame.pointer(hovered, message != WM_LBUTTONUP);
            if message == WM_LBUTTONUP {
                if let Err(error) = unsafe { ReleaseCapture() } {
                    frame.error(error);
                }
                if hovered {
                    let command = if unsafe { IsZoomed(hwnd).as_bool() } { SC_RESTORE } else { SC_MAXIMIZE };
                    if let Err(error) =
                        unsafe { PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(command as _), LPARAM(0)) }
                    {
                        frame.error(error);
                    }
                }
            }
            LRESULT(0)
        }
        WM_CANCELMODE | WM_CAPTURECHANGED => {
            frame.pointer(false, false);
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
        }
        WM_NCCALCSIZE if wparam.0 != 0 && unsafe { IsZoomed(hwnd).as_bool() } => {
            // tao 0.37 queries ABM_GETAUTOHIDEBAR, which only knows the primary
            // taskbar. Recompute from this monitor's work area, not its already
            // inset result, so each edge is reserved exactly once (2026-10-04).
            let proposed = unsafe { (*(lparam.0 as *const NCCALCSIZE_PARAMS)).rgrc[0] };
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            if unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_CAPTION.0 != 0 {
                let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as _, ..Default::default() };
                if unsafe { GetMonitorInfoW(MonitorFromRect(&proposed, MONITOR_DEFAULTTONEAREST), &mut info) }.as_bool()
                {
                    let mut rect = info.rcWork;
                    for edge in [ABE_LEFT, ABE_TOP, ABE_RIGHT, ABE_BOTTOM] {
                        let mut appbar = APPBARDATA {
                            cbSize: std::mem::size_of::<APPBARDATA>() as _,
                            uEdge: edge,
                            rc: info.rcMonitor,
                            ..Default::default()
                        };
                        if unsafe { SHAppBarMessage(ABM_GETAUTOHIDEBAREX, &mut appbar) } != 0 {
                            match edge {
                                ABE_LEFT => rect.left += 1,
                                ABE_TOP => rect.top += 1,
                                ABE_RIGHT => rect.right -= 1,
                                _ => rect.bottom -= 1,
                            }
                        }
                    }
                    unsafe {
                        (*(lparam.0 as *mut NCCALCSIZE_PARAMS)).rgrc[0] = rect;
                    }
                } else {
                    frame.error("reading maximized window's monitor work area failed");
                }
            }
            result
        }
        WM_SIZE => {
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            frame.publish();
            result
        }
        _ => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}
