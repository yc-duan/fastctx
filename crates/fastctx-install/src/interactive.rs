//! Application maintenance in an existing ordinary interactive user session.
//! No guardian command or privileged arbitrary-process endpoint is introduced.

mod session;
mod task;

use anyhow::{Context as _, ensure};
use std::path::Path;
use std::time::Duration;

/// Whether this user is signed in with an active interactive session now:
/// installing or repairing the application for them waits until they are.
pub fn signed_in() -> anyhow::Result<bool> {
    session::signed_in()
}

/// Copies and verifies the runnable installer immediately before scheduling it.
/// Repair preserves /UPDATE and adds only the runtime-repair hook flag.
pub fn install(root: &Path, archive: &Path, signature: &str, version: &str, repair: bool) -> anyhow::Result<()> {
    let temporary = tempfile::Builder::new()
        .prefix("installer-")
        .tempdir_in(root.join("updates"))
        .with_context(|| format!("preparing installer under {}", root.display()))?;
    let runnable = temporary.path().join("installer.exe");
    std::fs::copy(archive, &runnable)
        .with_context(|| format!("copying installer {} to {}", archive.display(), runnable.display()))?;
    fastctx_update::verify::file(&runnable, signature, version)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("verifying runnable installer {} for {version}", runnable.display()))?;
    let mut args = vec!["/S".into(), "/UPDATE".into()];
    if repair {
        args.push("/FASTCTXREPAIR".into());
    }
    let result = task::run(root, &runnable, &args, Duration::from_secs(600), false);
    let cleaned = temporary.close().with_context(|| format!("removing installer copy {}", runnable.display()));
    result?;
    cleaned
}

fn registered(root: &Path, executable: &Path) -> anyhow::Result<()> {
    let path = root.join("updates/app.json");
    let app = crate::renewal::read_json::<crate::repair::application::Registration>(&path)?
        .with_context(|| format!("no application registered at {}", path.display()))?;
    ensure!(app.executable()? == executable, "{} is not the registered application", executable.display());
    Ok(())
}

pub fn probe(root: &Path, executable: &Path, receipt: &Path, profile: &Path) -> anyhow::Result<()> {
    registered(root, executable)?;
    task::run(
        root,
        executable,
        &["--repair-probe".into(), receipt.display().to_string(), profile.display().to_string()],
        Duration::from_secs(45),
        false,
    )
}

/// Acceptance means Windows created the registered app in its user's session.
/// The task registration is deleted without stopping the long-lived application.
pub fn reopen(root: &Path, executable: &Path) -> anyhow::Result<()> {
    registered(root, executable)?;
    task::run(root, executable, &[], Duration::from_secs(30), true)
}
