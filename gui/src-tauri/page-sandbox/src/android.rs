use serde::Serialize;
use tauri::ipc::Channel;
use tauri::plugin::{PluginApi, PluginHandle};
use tauri::{Manager, Wry};

use crate::{Bounds, Delivery, document};

struct Android(PluginHandle<Wry>);

pub fn setup(app: &tauri::AppHandle, api: PluginApi<Wry, ()>) -> Result<(), Box<dyn std::error::Error>> {
    app.manage(Android(api.register_android_plugin("dev.fastctx.pages", "PageSandboxPlugin")?));
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Mount {
    id: String,
    html: String,
    wrapper: String,
    policy: &'static str,
    wrapper_policy: &'static str,
    navigation: &'static str,
    bounds: Bounds,
    messages: Channel<Delivery>,
}

fn call(window: tauri::WebviewWindow, command: &str, args: impl Serialize) -> Result<(), String> {
    window.state::<Android>().0.run_mobile_plugin::<()>(command, args).map_err(|e| e.to_string())
}

pub fn mount(window: tauri::WebviewWindow, id: String, html: String, bounds: Bounds, messages: Channel<Delivery>) -> Result<(), String> {
    call(window, "mount", Mount { id, html, bounds, messages, wrapper: document::wrapper(true), policy: document::POLICY, wrapper_policy: document::WRAPPER_POLICY, navigation: document::NAVIGATION })
}

pub fn place(window: tauri::WebviewWindow, id: String, bounds: Bounds) -> Result<(), String> {
    call(window, "place", serde_json::json!({ "id": id, "bounds": bounds }))
}

pub fn post(window: tauri::WebviewWindow, id: String, message: serde_json::Value) -> Result<(), String> {
    call(window, "post", serde_json::json!({ "id": id, "message": message.to_string() }))
}

pub fn close(window: tauri::WebviewWindow, id: String) -> Result<(), String> {
    call(window, "close", serde_json::json!({ "id": id }))
}
