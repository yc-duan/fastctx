//! The windows a card of a conversation's side is taken out into
//! (`side/board.ts`): opened beside the World's own window at the page's
//! word, where the card was let go, with the system's own frame; brought
//! forward; shut by its own page, and all of them as the main page loads
//! anew. Their pages and the main window's speak over a channel of the
//! page's own (`BroadcastChannel`): the app's webviews share one origin.

use sha2::{Digest as _, Sha256};
use tauri::{AppHandle, Manager as _, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// What every card window's label begins with.
pub(crate) const PREFIX: &str = "board-";

/// The label of the window of the card `key` names.
fn label(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let short: String = digest[..8].iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{PREFIX}{short}")
}

/// Opens the window of the card `key` names at `path` (the app's page, its
/// query and its address), `left`, `top`, `width` and `height` in the
/// screen's logical pixels; where it stands already, it comes forward.
#[fastctx_gui_command::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn board_open(
    app: AppHandle,
    key: String,
    path: String,
    title: String,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let label = label(&key);
    if let Some(window) = app.get_webview_window(&label) {
        return forward(&window);
    }
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(path.into()))
        .title(title)
        .inner_size(width, height)
        .min_inner_size(440.0, 420.0)
        .position(left, top)
        .build()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Brings the window of the card `key` names forward, where it stands.
#[fastctx_gui_command::command]
pub(crate) async fn board_front(app: AppHandle, key: String) -> Result<(), String> {
    match app.get_webview_window(&label(&key)) {
        Some(window) => forward(&window),
        None => Ok(()),
    }
}

/// Brings a card window forward, out of the taskbar first where it was
/// minimized: a minimized window takes no focus.
fn forward(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.is_minimized().unwrap_or(false) {
        window.unminimize().map_err(|error| error.to_string())?;
    }
    window.set_focus().map_err(|error| error.to_string())
}

/// Shuts the card window asking: a card's page shuts its own window, and no
/// other.
#[fastctx_gui_command::command]
pub(crate) async fn board_close(window: WebviewWindow) -> Result<(), String> {
    if !window.label().starts_with(PREFIX) {
        return Err("only a card's own window is shut this way".into());
    }
    window.close().map_err(|error| error.to_string())
}

/// Shuts every card window: the main page that served them loads anew.
pub(crate) fn shut_all(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if label.starts_with(PREFIX) {
            let _ = window.close();
        }
    }
}
