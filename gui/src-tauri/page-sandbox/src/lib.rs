//! Page surfaces are native views outside Tauri's webview registry. Only
//! the trusted interface calls this plugin; page messages carry no commands.

#[cfg(windows)]
mod windows;
#[cfg(target_os = "android")]
mod android;
#[cfg(any(windows, target_os = "android"))]
mod document;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::Manager;
use tauri::plugin::{Builder, TauriPlugin};

#[derive(Clone, Deserialize, Serialize)]
pub struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
    visible: bool,
}

impl Bounds {
    fn validate(&self) -> Result<(), String> {
        if [self.x, self.y, self.width, self.height, self.scale].into_iter().all(f64::is_finite)
            && self.width >= 0.0 && self.height >= 0.0 && self.scale > 0.0
        {
            Ok(())
        } else {
            Err("invalid page surface bounds".into())
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Delivery {
    Message(serde_json::Value),
    Failed(String),
}

/// Fonts come from the application's bundled assets. The interface's CSP
/// does not permit fetch, so reading these bytes is a native operation.
#[tauri::command]
fn font(window: tauri::WebviewWindow, path: String) -> Result<tauri::ipc::Response, String> {
    if window.label() != "main" || !path.starts_with("/fonts/") || !path.ends_with(".woff2")
        || path.contains('\\') || path.split('/').any(|part| part == "." || part == "..")
    { return Err("not a bundled World font".into()); }
    let asset = window.app_handle().asset_resolver().get(path).ok_or("bundled font is missing")?;
    if !asset.bytes.starts_with(b"wOF2") { return Err("bundled font is not WOFF2".into()); }
    Ok(tauri::ipc::Response::new(asset.bytes))
}

async fn on_main<T: Send + 'static>(
    window: tauri::WebviewWindow,
    work: impl FnOnce(tauri::WebviewWindow) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    if window.label() != "main" {
        return Err("only the main interface owns page surfaces".into());
    }
    #[cfg(target_os = "android")]
    return tauri::async_runtime::spawn_blocking(move || work(window)).await.map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "android"))]
    {
    let (send, receive) = tokio::sync::oneshot::channel();
    let target = window.clone();
    window.run_on_main_thread(move || { let _ = send.send(work(target)); }).map_err(|e| e.to_string())?;
    receive.await.map_err(|e| e.to_string())?
    }
}

#[tauri::command]
async fn mount(window: tauri::WebviewWindow, id: String, html: String, bounds: Bounds, messages: Channel<Delivery>) -> Result<(), String> {
    bounds.validate()?;
    on_main(window, move |window| {
        #[cfg(windows)]
        return windows::mount(window, id, html, bounds, messages);
        #[cfg(target_os = "android")]
        return android::mount(window, id, html, bounds, messages);
        #[cfg(not(any(windows, target_os = "android")))]
        { let _ = (window, id, html, bounds, messages); Err("native pages are not implemented on this platform".into()) }
    }).await
}

#[tauri::command]
async fn place(window: tauri::WebviewWindow, id: String, bounds: Bounds) -> Result<(), String> {
    bounds.validate()?;
    on_main(window, move |window| {
        #[cfg(windows)]
        { let _ = window; windows::place(&id, &bounds) }
        #[cfg(target_os = "android")]
        { android::place(window, id, bounds) }
        #[cfg(not(any(windows, target_os = "android")))]
        { let _ = (window, id, bounds); Err("native pages are not implemented on this platform".into()) }
    }).await
}

#[tauri::command]
async fn post(window: tauri::WebviewWindow, id: String, message: serde_json::Value) -> Result<(), String> {
    on_main(window, move |window| {
        #[cfg(windows)]
        { let _ = window; windows::post(&id, &message) }
        #[cfg(target_os = "android")]
        { android::post(window, id, message) }
        #[cfg(not(any(windows, target_os = "android")))]
        { let _ = (window, id, message); Err("native pages are not implemented on this platform".into()) }
    }).await
}

#[tauri::command]
async fn close(window: tauri::WebviewWindow, id: String) -> Result<(), String> {
    on_main(window, move |window| {
        #[cfg(windows)]
        { let _ = window; windows::close(&id) }
        #[cfg(target_os = "android")]
        { android::close(window, id) }
        #[cfg(not(any(windows, target_os = "android")))]
        { let _ = (window, id); Ok(()) }
    }).await
}

pub fn init() -> TauriPlugin<tauri::Wry> {
    Builder::new("world-pages")
        .invoke_handler(tauri::generate_handler![mount, place, post, close, font])
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            android::setup(_app, _api)?;
            Ok(())
        })
        .on_event(|_app, event| {
            #[cfg(windows)]
            if matches!(event, tauri::RunEvent::Exit) { windows::close_all(); }
            #[cfg(not(windows))]
            let _ = event;
        })
        .build()
}
