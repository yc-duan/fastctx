//! The graphical shell streams the same CLI contract as a terminal. Its
//! own package and WebView files are retired only after the shell exits.

use fastctx_types::uninstall::{Event, Plan};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager as _};

#[derive(Default)]
pub(crate) struct State(std::sync::Mutex<Completed>);

#[derive(Default)]
struct Completed {
    ready: bool,
    /// Kept alive until World exits; the deb's removal is handed it on Linux.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    handoff: Option<tempfile::TempDir>,
}

pub(crate) fn program() -> Result<PathBuf, String> {
    crate::setup::carried()
        .or_else(|| crate::setup::places().ok().map(|places| places.installed()).filter(|path| path.is_file()))
        .ok_or_else(|| "Fastctx has no fastctx executable available to uninstall this computer".into())
}

fn directories(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    let resolver = app.path();
    let mut paths = vec![
        resolver.app_data_dir(),
        resolver.app_local_data_dir(),
        resolver.app_config_dir(),
        resolver.app_cache_dir(),
    ]
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .map_err(|error| error.to_string())?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn command(app: &AppHandle, action: &str, everything: bool) -> Result<Command, String> {
    let mut command = Command::new(program()?);
    command.args(["uninstall", action, "--json"]);
    let application = std::env::current_exe().map_err(|error| error.to_string())?;
    #[cfg(windows)]
    command.arg("--application").arg(application.parent().ok_or("Fastctx has no install directory")?);
    #[cfg(not(windows))]
    command.arg("--application").arg(&application);
    command.arg("--graphical");
    #[cfg(target_os = "linux")]
    if let Some(image) = std::env::var_os("APPIMAGE") {
        command.arg("--application-image").arg(image);
    }
    if everything {
        command.arg("--everything");
    }
    for path in directories(app)? {
        command.arg("--application-dir").arg(path);
    }
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    Ok(command)
}

#[fastctx_gui_command::command]
pub async fn removal_plan(app: AppHandle, everything: bool) -> Result<Plan, String> {
    let mut command = command(&app, "plan", everything)?;
    tauri::async_runtime::spawn_blocking(move || {
        let output = command.output().map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        serde_json::from_slice(&output.stdout).map_err(|error| format!("reading uninstall preview: {error}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[fastctx_gui_command::command]
pub async fn removal_run(
    app: AppHandle,
    shell: tauri::State<'_, crate::Shell>,
    everything: bool,
    events: Channel<Event>,
) -> Result<(), String> {
    // The page lets go of everything it follows, and of what it keeps,
    // before anything is removed: nothing it does writes there again.
    shell.close_page();
    shell.connection.forget_kept();
    if let Some(device) = shell.world().device() {
        device.disconnect();
    }
    let mut command = command(&app, "run", everything)?;
    let mut plan_command = self::command(&app, "plan", everything)?;
    let plan = plan_command.output().map_err(|error| error.to_string())?;
    if !plan.status.success() {
        return Err(String::from_utf8_lossy(&plan.stderr).into_owned());
    }
    let plan: Plan = serde_json::from_slice(&plan.stdout).map_err(|error| error.to_string())?;
    let local = plan
        .steps
        .iter()
        .filter(|step| step.kind != "logout")
        .map(|step| step.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    // Only the deb's removal on Linux is handed a control file. Elsewhere
    // nothing reads it, and its directory would outlive World, which exits
    // without dropping its state.
    let handoff = if cfg!(target_os = "linux") {
        let handoff =
            tempfile::Builder::new().prefix("world-removal-handoff-").tempdir().map_err(|error| error.to_string())?;
        fastctx_platform::ensure_private_dir(handoff.path()).map_err(|error| error.to_string())?;
        command.arg("--application-control").arg(handoff.path().join("finish"));
        Some(handoff)
    } else {
        None
    };
    command.arg("--application-process").arg(
        serde_json::to_string(&fastctx_platform::current_identity().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?,
    );
    let state = app.state::<State>();
    *state.0.lock().expect("removal state") = Completed { ready: false, handoff };
    let result_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead as _, Read as _};
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stderr = child.stderr.take().ok_or("uninstall stderr is absent")?;
        let diagnostic = std::thread::spawn(move || -> std::io::Result<String> {
            let mut text = String::new();
            std::io::BufReader::new(stderr).read_to_string(&mut text)?;
            Ok(text)
        });
        let mut display_failure = None;
        let mut stream_failure = None;
        let mut finished = false;
        let mut done = std::collections::BTreeSet::new();
        for line in std::io::BufReader::new(child.stdout.take().ok_or("uninstall progress is absent")?).lines() {
            let event = line.map_err(|error| error.to_string()).and_then(|line| {
                serde_json::from_str::<Event>(&line).map_err(|error| format!("reading uninstall progress: {error}"))
            });
            let event = match event {
                Ok(event) => event,
                Err(error) => {
                    stream_failure = Some(error);
                    continue;
                }
            };
            if matches!(event, Event::Finished { .. }) {
                finished = true;
            }
            if let Event::Step { step, result, .. } = &event {
                if *result == fastctx_types::uninstall::Result::Done {
                    done.insert(step.clone());
                } else {
                    done.remove(step);
                }
            }
            if let Err(error) = events.send(event) {
                display_failure = Some(error.to_string());
            }
        }
        let status = child.wait().map_err(|error| error.to_string())?;
        let text = diagnostic
            .join()
            .map_err(|_| "reading uninstall diagnostics panicked".to_owned())?
            .map_err(|error| error.to_string())?;
        result_app.state::<State>().0.lock().expect("removal state").ready =
            finished && local.is_subset(&done) && stream_failure.is_none();
        if let Some(error) = stream_failure {
            return Err(error);
        }
        if !status.success() {
            return Err(text);
        }
        if !finished {
            return Err("the uninstall process ended without its final remaining-items report".into());
        }
        if let Some(error) = display_failure {
            return Err(format!("uninstall completed, but displaying progress failed: {error}"));
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[fastctx_gui_command::command]
pub async fn removal_finish(app: AppHandle, everything: bool) -> Result<(), String> {
    if !app.state::<State>().0.lock().expect("removal state").ready {
        return Err("the local kernel steps must finish before removing the application".into());
    }
    finish(&app, everything)?;
    app.exit(0);
    Ok(())
}

#[cfg(windows)]
fn finish(_app: &AppHandle, everything: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let uninstaller = current.parent().ok_or("Fastctx has no install directory")?.join("uninstall.exe");
    if !uninstaller.is_file() {
        return Err("this Fastctx is not installed by an uninstallable package".into());
    }
    let identity = fastctx_platform::current_identity().map_err(|error| error.to_string())?;
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let path = uninstaller.to_str().ok_or("Fastctx's uninstaller path is not Unicode")?;
    // World's WebView2 processes outlive it by a moment and hold the
    // application data a complete removal deletes, so the uninstaller waits
    // for every process World started, found by parent and by having started
    // after World, as well as for World itself; this waiting PowerShell is
    // one of World's children too, and is not waited for.
    let script = format!(
        "$ErrorActionPreference='Stop'; $born=[DateTime]::FromFileTimeUtc([long]{token}); \
         $all=@(Get-CimInstance Win32_Process); $tree=@({pid}); \
         for ($i=0; $i -lt $tree.Count; $i++) {{ $tree += @($all | Where-Object {{ $_.ParentProcessId -eq $tree[$i] -and $_.ProcessId -ne $PID -and $_.CreationDate.ToUniversalTime() -ge $born }} | ForEach-Object {{ [int]$_.ProcessId }}) }}; \
         $p=Get-Process -Id {pid} -ErrorAction SilentlyContinue; \
         if ($null -ne $p -and $p.StartTime.ToUniversalTime().ToFileTimeUtc().ToString() -eq {token}) {{ $p.WaitForExit() }}; \
         @($tree | Select-Object -Skip 1 | ForEach-Object {{ Get-Process -Id $_ -ErrorAction SilentlyContinue }} | Where-Object {{ $_.StartTime.ToUniversalTime() -ge $born }}) | ForEach-Object {{ $_.WaitForExit() }}; \
         & {path} /S /FASTCTXKERNELDONE {everything}",
        pid = identity.pid,
        token = quote(&identity.token),
        path = quote(path),
        everything = if everything { "/FASTCTXEVERYTHING" } else { "" }
    );
    let mut command = Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(0x08000000);
    command.spawn().map_err(|error| format!("starting Fastctx's package uninstaller: {error}"))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn finish(app: &AppHandle, everything: bool) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    if current == std::path::Path::new("/usr/bin/fastctx-app") {
        let state = app.state::<State>();
        let mut completed = state.0.lock().expect("removal state");
        let handoff = completed.handoff.take().ok_or("the Debian package handoff is absent")?;
        std::fs::write(handoff.path().join("finish"), b"finish\n").map_err(|error| error.to_string())?;
        let _ = handoff.keep();
        return Ok(());
    }
    let image = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .ok_or("this Fastctx is not running from an installed deb or an AppImage")?;
    let identity = fastctx_platform::current_identity().map_err(|error| error.to_string())?;
    let temporary =
        tempfile::Builder::new().prefix("fastctx-appimage-uninstall-").tempdir().map_err(|error| error.to_string())?;
    fastctx_platform::ensure_private_dir(temporary.path()).map_err(|error| error.to_string())?;
    let helper = temporary.path().join("fastctx");
    std::fs::copy(program()?, &helper).map_err(|error| error.to_string())?;
    let mut command = Command::new(&helper);
    command
        .args(["uninstall", "application", "--image"])
        .arg(image)
        .arg("--process")
        .arg(serde_json::to_string(&identity).map_err(|error| error.to_string())?)
        .arg("--temporary")
        .arg(temporary.path());
    if everything {
        for path in directories(app)? {
            command.arg("--data").arg(path);
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let _ = temporary.keep();
    Ok(())
}

/// On macOS the application is the `.app` bundle it runs from: a copy of
/// fastctx outside it removes it once Fastctx has exited.
#[cfg(target_os = "macos")]
fn finish(app: &AppHandle, everything: bool) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let bundle = current
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .ok_or("this Fastctx is not running from an application bundle")?
        .to_path_buf();
    let identity = fastctx_platform::current_identity().map_err(|error| error.to_string())?;
    let temporary =
        tempfile::Builder::new().prefix(fastctx_uninstall_prefix()).tempdir().map_err(|error| error.to_string())?;
    fastctx_platform::ensure_private_dir(temporary.path()).map_err(|error| error.to_string())?;
    let helper = temporary.path().join("fastctx");
    std::fs::copy(program()?, &helper).map_err(|error| error.to_string())?;
    let mut command = Command::new(&helper);
    command
        .args(["uninstall", "bundle", "--bundle"])
        .arg(&bundle)
        .arg("--process")
        .arg(serde_json::to_string(&identity).map_err(|error| error.to_string())?)
        .arg("--temporary")
        .arg(temporary.path());
    if everything {
        for path in directories(app)? {
            command.arg("--data").arg(path);
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let _ = temporary.keep();
    Ok(())
}

/// The prefix of the removal helper's private directory, as the helper
/// checks it.
#[cfg(target_os = "macos")]
fn fastctx_uninstall_prefix() -> &'static str {
    "fastctx-bundle-uninstall-"
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn finish(_app: &AppHandle, _everything: bool) -> Result<(), String> {
    Err("package uninstall is supported on Windows, Linux and macOS".into())
}
