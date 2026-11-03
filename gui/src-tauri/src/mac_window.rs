//! The window on macOS, as Mac apps keep theirs: closing it puts it away and
//! the app stays in the Dock, its icon brings the window back, and Quit
//! ends the app. Its place is kept as it is put away, as the tray keeps it
//! on Windows.

use tauri::Manager as _;

pub(crate) fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("mac-window")
        .on_event(|app, event| match event {
            tauri::RunEvent::WindowEvent { label, event, .. } if label == "main" => {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    {
                        use tauri_plugin_window_state::AppHandleExt as _;
                        if let Err(error) = app.save_window_state(crate::WINDOW_KEPT) {
                            crate::startup::warning(app, "keep_window", error.to_string());
                        }
                    }
                    if let Some(window) = app.get_webview_window("main")
                        && let Err(error) = window.hide()
                    {
                        crate::startup::warning(app, "hide_window", error.to_string());
                    }
                }
            }
            tauri::RunEvent::Reopen { .. } => {
                if let Some(window) = app.get_webview_window("main") {
                    for (step, done) in [
                        ("unminimize_window", window.unminimize()),
                        ("show_window", window.show()),
                        ("focus_window", window.set_focus()),
                    ] {
                        if let Err(error) = done {
                            crate::startup::warning(app, step, error.to_string());
                        }
                    }
                }
            }
            _ => {}
        })
        .build()
}
