//! A release someone asked from elsewhere that this machine update to, when
//! an application carries its World program (`world-upgrade.md`, 别的机器的
//! 更新). The application here takes it once it is open, as an automatic
//! update would. Closed on Windows, the node installs it: it brings the
//! installer here, runs it silently for this user, and hands the World
//! program the new application carries to the renewal. Elsewhere it waits
//! for the application to be opened.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use fastctx_types::update::{Failed, FailureReason, Fetching, Manifest, Problem, Via};

use super::{AskedFile, Asker, dir, keep};
use crate::Node;

/// How long the application's installer may run.
const INSTALLING: Duration = Duration::from_secs(10 * 60);

/// The interfaces open on this machine now.
static OPEN: AtomicUsize = AtomicUsize::new(0);
static INSTALLING_NOW: AtomicBool = AtomicBool::new(false);
struct Installing;
impl Drop for Installing {
    fn drop(&mut self) {
        INSTALLING_NOW.store(false, Ordering::Release);
    }
}
pub(super) fn busy() -> bool {
    INSTALLING_NOW.load(Ordering::Acquire)
}

/// An interface open on this machine, while it is.
pub(crate) struct Open;

impl Open {
    pub(crate) fn new() -> Self {
        OPEN.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Whether an interface on this machine is open now.
pub(crate) fn open() -> bool {
    OPEN.load(Ordering::Relaxed) > 0
}

/// What is kept of a release the application here is to take.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Take {
    pub version: String,
    #[serde(default)]
    pub manifest: Option<Manifest>,
    #[serde(default)]
    pub origin: Option<super::schedule::Origin>,
}

/// `asker` asks that the application here update to `manifest`, which is
/// newer than it: kept for it to take, and installed now on Windows when it
/// is closed.
pub(super) fn ask(node: &Arc<Node>, manifest: Manifest, asker: Asker, app: &str) -> Result<(), String> {
    let origin = super::schedule::Origin::from_asker(&asker);
    if super::schedule::withdrawn(node, &manifest.version, origin.as_ref()) {
        return Ok(());
    }
    let installing = if !open() && cfg!(windows) {
        if INSTALLING_NOW.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            if node.updates.held.lock().expect("never poisoned").take.as_deref() == Some(&manifest.version) {
                return Ok(());
            }
            return Err("the application is already being installed".to_string());
        }
        Some(Installing)
    } else {
        None
    };
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        let origin =
            origin.or_else(|| (held.take.as_deref() == Some(&manifest.version)).then(|| held.origin.clone()).flatten());
        let asked = AskedFile { by: asker.by, from: env!("CARGO_PKG_VERSION").to_string(), hub: asker.hub };
        keep(node, "asked.json", &asked).map_err(|error| format!("keeping who asked for the update: {error}"))?;
        keep(
            node,
            "take.json",
            &Take { version: manifest.version.clone(), manifest: Some(manifest.clone()), origin: origin.clone() },
        )
        .map_err(|error| format!("keeping the release asked for: {error}"))?;
        held.take = Some(manifest.version.clone());
        held.origin = origin;
        held.chosen = Some(manifest.clone());
        held.failed = None;
        held.activity = None;
    }
    super::report(node);
    let Some(installing) = installing else { return Ok(()) };
    let node = node.clone();
    let target = app.to_string();
    tokio::spawn(async move {
        let _installing = installing;
        if let Err(why) = install(&node, &manifest, &target).await {
            node.log.line(format_args!("installing Fastctx {} while it was closed: {why}", manifest.version));
            node.updates.held.lock().expect("never poisoned").failed =
                Some(Failed { version: manifest.version.clone(), target, why: why.why, reason: Some(why.reason) });
        }
        super::report(&node);
    });
    Ok(())
}

/// The application here says it is `version`: a release asked for that it
/// is, or passes, is taken.
pub(super) fn took(node: &Node, version: &str) {
    let mut held = node.updates.held.lock().expect("never poisoned");
    let taken = held
        .take
        .as_deref()
        .is_some_and(|take| fastctx_types::update::release(version) >= fastctx_types::update::release(take));
    if taken {
        held.take = None;
        held.origin = None;
        drop(held);
        let _ = std::fs::remove_file(dir(node).join("take.json"));
    }
}

/// Brings the application's installer of `manifest` for `target` here, runs
/// it for this user without a word, and stages the World program the new
/// application carries.
async fn install(node: &Arc<Node>, manifest: &Manifest, target: &str) -> Result<(), Problem> {
    let program = node.updates.held.lock().expect("never poisoned").program.clone();
    let program = program.ok_or("the application here never said where it keeps the World program it carries")?;
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        if held.fetching.is_some() {
            return Err("a package is on its way here already".into());
        }
        held.failed = None;
        held.activity = None;
        held.fetching = Some(Fetching {
            version: manifest.version.clone(),
            target: target.to_string(),
            via: Via::Github,
            received: 0,
            total: None,
        });
    }
    let brought = super::bring(node, manifest, target).await;
    node.updates.held.lock().expect("never poisoned").fetching = None;
    let installer = brought?;
    let origin = node.updates.held.lock().expect("never poisoned").origin.clone();
    if open() || super::schedule::withdrawn(node, &manifest.version, origin.as_ref()) {
        return Ok(());
    }
    node.updates.held.lock().expect("never poisoned").activity =
        Some(fastctx_types::update::Stage::Installing { version: manifest.version.clone() });
    super::report(node);
    let installed = run_installer(&installer).await;
    node.updates.held.lock().expect("never poisoned").activity = None;
    installed?;
    let inspecting = program.clone();
    let build = tokio::task::spawn_blocking(move || fastctx_install::program::inspect(&inspecting))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    if build.version != manifest.version {
        return Err(Problem::new(
            FailureReason::Other,
            format!("the installer left World {} instead of {}", build.version, manifest.version),
        ));
    }
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        let mut app = held.app.clone().ok_or("the application's identity disappeared")?;
        app.version = manifest.version.clone();
        let file = super::AppFile { app: app.clone(), carried: held.carried.clone(), program: held.program.clone() };
        keep(node, "app.json", &file).map_err(|e| Problem::new(FailureReason::Disk, e.to_string()))?;
        held.app = Some(app);
    }
    let root = node.root.clone();
    tokio::task::spawn_blocking(move || super::stage(&root, &program)).await.map_err(|error| error.to_string())??;
    took(node, &manifest.version);
    node.log.line(format_args!(
        "installed Fastctx {} while it was closed; its World program takes over once the turns under way end",
        manifest.version
    ));
    Ok(())
}

/// Runs the installer silently, for this user alone, to its end.
async fn run_installer(installer: &Path) -> Result<(), Problem> {
    // The cached package carries no extension; the installer runs as an
    // executable only under one.
    let runnable: PathBuf = installer.with_extension("exe");
    if runnable != installer {
        std::fs::copy(installer, &runnable)
            .map_err(|error| Problem::new(FailureReason::Disk, format!("{}: {error}", runnable.display())))?;
    }
    let mut command = tokio::process::Command::new(&runnable);
    command.arg("/S").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).kill_on_drop(true);
    let status = fastctx_platform::doing::outside("the application's installer", INSTALLING, command.status())
        .await
        .map_err(|_| format!("the installer did not finish within {} minutes", INSTALLING.as_secs() / 60))?
        .map_err(|error| format!("running the installer: {error}"))?;
    if runnable != installer {
        let _ = std::fs::remove_file(&runnable);
    }
    if !status.success() {
        return Err(Problem::new(
            FailureReason::Installer { code: status.code() },
            format!("the installer exited {status}"),
        ));
    }
    Ok(())
}
