use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSSavePanel};
use objc2_foundation::NSString;
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;

pub async fn choose(app: tauri::AppHandle, name: String) -> Result<Option<PathBuf>, String> {
    let (answer, answered) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = answer.send(select(&name));
    })
    .map_err(|error| error.to_string())?;
    answered.await.map_err(|error| error.to_string())?
}

fn select(name: &str) -> Result<Option<PathBuf>, String> {
    let main = MainThreadMarker::new().ok_or("the save dialog requires the main thread")?;
    let panel = NSSavePanel::savePanel(main);
    panel.setNameFieldStringValue(&NSString::from_str(name));
    panel.setCanCreateDirectories(true);
    match panel.runModal() {
        response if response == NSModalResponseCancel => Ok(None),
        response if response == NSModalResponseOK => {
            let url = panel.URL().ok_or("the save dialog returned no destination")?;
            if !url.isFileURL() {
                return Err("the save destination is not a local file".into());
            }
            let bytes = unsafe { std::ffi::CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) }.to_bytes();
            Ok(Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes))))
        }
        response => Err(format!("the save dialog ended with response {response}")),
    }
}
