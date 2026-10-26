//! Checking the installed application in an isolated WebView profile, and
//! replacing only its program files from a verified release package.

use super::Journal;
use anyhow::{Context as _, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::time::{Duration, Instant};

#[derive(Clone, Serialize, Deserialize)]
pub struct Registration {
    pub app: fastctx_types::update::App,
    #[serde(default)]
    pub carried: Option<PathBuf>,
    #[serde(default)]
    pub program: Option<PathBuf>,
    #[serde(default)]
    pub executable: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
pub struct Probe {
    pub version: String,
    pub error: Option<String>,
}

impl Registration {
    pub fn executable(&self) -> anyhow::Result<PathBuf> {
        if let Some(path) = &self.executable {
            return Ok(path.clone());
        }
        let program = self.program.as_ref().context("application did not record its program path")?;
        Ok(program.parent().context("application program has no parent")?.join(if cfg!(windows) {
            "fastctx-app.exe"
        } else {
            "fastctx-app"
        }))
    }
}

#[derive(Debug)]
struct ProtectedApplication(String);
impl std::fmt::Display for ProtectedApplication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for ProtectedApplication {}

pub(super) fn program_build(root: &Path, app: &Registration) -> anyhow::Result<fastctx_types::renewal::Build> {
    #[cfg(target_os = "linux")]
    if app.app.target.ends_with("-appimage") {
        return super::appimage::program(root, app);
    }
    let _ = root;
    let program = app.program.as_ref().context("application did not record its carried program")?;
    crate::program::inspect(program).with_context(|| format!("checking application program {}", program.display()))
}

pub fn check(root: &Path, app: &Registration, build: &fastctx_types::renewal::Build) -> anyhow::Result<()> {
    let actual = program_build(root, app)?;
    if actual != *build {
        let forward = crate::program::newer(build, &actual).map_err(|error| {
            ProtectedApplication(format!("checking application version before replacement: {error:#}"))
        })?;
        if !forward {
            return Err(ProtectedApplication(format!(
                "application {} carries {} newer than or unordered against repair {}; no application files changed",
                app.executable()?.display(),
                actual.version,
                build.version
            ))
            .into());
        }
        anyhow::bail!(
            "application {} carries {}, expected {}",
            app.executable()?.display(),
            actual.version,
            build.version
        );
    }
    let executable = app.executable()?;
    let receipt = root.join("updates").join(format!("application-probe-{}.json", std::process::id()));
    let profile = root.join("updates").join(format!("application-probe-{}", std::process::id()));
    #[cfg(not(windows))]
    let mut child = std::process::Command::new(&executable)
        .arg("--repair-probe")
        .arg(&receipt)
        .arg(&profile)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("probing application {}", executable.display()))?;
    let result = (|| -> anyhow::Result<()> {
        #[cfg(windows)]
        if let Err(error) = crate::interactive::probe(root, &executable, &receipt, &profile) {
            let detail = crate::renewal::read_json::<Probe>(&receipt).ok().flatten().and_then(|p| p.error);
            return Err(error).with_context(|| {
                format!(
                    "probing {}: {}",
                    executable.display(),
                    detail.as_deref().unwrap_or("no WebView receipt was returned")
                )
            });
        }
        #[cfg(not(windows))]
        {
            let began = Instant::now();
            loop {
                if let Some(status) = child.try_wait()? {
                    if !status.success() {
                        let reason = crate::renewal::read_json::<Probe>(&receipt)
                            .ok()
                            .flatten()
                            .and_then(|p| p.error)
                            .unwrap_or_else(|| "no WebView error receipt".into());
                        anyhow::bail!("application probe {} exited {status}: {reason}", executable.display());
                    }
                    break;
                }
                ensure!(
                    began.elapsed() < Duration::from_secs(45),
                    "application {} did not open its probe window in 45 seconds",
                    executable.display()
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let probe = crate::renewal::read_json::<Probe>(&receipt)?
            .with_context(|| format!("application {} produced no window receipt", executable.display()))?;
        if fastctx_types::update::release(&probe.version) > fastctx_types::update::release(&build.version) {
            return Err(ProtectedApplication(format!(
                "application {} reports {}, newer than repair {}; no application files changed",
                executable.display(),
                probe.version,
                build.version
            ))
            .into());
        }
        ensure!(
            probe.version == build.version,
            "application {} reports {}, expected {}",
            executable.display(),
            probe.version,
            build.version
        );
        ensure!(probe.error.is_none(), "application {}: {}", executable.display(), probe.error.unwrap_or_default());
        Ok(())
    })();
    let cleanup = (|| -> anyhow::Result<()> {
        // This is only the directly owned probe child, never a discovered process.
        #[cfg(not(windows))]
        if child.try_wait()?.is_none() {
            child.kill()?;
            child.wait()?;
        }
        if receipt.try_exists()? {
            std::fs::remove_file(&receipt).with_context(|| format!("deleting probe receipt {}", receipt.display()))?;
        }
        if profile.try_exists()? {
            std::fs::remove_dir_all(&profile)
                .with_context(|| format!("deleting probe profile {}", profile.display()))?;
        }
        Ok(())
    })();
    match (result, cleanup) {
        (Err(primary), Err(cleanup)) => Err(anyhow::anyhow!("{primary:#}; probe cleanup also failed: {cleanup:#}")),
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

/// Serializes application file changes with the shell and background installer.
pub fn lock(root: &Path) -> anyhow::Result<fastctx_platform::FileLock> {
    let directory = root.join("updates");
    fastctx_platform::ensure_private_dir(&directory)
        .with_context(|| format!("preparing application maintenance directory {}", directory.display()))?;
    let path = directory.join("application.lock");
    fastctx_platform::FileLock::try_acquire(&path)
        .with_context(|| format!("opening application maintenance gate {}", path.display()))?
        .context("an application installation or repair is already running")
}

pub async fn repair(
    root: &Path,
    build: &fastctx_types::renewal::Build,
    journal: &mut Journal,
) -> anyhow::Result<String> {
    let _application = lock(root)?;
    let path = root.join("updates/app.json");
    let Some(mut app) = crate::renewal::read_json::<Registration>(&path)? else {
        return Ok("no application registered".into());
    };
    match check(root, &app, build) {
        Ok(()) => {
            if app.app.version != build.version {
                journal.action(format!(
                    "refresh application version in {} from {} to {}",
                    path.display(),
                    app.app.version,
                    build.version
                ))?;
                app.app.version = build.version.clone();
                crate::renewal::write_json(&path, &app)?;
            }
            return Ok("application version, carried program and WebView startup are correct".into());
        }
        Err(error) if error.is::<ProtectedApplication>() => return Err(error),
        Err(error) => journal.action(format!("reinstall application {}: {error:#}", app.executable()?.display()))?,
    }
    ensure!(
        fastctx_types::update::release(&app.app.version) <= fastctx_types::update::release(&build.version),
        "application is newer than repair {}",
        build.version
    );
    let manifest = super::package::manifest(root, &build.version).await?;
    let installer = super::package::obtain(root, &manifest, &app.app.target, journal).await?;
    if installer.brought {
        journal.action(format!("obtain and verify {}", installer.path.display()))?;
    }
    install(root, &app, &installer.path, &manifest).await?;
    check(root, &app, build)?;
    app.app.version = build.version.clone();
    crate::renewal::write_json(&path, &app)?;
    Ok("application reinstalled and its isolated window opened".into())
}

pub async fn install(
    root: &Path,
    app: &Registration,
    installer: &Path,
    manifest: &fastctx_types::update::Manifest,
) -> anyhow::Result<()> {
    let package = manifest.package(&app.app.target).context("manifest has no application package")?;
    fastctx_update::verify::file(installer, &package.signature, &manifest.version).map_err(anyhow::Error::msg)?;
    #[cfg(windows)]
    {
        let (root, installer, signature, version) =
            (root.to_owned(), installer.to_owned(), package.signature.clone(), manifest.version.clone());
        tokio::task::spawn_blocking(move || crate::interactive::install(&root, &installer, &signature, &version, true))
            .await
            .context("joining interactive application installer")?
    }
    #[cfg(target_os = "macos")]
    {
        let executable = app.executable()?;
        let bundle = executable
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .context("application has no .app directory")?;
        let parent = bundle.parent().context("application bundle has no parent")?;
        let staging = parent.join(format!(".fastctx-repair-{}", std::process::id()));
        std::fs::create_dir(&staging)?;
        let result = (|| -> anyhow::Result<()> {
            let status = std::process::Command::new("/usr/bin/tar")
                .arg("-xzf")
                .arg(installer)
                .arg("-C")
                .arg(&staging)
                .status()?;
            ensure!(status.success(), "unpacking {} exited {status}", installer.display());
            let replacement = std::fs::read_dir(&staging)?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .find(|e| e.path().extension().is_some_and(|x| x == "app"))
                .context("application package has no bundle")?
                .path();
            let backup = staging.join("previous.app");
            std::fs::rename(bundle, &backup)
                .with_context(|| format!("moving {} to {}", bundle.display(), backup.display()))?;
            if let Err(error) = std::fs::rename(&replacement, bundle) {
                std::fs::rename(&backup, bundle)
                    .with_context(|| format!("restoring {} after replacement failed: {error}", bundle.display()))?;
                return Err(error).with_context(|| format!("publishing application {}", bundle.display()));
            }
            Ok(())
        })();
        if result.is_ok() {
            std::fs::remove_dir_all(&staging)?;
        }
        let _ = root;
        result
    }
    #[cfg(target_os = "linux")]
    {
        if app.app.target.ends_with("-deb") {
            let mut command =
                tokio::process::Command::new(if fastctx_platform::elevation::is_elevated()? { "dpkg" } else { "sudo" });
            if !fastctx_platform::elevation::is_elevated()? {
                command.args(["-n", "dpkg"]);
            }
            let output = command.arg("-i").arg(installer).kill_on_drop(true).output();
            let output = tokio::time::timeout(Duration::from_secs(600), output)
                .await
                .context("installing deb exceeded ten minutes")??;
            ensure!(
                output.status.success(),
                "installing {} requires sudo dpkg -i {}: {}",
                installer.display(),
                fastctx_platform::shell_quote::path(installer),
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            let destination =
                app.executable.as_ref().context("the AppImage did not record its original executable path")?;
            ensure!(app.app.target.ends_with("-appimage"), "{} is not the registered AppImage", destination.display());
            crate::renewal::publish_image(installer, destination)
                .with_context(|| format!("replacing AppImage {}", destination.display()))?;
        }
        let _ = root;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (root, installer);
        anyhow::bail!("application repair is not supported on this platform")
    }
}
