//! Android's process owns one device and runtime, even without an Activity.
//! The web page supplies notification rules; the service executes them.

mod bridge;
pub(crate) use bridge::{joining, joining_ended};
mod monitor;
mod observer;
pub(crate) mod readiness;
pub(crate) mod uploading;
pub(crate) mod watching;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use fastctx_client::{World, device::Device};
use serde_json::Value;
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{Emitter, Manager};

static WORLD: OnceLock<World> = OnceLock::new();
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
static ACTIVITY_READY: AtomicBool = AtomicBool::new(false);
static VISIBLE: std::sync::LazyLock<tokio::sync::watch::Sender<bool>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(false).0);

fn start(directory: PathBuf, bridge: Arc<bridge::Bridge>) -> Result<(), String> {
    fastctx_platform::ensure_private_dir(&directory).map_err(|error| error.to_string())?;
    fastctx_net::android::install(bridge.clone())?;
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    let world = {
        let _entered = runtime.enter();
        let cache = crate::connection::kept(&directory)?;
        World::from_device(
            Device::open_managed(directory.join("device"), cache.clone()).map_err(|error| error.to_string())?,
        )
        .keeping(cache)
    };
    WORLD.set(world.clone()).map_err(|_| "Fastctx's Android runtime was already started")?;
    tauri::async_runtime::set(runtime.handle().clone());
    runtime.spawn(monitor::run(world.clone(), bridge.clone()));
    runtime.spawn(observer::run(world, directory, bridge));
    RUNTIME.set(runtime).map_err(|_| "Fastctx's Android runtime was already started")?;
    Ok(())
}

pub fn world() -> Result<World, String> {
    WORLD.get().cloned().ok_or_else(|| "Android has not started the device connection".into())
}

fn reconnect() {
    if let Some(world) = WORLD.get()
        && let Some(device) = world.device()
    {
        readiness::reconnecting(device);
        fastctx_net::android::network_changed();
        world.retry_now();
    }
}

fn visible(visible: bool) {
    if visible {
        reconnect();
    }
    VISIBLE.send_replace(visible);
}

pub(super) fn visibility() -> tokio::sync::watch::Receiver<bool> {
    VISIBLE.subscribe()
}

fn activity(ready: bool) {
    ACTIVITY_READY.store(ready, Ordering::Release);
    if ready {
        rebuild_window();
    }
}

fn rebuild_window() {
    let Some(app) = APP.get() else { return };
    let handle = app.clone();
    let schedule = handle.clone();
    // On the event-loop thread run_on_main_thread executes immediately.
    // Leave the plugin callback's lock before constructing another WebView.
    tauri::async_runtime::spawn(async move {
        if let Err(error) = schedule.run_on_main_thread(move || {
            if ACTIVITY_READY.load(Ordering::Acquire)
                && handle.get_webview_window("main").is_none()
                && let Err(error) =
                    tauri::WebviewWindowBuilder::new(&handle, "main", tauri::WebviewUrl::App("index.html".into()))
                        .build()
            {
                bridge::report(format!("rebuilding the Android window: {error}"));
            }
        }) {
            bridge::report(format!("scheduling the Android window: {error}"));
        }
    });
}

fn pending() {
    if let Some(app) = APP.get()
        && let Err(error) = app.emit_to("main", "android-world-pending", ())
    {
        bridge::report(format!("waking the Android page: {error}"));
    }
}

struct Native(PluginHandle<tauri::Wry>);

pub(crate) async fn save_as(
    app: &tauri::AppHandle,
    path: String,
    name: String,
) -> Result<crate::save_as::Outcome, String> {
    app.state::<Native>()
        .0
        .run_mobile_plugin_async("saveAs", serde_json::json!({"path": path, "suggestedName": name}))
        .await
        .map_err(|error| error.to_string())
}

/// Hands the package at `path` to the system's installer; `false` when the
/// system's page for allowing it to install apps opened instead.
pub(crate) async fn install_package(app: &tauri::AppHandle, path: String) -> Result<bool, String> {
    let answer: Value = app
        .state::<Native>()
        .0
        .run_mobile_plugin_async("installPackage", serde_json::json!({"path": path}))
        .await
        .map_err(|error| error.to_string())?;
    Ok(answer.get("started").and_then(Value::as_bool).unwrap_or(false))
}

pub(crate) async fn open_file(app: &tauri::AppHandle, path: String) -> Result<(), String> {
    app.state::<Native>()
        .0
        .run_mobile_plugin_async::<serde_json::Value>("openFile", serde_json::json!({"path": path}))
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

macro_rules! command {
    ($name:ident, $native:literal) => {
        #[fastctx_gui_command::command]
        async fn $name(state: tauri::State<'_, Native>) -> Result<Value, String> {
            state.0.run_mobile_plugin_async($native, ()).await.map_err(|error| error.to_string())
        }
    };
    ($name:ident, $native:literal, $argument:ident) => {
        #[fastctx_gui_command::command]
        async fn $name(state: tauri::State<'_, Native>, $argument: Value) -> Result<Value, String> {
            state.0.run_mobile_plugin_async($native, serde_json::json!({stringify!($argument): $argument}))
                .await.map_err(|error| error.to_string())
        }
    };
}
#[fastctx_gui_command::command]
async fn status(state: tauri::State<'_, Native>) -> Result<Value, String> {
    let mut status: Value = state.0.run_mobile_plugin_async("status", ()).await.map_err(|error| error.to_string())?;
    status["connection"] =
        serde_json::to_value(world()?.device().expect("Android device").state()).map_err(|error| error.to_string())?;
    Ok(status)
}
command!(scan, "scan");
command!(request_notifications, "requestNotifications");
command!(pending_activations, "pending");
command!(configure, "configure", configuration);
command!(acknowledge, "acknowledge", id);
command!(notify, "notify", notice);
command!(dismiss, "dismiss", id);
command!(settings, "settings", destination);

/// The conversation the page shows, `resident@machine`; empty on another
/// page (`observer::in_view`).
static VIEWING: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

#[fastctx_gui_command::command]
async fn viewing(conversation: String) -> Result<(), String> {
    *VIEWING.lock().expect("never poisoned") = conversation;
    Ok(())
}

pub(super) fn viewing_now() -> String {
    VIEWING.lock().expect("never poisoned").clone()
}

pub fn plugin() -> TauriPlugin<tauri::Wry> {
    Builder::new("android-world")
        .invoke_handler(tauri::generate_handler![
            status,
            scan,
            request_notifications,
            pending_activations,
            configure,
            acknowledge,
            notify,
            dismiss,
            settings,
            viewing
        ])
        .setup(|app, api| {
            let plugin = api.register_android_plugin("dev.fastctx.world", "WorldPlugin")?;
            app.manage(Native(plugin));
            APP.set(app.clone()).map_err(|_| "Fastctx's Android shell was already started")?;
            Ok(())
        })
        .on_event(|app, event| match event {
            tauri::RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
            tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } if label == "main" => {
                app.state::<crate::Shell>().close_page();
                rebuild_window();
            }
            _ => (),
        })
        .build()
}
