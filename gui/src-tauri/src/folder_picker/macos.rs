use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSOpenPanel};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;

pub async fn pick(app: tauri::AppHandle) -> Result<Option<PathBuf>, String> {
    let (answer, answered) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = answer.send(select());
    })
    .map_err(|error| error.to_string())?;
    answered.await.map_err(|error| error.to_string())?
}

fn select() -> Result<Option<PathBuf>, String> {
    let main = MainThreadMarker::new().ok_or("the folder dialog requires the main thread")?;
    let panel = NSOpenPanel::openPanel(main);
    panel.setCanChooseFiles(false);
    panel.setCanChooseDirectories(true);
    panel.setAllowsMultipleSelection(false);
    panel.setCanCreateDirectories(true);
    match panel.runModal() {
        response if response == NSModalResponseCancel => Ok(None),
        response if response == NSModalResponseOK => {
            let url = panel.URL().ok_or("the folder dialog returned no folder")?;
            if !url.isFileURL() {
                return Err("the chosen folder is not a local folder".into());
            }
            let bytes = unsafe { std::ffi::CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) }.to_bytes();
            Ok(Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes))))
        }
        response => Err(format!("the folder dialog ended with response {response}")),
    }
}
