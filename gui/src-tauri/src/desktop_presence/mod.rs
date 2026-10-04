//! Windows presence is a self-contained plugin. Notification policy and
//! action semantics belong to the page; this module owns OS lifetimes.

mod identity;
mod instance;
mod notifications;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsIconic, IsWindowVisible, SetForegroundWindow};
use windows::core::HSTRING;

use instance::{Instance, Launch};
use notifications::{Notice, Notifications};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Activation {
    route: Option<String>,
    action: Option<String>,
    notification_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Event {
    Activation(Activation),
    NotificationFailed { notification_id: String, error: String },
    Error { error: String },
}

#[derive(Clone, Serialize)]
struct Delivery {
    id: u64,
    event: Event,
}

#[derive(Clone, Serialize)]
struct WindowPresence {
    visible: bool,
    focused: bool,
    minimized: bool,
}

struct Presence {
    notifications: Notifications,
    pending: Mutex<VecDeque<Delivery>>,
    next: AtomicU64,
}

impl Presence {
    fn push(&self, app: &tauri::AppHandle, event: Event) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().expect("desktop deliveries").push_back(Delivery { id, event });
        // The event is only a wakeup. Deliveries stay until the page explicitly
        // acknowledges them, including across page loads and listener setup.
        if let Err(error) = app.emit_to("main", "desktop-presence-pending", ()) {
            eprintln!("waking the desktop page: {error}");
        }
    }
}

fn report(app: &tauri::AppHandle, error: String) {
    eprintln!("{error}");
    app.state::<Presence>().push(app, Event::Error { error });
}

#[tauri::command]
fn notify(app: tauri::AppHandle, state: tauri::State<'_, Presence>, notice: Notice) -> Result<String, String> {
    state.notifications.show(&app, notice)
}

#[tauri::command]
fn dismiss(state: tauri::State<'_, Presence>, notification_id: String) -> Result<(), String> {
    state.notifications.remove(&notification_id)
}

#[tauri::command]
fn pending(state: tauri::State<'_, Presence>) -> Vec<Delivery> {
    state.pending.lock().expect("desktop deliveries").iter().cloned().collect()
}

#[tauri::command]
fn acknowledge(state: tauri::State<'_, Presence>, id: u64) -> Result<(), String> {
    let mut pending = state.pending.lock().expect("desktop deliveries");
    if pending.front().is_some_and(|item| item.id == id) {
        pending.pop_front();
        Ok(())
    } else {
        Err("desktop deliveries must be acknowledged in order".into())
    }
}

fn hwnd(app: &tauri::AppHandle) -> Result<HWND, String> {
    let window = app.get_webview_window("main").ok_or("World's main window does not exist")?;
    window.hwnd().map(|value| HWND(value.0)).map_err(|e| e.to_string())
}

#[tauri::command]
fn window_presence(app: tauri::AppHandle) -> Result<WindowPresence, String> {
    let hwnd = hwnd(&app)?;
    unsafe {
        let minimized = IsIconic(hwnd).as_bool();
        Ok(WindowPresence {
            visible: IsWindowVisible(hwnd).as_bool() && !minimized,
            focused: GetForegroundWindow() == hwnd,
            minimized,
        })
    }
}

fn reveal(app: &tauri::AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("World's main window does not exist")?;
    // Keep tao's visibility cache in sync. Calling ShowWindow directly would
    // make a formerly hidden window visible while tao still considers it
    // hidden, causing the next window.hide() to do nothing.
    window.show().map_err(|e| e.to_string())?;
    window.unminimize().map_err(|e| e.to_string())?;
    let hwnd = hwnd(app)?;
    unsafe {
        if !SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() != hwnd {
            return Err("Windows refused to bring World to the foreground".into());
        }
    }
    if let Err(error) = app.emit_to("main", "desktop-presence-changed", ()) {
        return Err(error.to_string());
    }
    Ok(())
}

async fn launch(app: &tauri::AppHandle, launch: Launch) -> Result<(), String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            let state = handle.state::<Presence>();
            let activation = match launch {
                Launch::Notification(uri) => state.notifications.activate(&uri)?,
                Launch::Open(route) => {
                    if let Some(route) = &route {
                        notifications::route(route)?;
                    }
                    Activation { route, action: None, notification_id: None }
                }
            };
            state.push(&handle, Event::Activation(activation));
            reveal(&handle)
        })();
        let _ = send.send(result);
    })
    .map_err(|e| e.to_string())?;
    receive.await.map_err(|e| e.to_string())?
}

/// Runs before constructing the World client. Installer commands and
/// secondary launches never start a WebView or touch the World.
pub fn enter() -> Result<Option<Instance>, String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "--desktop-install" || command == "--desktop-uninstall" => {
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(|e| e.to_string())?;
            }
            let result = if command == "--desktop-install" { identity::install() } else { identity::uninstall() };
            unsafe {
                CoUninitialize();
            }
            result?;
            Ok(None)
        }
        [] => instance::enter(Launch::Open(None)),
        [route] if route.starts_with("#/") => {
            notifications::route(route)?;
            instance::enter(Launch::Open(Some(route.clone())))
        }
        [command, value] if command == "--desktop-activate" => instance::enter(Launch::Notification(value.clone())),
        _ => Err("expected no arguments, a #/ route, or --desktop-activate <notification URI>".into()),
    }
}

fn tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "desktop-presence-open", "打开 World", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "desktop-presence-quit", "退出 World", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    TrayIconBuilder::with_id("desktop-presence")
        .icon(app.default_window_icon().expect("bundled World icon").clone())
        .tooltip("World")
        .menu(&menu)
        .show_menu_on_left_click(false)
        // Tauri menu listeners are global. Register exactly once, here;
        // registering the same handler on the app also runs each action twice.
        .on_menu_event(|app, event| match event.id().as_ref() {
            "desktop-presence-open" => {
                if let Err(error) = reveal(app) {
                    report(app, error);
                }
            }
            "desktop-presence-quit" => app.exit(0),
            _ => (),
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
            ) && let Err(error) = reveal(tray.app_handle())
            {
                report(tray.app_handle(), error);
            }
        })
        .build(app)?;
    Ok(())
}

pub fn plugin(instance: Instance) -> TauriPlugin<tauri::Wry> {
    let mut instance = Some(instance);
    Builder::new("desktop-presence")
        .invoke_handler(tauri::generate_handler![notify, dismiss, pending, acknowledge, window_presence])
        .setup(|app, _| {
            unsafe {
                SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(identity::APP_ID))?;
            }
            app.manage(Presence {
                notifications: Notifications::new(
                    app.path().local_data_dir()?.join(identity::APP_ID).join("notifications"),
                ),
                pending: Mutex::new(VecDeque::new()),
                next: AtomicU64::new(1),
            });
            Ok(())
        })
        .on_event(move |app, event| match event {
            tauri::RunEvent::Ready => {
                if let Err(error) = tray(app) {
                    report(app, format!("creating World's tray: {error}"));
                    app.exit(1);
                    return;
                }
                let instance = instance.take().expect("one application Ready event");
                let initial = instance.launch.clone();
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    if !matches!(initial, Launch::Open(None))
                        && let Err(error) = launch(&handle, initial).await
                    {
                        report(&handle, error);
                    }
                    if let Err(error) = instance.serve(handle.clone()).await {
                        report(&handle, format!("World instance listener stopped: {error}"));
                        handle.exit(1);
                    }
                });
            }
            tauri::RunEvent::WindowEvent { label, event, .. } if label == "main" => {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Some(window) = app.get_webview_window("main")
                        && let Err(error) = window.hide()
                    {
                        report(app, format!("hiding World's window: {error}"));
                    }
                }
                if let Err(error) = app.emit_to("main", "desktop-presence-changed", ()) {
                    eprintln!("reporting World's window state: {error}");
                }
            }
            _ => (),
        })
        .build()
}
