//! New versions of the application (`world-upgrade.md`, 新版本自己往后怎样升级):
//! how they stand, from this computer's node or this phone's own device; the
//! package brought here by them, from GitHub or through the network; and
//! installing it. A computer installs with the official updater, handed the
//! package the node checked through an address on this computer alone, so a
//! package that came through the network is installed as one from GitHub
//! is; a phone hands it to the system's installer. After a computer's update
//! the World program the new application carries is put in place, as the
//! user asked for it with the update.

use fastctx_types::update::{App, Settings, State, UpdateCall};
use fastctx_types::{Call, Reply};

use crate::Shell;

/// The updater target this application's packages are for: the official
/// updater's `<os>-<arch>-<installer>`, or the phone's own.
pub(crate) fn target() -> String {
    if cfg!(target_os = "android") {
        return "android-apk".to_string();
    }
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    let arch = std::env::consts::ARCH;
    #[cfg(desktop)]
    let installer = tauri::utils::platform::bundle_type().map(|bundle| match bundle {
        tauri::utils::config::BundleType::Deb => "deb",
        tauri::utils::config::BundleType::Rpm => "rpm",
        tauri::utils::config::BundleType::AppImage => "appimage",
        tauri::utils::config::BundleType::Msi => "msi",
        tauri::utils::config::BundleType::Nsis => "nsis",
        tauri::utils::config::BundleType::App | tauri::utils::config::BundleType::Dmg => "app",
    });
    #[cfg(not(desktop))]
    let installer: Option<&str> = None;
    match installer {
        Some(installer) => format!("{os}-{arch}-{installer}"),
        None => format!("{os}-{arch}"),
    }
}

/// Where this application keeps the command-line programs it carries.
static CARRIED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

fn app() -> App {
    App { version: env!("CARGO_PKG_VERSION").to_string(), target: target() }
}

/// Asks, and tells the node or the device which application this is
/// whenever what it keeps of it is not this one: a node that was not up
/// when the application started, or one an older application spoke to.
async fn ask(shell: &Shell, call: UpdateCall) -> Result<State, String> {
    let state = once(shell, call).await?;
    if state.app.as_ref() == Some(&app()) {
        return Ok(state);
    }
    once(
        shell,
        UpdateCall::App {
            app: app(),
            carried: CARRIED.get().cloned().flatten(),
            program: program(),
            executable: executable(),
        },
    )
    .await
}

async fn once(shell: &Shell, call: UpdateCall) -> Result<State, String> {
    match shell.world().call(Call::Update(call)).await.map_err(|error| error.to_string())?.0 {
        Reply::Update(state) => Ok(*state),
        other => Err(format!("the World answered {other:?}")),
    }
}

/// Tells the node or the device which application this is, and where it
/// keeps the command-line programs it carries.
pub(crate) async fn introduce(shell: &Shell, carried: Option<std::path::PathBuf>) -> Result<(), String> {
    let carried = carried.filter(|dir| dir.is_dir()).map(|dir| dir.display().to_string());
    let _ = CARRIED.set(carried.clone());
    once(shell, UpdateCall::App { app: app(), carried, program: program(), executable: executable() }).await.map(|_| ())
}

/// The World program this application carries, beside its own executable.
fn program() -> Option<String> {
    #[cfg(desktop)]
    return crate::setup::carried().map(|path| path.display().to_string());
    #[cfg(not(desktop))]
    None
}

#[fastctx_gui_command::command]
pub(crate) async fn update_state(shell: tauri::State<'_, Shell>) -> Result<State, String> {
    ask(&shell, UpdateCall::State).await
}

#[fastctx_gui_command::command]
pub(crate) async fn update_look(shell: tauri::State<'_, Shell>) -> Result<State, String> {
    ask(&shell, UpdateCall::Look).await
}

#[fastctx_gui_command::command]
pub(crate) async fn update_set(shell: tauri::State<'_, Shell>, settings: Settings) -> Result<State, String> {
    ask(&shell, UpdateCall::Set { settings }).await
}

/// Reports the local interface's waiting or installation activity. Idle clears it.
#[fastctx_gui_command::command]
pub(crate) async fn update_progress(
    shell: tauri::State<'_, Shell>,
    stage: fastctx_types::update::Stage,
) -> Result<State, String> {
    let current = ask(&shell, UpdateCall::State).await?;
    if !current.progress {
        return Err("this World program does not accept update activity reports".into());
    }
    once(&shell, UpdateCall::Progress { stage }).await
}

/// Accepts repair without keeping the interface's request open through handover.
#[fastctx_gui_command::command]
pub(crate) async fn update_repair(shell: tauri::State<'_, Shell>, version: Option<String>) -> Result<State, String> {
    let current = ask(&shell, UpdateCall::State).await?;
    if !current.repair_supported {
        return Err("this World program is too old or does not support local repair".into());
    }
    once(&shell, UpdateCall::Repair { version, id: None }).await
}

/// Brings this application's package of `version` here.
#[fastctx_gui_command::command]
pub(crate) async fn update_fetch(shell: tauri::State<'_, Shell>, version: String) -> Result<State, String> {
    ask(&shell, UpdateCall::Fetch { version, target: target() }).await
}

/// What this application offers to update to is `version` from now on;
/// none, the newest on its line again.
#[fastctx_gui_command::command]
pub(crate) async fn update_choose(shell: tauri::State<'_, Shell>, version: Option<String>) -> Result<State, String> {
    ask(&shell, UpdateCall::Choose { version }).await
}

/// Reads or changes the account's update view on its hub.
#[fastctx_gui_command::command]
pub(crate) async fn network_updates(
    shell: tauri::State<'_, Shell>,
    hub: String,
    account: String,
    request: fastctx_types::update::ViewCall,
) -> Result<fastctx_types::update::ViewReply, String> {
    let hub = hub.parse::<fastctx_types::HubName>().map_err(|e| e.to_string())?;
    let account = account.parse::<fastctx_types::AccountName>().map_err(|e| e.to_string())?;
    match shell.world().call(Call::NetworkUpdates { hub, account, request }).await.map_err(|e| e.to_string())?.0 {
        Reply::NetworkUpdates(reply) => Ok(reply),
        other => Err(format!("the World answered {other:?}")),
    }
}

/// Has `machine_name` update its World program itself.
#[fastctx_gui_command::command]
pub(crate) async fn update_machine(shell: tauri::State<'_, Shell>, machine_name: String) -> Result<State, String> {
    let machine = fastctx_types::access::Written::try_from(machine_name).map_err(|error| error.to_string())?;
    match shell.world().call(Call::UpdateMachine { machine }).await.map_err(|error| error.to_string())?.0 {
        Reply::Update(state) => Ok(*state),
        other => Err(format!("the World answered {other:?}")),
    }
}

/// Asks `machine_name`, another machine of the network, about its updates:
/// how they stand, a look, its choices, or an update to a release.
#[fastctx_gui_command::command]
pub(crate) async fn machine_updates(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    mut update: UpdateCall,
) -> Result<State, String> {
    let machine = fastctx_types::access::Written::try_from(machine_name).map_err(|error| error.to_string())?;
    if matches!(update, UpdateCall::App { .. } | UpdateCall::Fetch { .. } | UpdateCall::Progress { .. }) {
        return Err("only an interface on that machine says what it is or brings packages there".to_string());
    }
    if matches!(update, UpdateCall::Repair { .. }) && !once(&shell, UpdateCall::State).await?.repair_routing {
        return Err("this device's World program is too old to route repair; update it first".into());
    }
    if let UpdateCall::Preferences { auto, relays } = update {
        let local = once(&shell, UpdateCall::State).await?;
        update = if local.preferences {
            UpdateCall::Preferences { auto, relays }
        } else {
            if local.running.as_deref().and_then(fastctx_types::update::release)
                < fastctx_types::update::release(fastctx_types::update::ASKED_SINCE)
            {
                return Err("update the World program on this device before changing remote update settings".into());
            }
            let (reply, _) = shell
                .world()
                .call(Call::MachineUpdates { machine: machine.clone(), update: UpdateCall::State })
                .await
                .map_err(|e| e.to_string())?;
            let Reply::Update(current) = reply else {
                return Err("the machine did not report its update settings".into());
            };
            UpdateCall::Set { settings: current.settings.with_preferences(auto, relays) }
        };
    }
    match shell.world().call(Call::MachineUpdates { machine, update }).await.map_err(|error| error.to_string())?.0 {
        Reply::Update(state) => Ok(*state),
        other => Err(format!("the World answered {other:?}")),
    }
}

/// Installs this application's package of `version`, which is here: on a
/// computer the application then starts again as the new version, on a
/// phone the system asks the user. `false` when the phone first sent the
/// user to allow installing apps.
#[fastctx_gui_command::command]
pub(crate) async fn update_install(
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
    version: String,
) -> Result<bool, String> {
    let mut state = ask(&shell, UpdateCall::State).await?;
    if state.preferences
        && !state
            .offer
            .iter()
            .chain(state.chosen.iter())
            .chain(state.looked.newer.iter())
            .any(|manifest| manifest.version == version)
    {
        state = ask(&shell, UpdateCall::Choose { version: Some(version.clone()) }).await?;
    }
    let target = target();
    let kept = state
        .kept
        .iter()
        .find(|kept| kept.version == version && kept.target == target)
        .ok_or("the new version is not here yet")?;
    if state.progress {
        once(
            &shell,
            UpdateCall::Progress { stage: fastctx_types::update::Stage::Installing { version: version.clone() } },
        )
        .await?;
    }
    #[cfg(target_os = "android")]
    {
        let installed = crate::android::install_package(&app, kept.path.clone()).await;
        installation_outcome(&shell, &state, &version, &installed).await;
        installed
    }
    #[cfg(desktop)]
    {
        let manifest = [state.offer.as_ref(), state.chosen.as_ref()]
            .into_iter()
            .flatten()
            .chain(
                state.network.as_ref().into_iter().flat_map(|network| network.stable.iter().chain(network.beta.iter())),
            )
            .chain(state.looked.stable.iter().chain(state.looked.beta.iter()))
            .chain(state.looked.newer.iter())
            .find(|manifest| manifest.version == version)
            .ok_or("the new version's manifest is not known here")?;
        let package = manifest.package(&target).ok_or("the new version has no package for this computer")?;
        let installed =
            desktop::install(&app, &version, &target, package, std::path::Path::new(&kept.path)).await.map(|()| true);
        installation_outcome(&shell, &state, &version, &installed).await;
        installed
    }
}

async fn installation_outcome(shell: &Shell, state: &State, version: &str, outcome: &Result<bool, String>) {
    if !state.progress {
        return;
    }
    use fastctx_types::update::{FailureReason, Problem, Stage};
    let stage = match outcome {
        Err(why) => Stage::Failed { version: version.into(), problem: Problem::new(FailureReason::Other, why.clone()) },
        Ok(false) => Stage::Ready { version: version.into() },
        Ok(true) => return,
    };
    let _ = once(shell, UpdateCall::Progress { stage }).await;
}

/// After a computer's update, the World program the new application carries
/// is put in place once.
#[cfg(desktop)]
pub(crate) fn after_update(app: &tauri::AppHandle) {
    desktop::after_update(app);
}

/// Takes what an update left for this start as the program starts, before
/// its window: a first start after an update can be slow to open one, and
/// the note is acted on once the World is open (`after_update`).
#[cfg(desktop)]
pub(crate) fn take_left(identifier: &str) {
    desktop::take_left(identifier);
}

#[cfg(desktop)]
mod desktop {
    use std::path::{Path, PathBuf};

    use fastctx_types::update::Package;
    use tauri::Manager as _;
    use tauri_plugin_updater::UpdaterExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// Kept between the update and the new application's first start.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Updated {
        from: String,
        to: String,
    }

    fn marker(app: &tauri::AppHandle) -> Result<PathBuf, String> {
        Ok(app
            .path()
            .app_data_dir()
            .map_err(|error| format!("locating application update marker directory: {error}"))?
            .join("updated.json"))
    }

    pub(super) async fn install(
        app: &tauri::AppHandle,
        version: &str,
        target: &str,
        package: &Package,
        path: &Path,
    ) -> Result<(), String> {
        let root =
            fastctx_platform::fastctx_root().map_err(|e| format!("locating application maintenance root: {e}"))?;
        let _application = fastctx_install::repair::application::lock(&root).map_err(|e| format!("{e:#}"))?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| format!("opening local installer source for {}: {error}", path.display()))?;
        let port = listener.local_addr().map_err(|error| error.to_string())?.port();
        let manifest = serde_json::json!({
            "version": version,
            "platforms": { target: { "url": format!("http://127.0.0.1:{port}/package"), "signature": package.signature } },
        });
        let serving = tokio::spawn(serve(listener, manifest.to_string(), path.to_path_buf()));
        let installed = async {
            let endpoint =
                format!("http://127.0.0.1:{port}/latest.json").parse().map_err(|error| format!("{error}"))?;
            let updater = app
                .updater_builder()
                .endpoints(vec![endpoint])
                .map_err(|error| format!("setting updater source for {}: {error}", path.display()))?
                .target(target)
                .build()
                .map_err(|error| {
                    format!("constructing updater for {target} {version} from {}: {error}", path.display())
                })?;
            let update = updater
                .check()
                .await
                .map_err(|error| {
                    format!("checking local update manifest for {version} from {}: {error}", path.display())
                })?
                .ok_or("the updater does not take this version as newer")?;
            // The updater checks the signature again as it reads the package.
            let bytes = update.download(|_, _| {}, || {}).await.map_err(|error| {
                format!("reading and verifying installer {} for {version}: {error}", path.display())
            })?;
            let marker = marker(app)?;
            if let Some(dir) = marker.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|error| format!("creating update marker directory {}: {error}", dir.display()))?;
            }
            let updated = Updated { from: env!("CARGO_PKG_VERSION").to_string(), to: version.to_string() };
            fastctx_install::renewal::write_json(&marker, &updated)
                .map_err(|error| format!("saving update marker {}: {error}", marker.display()))?;
            update
                .install(bytes)
                .map_err(|error| format!("installing application {version} from {}: {error}", path.display()))
        };
        let outcome: Result<(), String> = installed.await;
        serving.abort();
        outcome?;
        // Windows' installer has ended this application and starts the new
        // one itself; elsewhere the new one is started here.
        app.restart();
    }

    /// Answers the updater's two requests: the manifest naming the package
    /// here, and the package.
    async fn serve(listener: tokio::net::TcpListener, manifest: String, package: PathBuf) {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (manifest, package) = (manifest.clone(), package.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut byte = [0_u8; 1];
                while !request.ends_with(b"\r\n\r\n") && request.len() < 16 * 1024 {
                    match stream.read(&mut byte).await {
                        Ok(1) => request.push(byte[0]),
                        _ => return,
                    }
                }
                let line = String::from_utf8_lossy(&request).lines().next().unwrap_or_default().to_string();
                let (kind, body) = if line.starts_with("GET /latest.json ") {
                    ("application/json", manifest.into_bytes())
                } else if line.starts_with("GET /package ") {
                    match tokio::fs::read(&package).await {
                        Ok(bytes) => ("application/octet-stream", bytes),
                        Err(_) => return,
                    }
                } else {
                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
                    return;
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
                let _ = stream.shutdown().await;
            });
        }
    }

    /// The new application's first start after an update puts the World
    /// program it carries in place; the renewal waits for the turns under
    /// way to end.
    /// What the update left, once taken at the start.
    static LEFT: std::sync::Mutex<Option<Updated>> = std::sync::Mutex::new(None);

    /// The note in the application's data folder, as Tauri names it:
    /// the platform's data directory and the application's identifier.
    pub(super) fn take_left(identifier: &str) {
        let Some(marker) = dirs::data_dir().map(|dir| dir.join(identifier).join("updated.json")) else { return };
        if let Some(updated) = take(&marker) {
            *LEFT.lock().unwrap_or_else(|error| error.into_inner()) = Some(updated);
        }
    }

    fn take(marker: &Path) -> Option<Updated> {
        let bytes = std::fs::read(marker).ok()?;
        let _ = std::fs::remove_file(marker);
        serde_json::from_slice::<Updated>(&bytes).ok()
    }

    pub(super) fn after_update(app: &tauri::AppHandle) {
        let left = LEFT.lock().unwrap_or_else(|error| error.into_inner()).take();
        let Some(updated) = left.or_else(|| marker(app).ok().and_then(|marker| take(&marker))) else { return };
        if updated.to != env!("CARGO_PKG_VERSION") {
            return;
        }
        crate::setup::take_carried();
        forget_downloads(app);
    }

    /// The updater keeps each installer it downloaded in a folder of its own
    /// under the temporary directory, named after the application and the
    /// version it brought; once an update has started this version, none of
    /// them is needed again. One still held by an installer finishing stays
    /// until the next update.
    fn forget_downloads(app: &tauri::AppHandle) {
        let prefix = format!("{}-", app.package_info().name);
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name.contains("-updater-") && entry.path().is_dir() {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

fn executable() -> Option<String> {
    #[cfg(desktop)]
    {
        std::env::var_os("APPIMAGE")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::current_exe().ok())
            .map(|p| p.display().to_string())
    }
    #[cfg(mobile)]
    {
        None
    }
}
