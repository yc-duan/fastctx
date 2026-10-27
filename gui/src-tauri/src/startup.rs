//! Window startup does not depend on opening the World. The page can inspect
//! failure and retry while no connection or node exists.

use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use tauri::{Emitter as _, Manager as _};

#[derive(Clone, Serialize)]
pub struct Failure {
    pub step: String,
    pub error: String,
}

#[derive(Clone, Serialize)]
pub struct Status {
    pub phase: &'static str,
    pub failure: Option<Failure>,
    pub warnings: Vec<Failure>,
}

pub struct Startup(Mutex<Status>);

impl Default for Startup {
    fn default() -> Self {
        Self(Mutex::new(Status { phase: "starting", failure: None, warnings: Vec::new() }))
    }
}

fn changed(app: &tauri::AppHandle) {
    let _ = app.emit("startup-changed", ());
}

pub fn warning(app: &tauri::AppHandle, step: &str, error: String) {
    persist(step, &error);
    if let Some(state) = app.try_state::<Startup>() {
        state.0.lock().unwrap_or_else(|e| e.into_inner()).warnings.push(Failure { step: step.into(), error });
        changed(app);
    }
}

pub fn persist(step: &str, error: &str) {
    eprintln!("Fastctx {step}: {error}");
    #[cfg(desktop)]
    if let Ok(root) = fastctx_platform::fastctx_root() {
        let path = root.join("updates/startup-error.json");
        if let Err(error) = fastctx_platform::ensure_private_dir(&root.join("updates")) {
            eprintln!("preparing startup error directory {}: {error}", root.join("updates").display());
            return;
        }
        if let Err(recording) =
            fastctx_install::renewal::write_json(&path, &Failure { step: step.into(), error: error.into() })
        {
            eprintln!("recording {}: {recording}", path.display());
        }
    }
}

#[tauri::command]
pub fn startup_state(state: tauri::State<'_, Startup>) -> Status {
    state.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Starts one fresh attempt; repeated clicks while opening share that attempt.
#[tauri::command]
pub fn startup_retry(app: tauri::AppHandle, state: tauri::State<'_, Startup>) -> Status {
    let mut status = state.0.lock().unwrap_or_else(|e| e.into_inner());
    if status.phase == "failed" && app.try_state::<crate::Shell>().is_none() {
        status.phase = "starting";
        status.failure = None;
        drop(status);
        open(app.clone());
        changed(&app);
        return startup_state(state);
    }
    status.clone()
}

pub fn open(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let directory = app.path().app_data_dir().map_err(|e| format!("locating application data: {e}"));
        let result = match directory {
            Ok(directory) => {
                tauri::async_runtime::spawn_blocking(move || crate::connection::Connection::open(directory))
                    .await
                    .map_err(|e| format!("opening connection worker: {e}"))
                    .and_then(|r| r)
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(connection) => {
                app.manage(crate::Shell {
                    connection,
                    watches: Default::default(),
                    transcripts: Default::default(),
                    next: AtomicU64::new(0),
                    page: AtomicU64::new(0),
                    line_requests: Default::default(),
                    held_reads: Default::default(),
                });
                {
                    let state = app.state::<Startup>();
                    let mut status = state.0.lock().unwrap_or_else(|e| e.into_inner());
                    status.phase = "ready";
                    status.failure = None;
                }
                changed(&app);
                #[cfg(desktop)]
                crate::updates::after_update(&app);
                let carried = app.path().resource_dir().ok().map(|dir| dir.join("linux"));
                if let Err(error) = crate::updates::introduce(&app.state::<crate::Shell>(), carried).await {
                    warning(&app, "introduce_application", error);
                }
            }
            Err(error) => {
                persist("open_connection", &error);
                let state = app.state::<Startup>();
                let mut status = state.0.lock().unwrap_or_else(|e| e.into_inner());
                status.phase = "failed";
                status.failure = Some(Failure { step: "open_connection".into(), error });
                drop(status);
                changed(&app);
            }
        }
    });
}
