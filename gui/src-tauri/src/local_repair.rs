//! The shell can start repair without a World connection or a WebView.

use fastctx_install::repair::application::Registration;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::Manager as _;

#[derive(Serialize, Deserialize)]
pub struct Accepted {
    pub id: String,
    pub accepted: bool,
    pub process: fastctx_types::ProcessIdentity,
}

fn device(directory: Option<&Path>) -> Result<bool, String> {
    if std::env::args_os().any(|arg| arg == "--device") {
        return Ok(true);
    }
    let Some(directory) = directory else { return Ok(false) };
    let path = directory.join("connection.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<crate::connection::Mode>(&bytes)
            .map(|mode| mode == crate::connection::Mode::Device)
            .or(Ok(true)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("reading connection choice {} before repair: {error}", path.display())),
    }
}

pub fn launch(directory: Option<PathBuf>, reopen: bool) -> Result<Accepted, String> {
    let root = fastctx_platform::fastctx_root().map_err(|e| format!("locating repair root: {e}"))?;
    let executable = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .ok_or("locating application executable")?;
    // The no-window entry can resolve the same application data directory
    // without building the runtime or constructing a WebView.
    let directory = match directory {
        Some(directory) => directory,
        None => {
            use tauri::utils::config::AppDirectoriesOverride;
            let context: tauri::Context<tauri::Wry> = tauri::generate_context!();
            let configured = match &context.config().app.app_directories_override {
                Some(AppDirectoriesOverride::Root(path)) => Some(path.clone()),
                Some(AppDirectoriesOverride::Directories(paths)) => paths.data.clone(),
                None => None,
            };
            match configured {
                Some(path) if path.is_absolute() => path,
                Some(path) => {
                    #[cfg(target_os = "macos")]
                    let base = executable
                        .ancestors()
                        .find(|p| p.extension().is_some_and(|x| x == "app"))
                        .unwrap_or(&executable);
                    #[cfg(not(target_os = "macos"))]
                    let base = &executable;
                    base.parent().ok_or("application has no parent directory")?.join(path)
                }
                None => dirs::data_dir().ok_or("locating application data")?.join(&context.config().identifier),
            }
        }
    };
    let application_only = device(Some(&directory))?;
    let carried = crate::setup::carried();
    let path = root.join("updates/app.json");
    {
        fastctx_platform::ensure_private_dir(&root.join("updates"))
            .map_err(|e| format!("preparing {}: {e}", root.join("updates").display()))?;
        let _gate = fastctx_platform::FileLock::try_acquire(&root.join("updates/repair.lock"))
            .map_err(|e| format!("opening repair gate under {}: {e}", root.display()))?
            .ok_or("a repair is already running")?;
        let previous = fastctx_install::renewal::read_json::<Registration>(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let app = Registration {
            app: fastctx_types::update::App {
                version: env!("CARGO_PKG_VERSION").into(),
                target: crate::updates::target(),
            },
            carried: previous.as_ref().and_then(|a| a.carried.clone()),
            program: carried.or_else(|| previous.as_ref().and_then(|a| a.program.clone())),
            executable: Some(executable.clone()),
        };
        let bytes = serde_json::to_vec(&app).map_err(|e| e.to_string())?;
        if previous.as_ref().and_then(|a| serde_json::to_vec(a).ok()).as_ref() != Some(&bytes) {
            fastctx_install::renewal::write_json(&path, &app)
                .map_err(|e| format!("registering application for repair at {}: {e}", path.display()))?;
        }
    }
    let prepared = prepare(&root)?;
    let program = &prepared.program;
    let mut command = std::process::Command::new(program);
    command
        .args(["renew", "repair", "--background", "--dir"])
        .arg(&root)
        .args(["--version", env!("CARGO_PKG_VERSION"), "--by", "application"])
        .env("FASTCTX_HOME", &root)
        .stdin(std::process::Stdio::null());
    if let Some(id) = &prepared.id {
        command.arg("--id").arg(id);
    }
    if application_only {
        command.arg("--application-only");
    }
    if reopen {
        command.arg("--reopen").arg(&executable);
    }
    for name in ["FASTCTX_RESIDENT", "FASTCTX_CONVERSATION", "FASTCTX_SESSION"] {
        command.env_remove(name);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let output = command
        .output()
        .map_err(|e| format!("starting repair with {} for {}: {e}", program.display(), root.display()))?;
    if !output.status.success() {
        return Err(format!(
            "starting repair with {} exited {}: {}",
            program.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("reading repair acceptance from {}: {e}", program.display()))
}

struct Prepared {
    program: PathBuf,
    id: Option<String>,
    _directory: Option<tempfile::TempDir>,
}

fn prepare(root: &Path) -> Result<Prepared, String> {
    if let Some(program) = crate::setup::carried()
        && fastctx_install::program::inspect(&program).is_ok()
    {
        return Ok(Prepared { program, id: None, _directory: None });
    }
    let stable = fastctx_install::renewal::installed(root);
    if fastctx_install::program::inspect(&stable).is_ok_and(|build| {
        fastctx_types::update::release(&build.version) >= fastctx_types::update::release(env!("CARGO_PKG_VERSION"))
    }) {
        return Ok(Prepared { program: stable, id: None, _directory: None });
    }
    let _gate = fastctx_platform::FileLock::try_acquire(&root.join("updates/repair.lock"))
        .map_err(|e| format!("opening repair gate under {}: {e}", root.display()))?
        .ok_or("a repair is already running")?;
    let mut journal = fastctx_install::repair::Journal::start(root, env!("CARGO_PKG_VERSION"), "application", None)
        .map_err(|e| format!("starting shell repair record: {e:#}"))?;
    journal.begin("package").map_err(|e| e.to_string())?;
    let result = (|| -> Result<Prepared, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
        let package = runtime
            .block_on(async {
                let manifest = fastctx_install::repair::package::manifest(root, env!("CARGO_PKG_VERSION")).await?;
                fastctx_install::repair::package::obtain(
                    root,
                    &manifest,
                    &fastctx_update::kernel::target(),
                    &mut journal,
                )
                .await
            })
            .map_err(|e| format!("obtaining a repair program for {}: {e:#}", root.display()))?;
        let directory = tempfile::Builder::new()
            .prefix("shell-repair-")
            .tempdir_in(root.join("updates"))
            .map_err(|e| format!("preparing repair executable under {}: {e}", root.display()))?;
        let program = fastctx_update::kernel::unpack(&package.path, directory.path())?;
        Ok(Prepared { program, id: Some(journal.report.id.clone()), _directory: Some(directory) })
    })();
    match result {
        Ok(prepared) => {
            journal.end(Ok("verified command-line package prepared by the shell".into())).map_err(|e| e.to_string())?;
            Ok(prepared)
        }
        Err(error) => {
            journal.end(Err(std::io::Error::other(error.clone()).into())).map_err(|e| e.to_string())?;
            journal.finish().map_err(|e| e.to_string())?;
            Err(error)
        }
    }
}

/// Returns immediately after the repairer accepts; progress survives this window.
#[fastctx_gui_command::command]
pub async fn repair_here(app: tauri::AppHandle) -> Result<Accepted, String> {
    let directory = app.path().app_data_dir().map_err(|e| format!("locating application data: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || launch(Some(directory), true))
        .await
        .map_err(|e| format!("starting local repair worker: {e}"))?
}

#[fastctx_gui_command::command]
pub async fn repair_state() -> Result<Option<fastctx_types::repair::Report>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let root = fastctx_platform::fastctx_root().map_err(|e| format!("locating repair root: {e}"))?;
        fastctx_install::repair::inspect(&root)
            .map_err(|e| format!("reading {}: {e:#}", root.join("updates/repair.json").display()))
    })
    .await
    .map_err(|e| format!("reading repair worker: {e}"))?
}
