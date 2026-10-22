//! Every delivery route converges on the stable World program and its renewal.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail, ensure};
use fastctx_types::renewal::Phase;

/// Places a first or newer kernel. An older delivery uses the already newer
/// World program; it never changes either the installed image or the plan.
/// Host configuration is written only after the replacement has taken over.
pub fn ensure(root: &Path, source: &Path) -> anyhow::Result<PathBuf> {
    let candidate = crate::program::inspect(source)?;
    let installed = crate::renewal::installed(root);
    let mut source = source;
    if installed.is_file() {
        let current = crate::program::inspect(&installed)?;
        // This very program is in place already, and no replacement by it is
        // still under way: there is nothing to place, nor to wait for.
        if current == candidate
            && crate::renewal::read(root)?
                .is_none_or(|plan| plan.phase == Phase::Complete || plan.candidate.build != candidate)
        {
            return Ok(installed);
        }
        if current != candidate && !crate::program::newer(&candidate, &current)? {
            source = &installed;
        }
    }
    let plan = crate::renewal::stage(root, source, None)?;
    if plan.phase == Phase::Complete {
        ensure!(
            crate::renewal::digest(&installed)? == plan.candidate.build.digest,
            "installed kernel differs from completed renewal"
        );
        if let Err(error) = crate::program_collection::collect(root) {
            eprintln!("fastctx: collecting unused programs: {error:#}");
        }
        return Ok(installed);
    }
    crate::renewal::request(root, true)?;
    loop {
        let plan = crate::renewal::read(root)?.context("kernel renewal lost its plan")?;
        match plan.phase {
            Phase::Complete => {
                ensure!(
                    crate::renewal::digest(&installed)? == plan.candidate.build.digest,
                    "installed kernel differs from completed renewal"
                );
                if let Err(error) = crate::program_collection::collect(root) {
                    eprintln!("fastctx: collecting unused programs: {error:#}");
                }
                return Ok(installed);
            }
            Phase::Failed => {
                bail!("kernel renewal failed: {}", plan.error.unwrap_or_else(|| "no failure detail recorded".into()))
            }
            // Published and serving, its boot service still to take it,
            // which goes on by itself: the program is in place.
            Phase::AwaitingAdministrator | Phase::UpdatingGuardian | Phase::ReturningToService
                if crate::renewal::digest(&installed)? == plan.candidate.build.digest =>
            {
                return Ok(installed);
            }
            Phase::AwaitingLegacyExit => {
                bail!("the prepared kernel waits for the legacy node to exit; the installed program has not changed")
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// Serializes publication with newly started nodes and tool adapters. This
/// lock is held through handover, never for an ordinary process's lifetime.
pub fn activation(root: &Path) -> std::io::Result<fastctx_platform::FileLock> {
    fastctx_platform::ensure_private_dir(&crate::renewal::directory(root))?;
    fastctx_platform::FileLock::acquire(&crate::renewal::directory(root).join("activation.lock"))
}

pub fn node_activation(root: &Path) -> std::io::Result<fastctx_platform::FileLock> {
    fastctx_platform::ensure_private_dir(&crate::renewal::directory(root))?;
    fastctx_platform::FileLock::acquire(&crate::renewal::directory(root).join("node-entry.lock"))
}
