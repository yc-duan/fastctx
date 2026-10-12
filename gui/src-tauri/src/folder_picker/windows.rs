use std::os::windows::ffi::OsStringExt as _;
use std::path::PathBuf;
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::core::HRESULT;

pub async fn pick(window: tauri::WebviewWindow) -> Result<Option<PathBuf>, String> {
    let owner = window.hwnd().map_err(|error| error.to_string())?.0 as usize;
    let (answer, answered) = tokio::sync::oneshot::channel();
    // COM's apartment belongs to this dedicated thread, not a shared
    // runtime worker that another component might have initialized.
    std::thread::Builder::new()
        .name("world-folder-dialog".into())
        .spawn(move || {
            let result = unsafe { select(owner) }.map_err(|error| error.to_string());
            let _ = answer.send(result);
        })
        .map_err(|error| error.to_string())?;
    answered.await.map_err(|error| error.to_string())?
}

unsafe fn select(owner: usize) -> windows::core::Result<Option<PathBuf>> {
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    let _apartment = Apartment;
    let dialog: IFileOpenDialog = unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        // A folder of the file system only: a library or a phone's storage
        // shown in Explorer has no path for a node to search.
        dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
        match dialog.Show(Some(HWND(owner as *mut _))) {
            Err(error) if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => return Ok(None),
            result => result?,
        }
        let item = dialog.GetResult()?;
        let path = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let copied = PathBuf::from(std::ffi::OsString::from_wide(path.as_wide()));
        CoTaskMemFree(Some(path.0.cast()));
        Ok(Some(copied))
    }
}
