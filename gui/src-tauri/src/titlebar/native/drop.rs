//! The caption input sink has its own OLE target. The rest of the webview
//! keeps wry's target. Both deliver the same frontend Tauri drag event contract.

use std::cell::Cell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use serde::Serialize;
use tauri::{Emitter, EventTarget};
use windows::Win32::Foundation::{DV_E_FORMATETC, HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, RegisterDragDrop,
    ReleaseStgMedium,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::core::{Error, Ref, Result, implement};

#[derive(Clone, Serialize)]
struct Payload {
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<PathBuf>>,
    position: tauri::PhysicalPosition<i32>,
}

#[implement(IDropTarget)]
struct Target {
    window: tauri::Window,
    active: Cell<bool>,
}

fn paths(object: Ref<'_, IDataObject>) -> Result<Option<Vec<PathBuf>>> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
        ..Default::default()
    };
    let mut medium = match unsafe { object.ok()?.GetData(&format) } {
        Ok(medium) => medium,
        Err(error) if error.code() == DV_E_FORMATETC => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    unsafe {
        let drop = HDROP(medium.u.hGlobal.0);
        for index in 0..DragQueryFileW(drop, u32::MAX, None) {
            let length = DragQueryFileW(drop, index, None) as usize;
            let mut text = vec![0; length + 1];
            DragQueryFileW(drop, index, Some(&mut text));
            paths.push(PathBuf::from(OsString::from_wide(&text[..length])));
        }
        ReleaseStgMedium(&mut medium);
    }
    Ok(Some(paths))
}

impl Target {
    fn emit(&self, event: &str, paths: Option<Vec<PathBuf>>, point: &POINTL) -> Result<()> {
        let hwnd =
            self.window.hwnd().map_err(|error| Error::new(windows::Win32::Foundation::E_FAIL, error.to_string()))?;
        let mut point = POINT { x: point.x, y: point.y };
        unsafe {
            ScreenToClient(HWND(hwnd.0), &mut point).ok()?;
        }
        let payload = Payload { paths, position: tauri::PhysicalPosition::new(point.x, point.y) };
        self.window
            .emit_to(EventTarget::labeled(self.window.label()), event, payload)
            .map_err(|error| Error::new(windows::Win32::Foundation::E_FAIL, error.to_string()))
    }
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        object: Ref<'_, IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let paths = paths(object)?;
        self.active.set(paths.is_some());
        unsafe {
            *effect = if self.active.get() { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        }
        if paths.is_some() {
            self.emit("tauri://drag-enter", paths, point)?;
        }
        Ok(())
    }
    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, point: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe {
            *effect = if self.active.get() { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        }
        if self.active.get() {
            self.emit("tauri://drag-over", None, point)?;
        }
        Ok(())
    }
    fn DragLeave(&self) -> Result<()> {
        if self.active.replace(false) {
            self.window
                .emit_to(EventTarget::labeled(self.window.label()), "tauri://drag-leave", ())
                .map_err(|error| Error::new(windows::Win32::Foundation::E_FAIL, error.to_string()))?;
        }
        Ok(())
    }
    fn Drop(
        &self,
        object: Ref<'_, IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        unsafe {
            *effect = DROPEFFECT_NONE;
        }
        if self.active.replace(false)
            && let Some(paths) = paths(object)?
        {
            self.emit("tauri://drag-drop", Some(paths), point)?;
            unsafe {
                *effect = DROPEFFECT_COPY;
            }
        }
        Ok(())
    }
}

pub(super) fn register(hwnd: HWND, window: tauri::Window) -> Result<()> {
    let target: IDropTarget = Target { window, active: Cell::new(false) }.into();
    unsafe { RegisterDragDrop(hwnd, &target) }
}
