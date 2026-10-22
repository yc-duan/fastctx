//! Windows presence is a self-contained plugin. Notification policy and
//! action semantics belong to the page; this module owns OS lifetimes.

mod identity;
mod instance;
mod notifications;
mod takeover;

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
    /// A quiet button's: the window stays where it is.
    #[serde(skip)]
    quiet: bool,
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
    notifications: Option<Notifications>,
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
    crate::startup::warning(app, "desktop_presence", error.clone());
    if let Some(state) = app.try_state::<Presence>() {
        state.push(app, Event::Error { error });
    }
}

#[tauri::command]
fn notify(app: tauri::AppHandle, state: tauri::State<'_, Presence>, notice: Notice) -> Result<String, String> {
    state.notifications.as_ref().ok_or("notification storage is unavailable")?.show(&app, notice)
}

#[tauri::command]
fn dismiss(state: tauri::State<'_, Presence>, notification_id: String) -> Result<(), String> {
    state.notifications.as_ref().ok_or("notification storage is unavailable")?.remove(&notification_id)
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
    let window = app.get_webview_window("main").ok_or("Fastctx's main window does not exist")?;
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
    let window = app.get_webview_window("main").ok_or("Fastctx's main window does not exist")?;
    // Keep tao's visibility cache in sync. Calling ShowWindow directly would
    // make a formerly hidden window visible while tao still considers it
    // hidden, causing the next window.hide() to do nothing.
    window.show().map_err(|e| e.to_string())?;
    window.unminimize().map_err(|e| e.to_string())?;
    let hwnd = hwnd(app)?;
    unsafe {
        if !SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() != hwnd {
            return Err("Windows refused to bring Fastctx to the foreground".into());
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
                Launch::Quit => {
                    handle.exit(0);
                    return Ok(());
                }
                Launch::Notification(uri) => {
                    state.notifications.as_ref().ok_or("notification storage is unavailable")?.activate(&uri)?
                }
                Launch::Open(route) => {
                    if let Some(route) = &route {
                        notifications::route(route)?;
                    }
                    Activation { route, action: None, notification_id: None, quiet: false }
                }
            };
            let quiet = activation.quiet;
            state.push(&handle, Event::Activation(activation));
            if quiet { Ok(()) } else { reveal(&handle) }
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
            if let Err(error) = result {
                crate::installer_lines::complain(&error);
                std::process::exit(1);
            }
            Ok(None)
        }
        // The installer's last step. What went wrong is said and the
        // installation goes on; World offers what is missing when it opens.
        [command] if command == "--after-install" => {
            if let Err(error) = crate::setup::after_install() {
                crate::installer_lines::complain(&format!("preparing this computer for Fastctx: {error}"));
                std::process::exit(1);
            }
            Ok(None)
        }
        // The package uninstaller's steps. Their output is the uninstaller's
        // details; a failure ends with its reason and a nonzero status, exit
        // status 10 says a removal of everything kept the data because this
        // computer is a world's server, and 11 that nothing was removed from
        // a managed machine its owner keeps.
        [command, rest @ ..] if command == "--uninstall-world" => {
            let everything = match rest {
                [] => false,
                [flag] if flag == "--everything" => true,
                _ => return Err("expected --uninstall-world [--everything]".into()),
            };
            match crate::package_removal::at_package_uninstall(everything) {
                Ok(crate::package_removal::Removed::AsAsked) => Ok(None),
                Ok(crate::package_removal::Removed::KeptForHub) => std::process::exit(10),
                Ok(crate::package_removal::Removed::Managed) => std::process::exit(11),
                Err(error) => {
                    crate::installer_lines::complain(&error);
                    std::process::exit(1);
                }
            }
        }
        [command] if command == "--quit-running" => {
            if let Err(error) = instance::quit_running() {
                crate::installer_lines::complain(&error);
                std::process::exit(1);
            }
            Ok(None)
        }
        [command] if command == "--uninstall-finish" => {
            if let Err(error) = crate::package_removal::finish_checkpoint() {
                crate::installer_lines::complain(&error);
                std::process::exit(1);
            }
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
    let open = MenuItem::with_id(app, "desktop-presence-open", "打开 Fastctx", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "desktop-presence-quit", "退出 Fastctx", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let icon = app
        .default_window_icon()
        .ok_or_else(|| tauri::Error::Io(std::io::Error::other("bundled Fastctx icon is missing")))?;
    TrayIconBuilder::with_id("desktop-presence")
        .icon(icon.clone())
        .tooltip("Fastctx")
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

pub fn plugin(mut instance: Option<Instance>, initial_error: Option<String>) -> TauriPlugin<tauri::Wry> {
    Builder::new("desktop-presence")
        .invoke_handler(tauri::generate_handler![notify, dismiss, pending, acknowledge, window_presence])
        .setup(move |app, _| {
            let notifications = match app.path().app_local_data_dir() {
                Ok(directory) => {
                    let directory = if app.config().app.app_directories_override.is_none() {
                        directory.with_file_name(identity::APP_ID)
                    } else {
                        directory
                    };
                    Some(Notifications::new(directory.join("notifications")))
                }
                Err(error) => {
                    crate::startup::warning(app, "notification_directory", error.to_string());
                    None
                }
            };
            app.manage(Presence { notifications, pending: Mutex::new(VecDeque::new()), next: AtomicU64::new(1) });
            if let Err(error) = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(identity::APP_ID)) } {
                report(app, format!("registering Fastctx window identity: {error}"));
            }
            if let Some(error) = initial_error {
                report(app, error);
            }
            Ok(())
        })
        .on_event(move |app, event| match event {
            tauri::RunEvent::Ready => {
                if let Err(error) = tray(app) {
                    report(app, format!("creating Fastctx's tray: {error}"));
                }
                let Some(instance) = instance.take() else { return };
                let initial = instance.launch.clone();
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    if !matches!(initial, Launch::Open(None))
                        && let Err(error) = launch(&handle, initial).await
                    {
                        report(&handle, error);
                    }
                    if let Err(error) = instance.serve(handle.clone()).await {
                        report(&handle, format!("Fastctx instance listener stopped: {error}"));
                    }
                });
            }
            tauri::RunEvent::WindowEvent { label, event, .. } if label == "main" => {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event
                    && app.tray_by_id("desktop-presence").is_some()
                {
                    api.prevent_close();
                    // The app lives on in the tray, and may never see its exit
                    // (a shutdown ends it), so the window's place is kept now.
                    {
                        use tauri_plugin_window_state::AppHandleExt;
                        if let Err(error) = app.save_window_state(crate::WINDOW_KEPT) {
                            report(app, format!("keeping Fastctx's window place: {error}"));
                        }
                    }
                    if let Some(window) = app.get_webview_window("main")
                        && let Err(error) = window.hide()
                    {
                        report(app, format!("hiding Fastctx's window: {error}"));
                    }
                }
                if let Err(error) = app.emit_to("main", "desktop-presence-changed", ()) {
                    eprintln!("reporting Fastctx's window state: {error}");
                }
            }
            _ => (),
        })
        .build()
}
