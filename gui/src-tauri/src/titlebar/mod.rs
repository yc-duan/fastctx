//! Native window mechanics for the page's titlebar. No visual policy lives here.

mod native;

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, TauriPlugin};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Regions {
    drag: Vec<Rect>,
    client: Vec<Rect>,
    maximize: Option<Rect>,
    scale: f64,
}

#[derive(Clone, Deserialize)]
pub(super) struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

impl Regions {
    fn validate(&self) -> Result<(), String> {
        if !self.scale.is_finite() || self.scale <= 0.0 {
            return Err("titlebar scale must be finite and positive".into());
        }
        for rect in self.drag.iter().chain(&self.client).chain(self.maximize.iter()) {
            if ![rect.x, rect.y, rect.width, rect.height, rect.x + rect.width, rect.y + rect.height]
                .into_iter()
                .all(f64::is_finite)
                || rect.width < 0.0
                || rect.height < 0.0
            {
                return Err("titlebar rectangles must have finite coordinates and nonnegative sizes".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct State {
    maximized: bool,
    hovered: bool,
    pressed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Minimize,
    ToggleMaximize,
    Close,
}

async fn on_window<T: Send + 'static>(
    window: tauri::WebviewWindow,
    work: impl FnOnce(tauri::WebviewWindow) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    if window.label() != "main" {
        return Err("the titlebar belongs to the main window".into());
    }
    let (send, receive) = tokio::sync::oneshot::channel();
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            let _ = send.send(work(target));
        })
        .map_err(|e| e.to_string())?;
    receive.await.map_err(|e| e.to_string())?
}

/// Rectangles are viewport CSS pixels; scale is the page's devicePixelRatio.
/// The page registers after mounting its buttons. The native input region
/// follows the rendered layout without changing any page styles.
#[fastctx_gui_command::command]
async fn update(window: tauri::WebviewWindow, regions: Regions) -> Result<State, String> {
    regions.validate()?;
    on_window(window, move |window| native::update(window, regions)).await
}

#[fastctx_gui_command::command]
async fn action(window: tauri::WebviewWindow, action: Action) -> Result<(), String> {
    on_window(window, move |window| native::action(&window, action)).await
}

#[fastctx_gui_command::command]
async fn detach(window: tauri::WebviewWindow) -> Result<(), String> {
    on_window(window, |window| native::detach(&window)).await
}

pub fn plugin() -> TauriPlugin<tauri::Wry> {
    Builder::new("world-titlebar")
        .invoke_handler(tauri::generate_handler![update, action, detach])
        .on_page_load(|webview, payload| {
            if webview.label() == "main" && matches!(payload.event(), tauri::webview::PageLoadEvent::Started) {
                let window = webview.window();
                if let Err(error) = native::clear(&window) {
                    eprintln!("clearing titlebar regions: {error}");
                }
            }
        })
        .build()
}

/// Set the initial frame before the HWND is created, avoiding a flash of the
/// system titlebar while WebView2 loads. Only Windows calls this function.
pub fn configure(mut context: tauri::Context<tauri::Wry>) -> tauri::Context<tauri::Wry> {
    for window in context.config_mut().app.windows.iter_mut().filter(|window| window.label == "main") {
        window.decorations = false;
        window.shadow = true;
    }
    context
}
