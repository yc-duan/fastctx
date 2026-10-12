//! The system's own folder picker, for a folder on this computer: the agents
//! page chooses a folder for logins with it. A phone has no folder of the
//! World's own to pick, and every machine but this one is walked through
//! its node instead.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// The folder the user picked; none when they cancelled.
#[cfg(windows)]
pub async fn pick(_app: tauri::AppHandle, window: tauri::WebviewWindow) -> Result<Option<PathBuf>, String> {
    windows::pick(window).await
}

#[cfg(target_os = "linux")]
pub async fn pick(_app: tauri::AppHandle, _window: tauri::WebviewWindow) -> Result<Option<PathBuf>, String> {
    linux::pick().await
}

#[cfg(target_os = "macos")]
pub async fn pick(app: tauri::AppHandle, _window: tauri::WebviewWindow) -> Result<Option<PathBuf>, String> {
    macos::pick(app).await
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub async fn pick(_app: tauri::AppHandle, _window: tauri::WebviewWindow) -> Result<Option<PathBuf>, String> {
    Err("this device has no folder picker of its own".into())
}
