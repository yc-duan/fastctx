//! Immutable renewal images remain only while a plan or process uses them.

use std::fs;
use std::path::Path;

use fastctx_platform::{FileLock, image_usage};
use fastctx_types::renewal::Phase;

use crate::renewal as store;

/// Shares the renewal worker and launch gates; no image can be staged or
/// started from a plan while it is being collected. Job reservations are
/// checked by the node for legacy program directories separately. The
/// collection lock is held around the others, so a placement that finds the
/// worker lock taken can tell a collection, which it waits out, from a
/// renewal (`renewal::stage`).
pub fn collect(root: &Path) -> anyhow::Result<()> {
    let renewal = store::directory(root);
    fastctx_platform::ensure_private_dir(&renewal)?;
    let Some(_collecting) = FileLock::try_acquire(&store::collection_lock(root))? else { return Ok(()) };
    let Some(_worker) = FileLock::try_acquire(&renewal.join("worker.lock"))? else { return Ok(()) };
    let Some(_entry) = FileLock::try_acquire(&renewal.join("node-entry.lock"))? else { return Ok(()) };
    let Some(_activation) = FileLock::try_acquire(&renewal.join("activation.lock"))? else { return Ok(()) };
    let Some(_starting) = FileLock::try_acquire(&renewal.join("request.lock"))? else { return Ok(()) };
    let mut plan = store::read(root)?;
    if let Some(plan) = plan.as_mut()
        && plan.phase == Phase::Complete
    {
        anyhow::ensure!(
            store::digest(&store::installed(root))? == plan.candidate.build.digest,
            "completed renewal differs from stable program"
        );
        // Saved only when this changes it: a collection runs on every
        // maintenance pass, and a save is a temporary file, a rename and two
        // syncs.
        let installed = store::installed(root);
        if plan.candidate.path != installed || plan.previous.is_some() {
            plan.candidate.path = installed;
            plan.previous = None;
            store::save(root, plan)?;
        }
    }
    let images = renewal.join("images");
    for directory in entries(&images)? {
        let metadata = fs::symlink_metadata(&directory)?;
        if fastctx_platform::owned_directory::is_link(&metadata) || !metadata.is_dir() {
            continue;
        }
        let Some(name) = directory.file_name().and_then(|name| name.to_str()) else { continue };
        if name.len() != 64 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let path = directory.join(store::EXECUTABLE);
        if plan.as_ref().is_some_and(|plan| plan.candidate.path == path) {
            continue;
        }
        if path.try_exists()? {
            let Some(_unused) = image_usage::unused(&path)? else { continue };
            fs::remove_file(&path)?;
        }
        if entries(&directory)?.is_empty() {
            fs::remove_dir(directory)?;
        }
    }
    for path in entries(&renewal)? {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        let digest = name
            .strip_prefix("prepared-")
            .or_else(|| name.strip_prefix("coordinator-"))
            .and_then(|name| name.strip_suffix(".json"));
        if let Some(digest) = digest
            && digest.len() == 64
            && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            && plan.as_ref().is_none_or(|plan| plan.phase == Phase::Complete || plan.candidate.build.digest != digest)
            && let Some(identity) = store::read_json::<fastctx_types::ProcessIdentity>(&path)?
            && !fastctx_platform::is_alive(&identity)?
        {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn entries(path: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    match fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
