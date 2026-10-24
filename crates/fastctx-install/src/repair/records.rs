//! Retiring abandoned renewal metadata under the existing worker gate.

use super::Journal;
use crate::renewal as store;
use anyhow::{Context as _, ensure};
use fastctx_platform::FileLock;
use fastctx_types::{
    ProcessIdentity,
    renewal::{Build, Phase},
};
use std::path::Path;

pub fn check_version(root: &Path, offered: &Build) -> anyhow::Result<()> {
    let permitted = |current: &Build| -> anyhow::Result<()> {
        ensure!(
            current == offered || crate::program::newer(offered, current)?,
            "repair {} would replace newer kernel {}; no changes made",
            offered.version,
            current.version
        );
        Ok(())
    };
    let plan = store::read(root)?;
    if let Some(plan) = &plan {
        permitted(&plan.candidate.build)?;
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let world = fastctx_types::layout::WorldDir::under(root);
    let serving = runtime
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(30), fastctx_client::renewal::probe(&world)).await
        })
        .context("checking the serving node for repair exceeded 30 seconds")??;
    if let fastctx_client::renewal::Probe::Running(node) = &serving {
        permitted(&node.build)?;
    }
    let stable = store::installed(root);
    if stable.try_exists()? {
        match crate::program::inspect(&stable) {
            Ok(build) => permitted(&build)?,
            Err(error) => ensure!(
                plan.is_some() || matches!(serving, fastctx_client::renewal::Probe::Running(_)),
                "cannot establish the version of damaged {} without a plan or serving node: {error:#}",
                stable.display()
            ),
        }
    }
    Ok(())
}

pub fn clean(root: &Path, source: &Path, build: &Build, journal: &mut Journal) -> anyhow::Result<String> {
    let directory = store::directory(root);
    fastctx_platform::ensure_private_dir(&directory)?;
    let worker = directory.join("worker.lock");
    let _worker = FileLock::try_acquire(&worker)?.with_context(|| {
        format!("renewal still owns {}; repair does not terminate an active worker", worker.display())
    })?;
    check_version(root, build)?;
    let entry = directory.join("node-entry.lock");
    let _entry = match FileLock::try_acquire(&entry)? {
        Some(lock) => lock,
        None => store::superseded(root, &entry, source, true, |holder| {
            journal.action(format!(
                "end superseded handover coordinator pid {} ({:?}); worker gate is free",
                holder.pid, holder
            ))
        })?
        .with_context(|| format!("{} is held by a handover not proved superseded", entry.display()))?,
    };
    let plan = store::read(root)?;
    for item in std::fs::read_dir(&directory)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        let Some(digest) = name
            .strip_prefix("coordinator-")
            .or_else(|| name.strip_prefix("prepared-"))
            .and_then(|rest| rest.strip_suffix(".json"))
        else {
            continue;
        };
        // An ended handover's record, once its renewal is complete or for
        // another program, is left for the program collection to clear in its
        // course: a healthy machine keeps them, and they block nothing.
        if plan.as_ref().is_none_or(|plan| plan.phase == Phase::Complete || plan.candidate.build.digest != digest) {
            continue;
        }
        if let Some(identity) = store::read_json::<ProcessIdentity>(&item.path())?
            && !fastctx_platform::is_alive(&identity)?
        {
            delete(&item.path(), &format!("recorded process {} has ended", identity.pid), journal)?;
        }
    }
    let uninstalled = root.join("uninstalled.json");
    if uninstalled.try_exists()? {
        // The repair controller holds the same exclusion gate as uninstall.
        ensure!(
            store::installed(root).is_file(),
            "{} exists but no installed program proves this is an installation",
            uninstalled.display()
        );
        delete(&uninstalled, "explicit repair of an installed World", journal)?;
    }
    let attempt = root.join("updates/attempt.json");
    if let Some(value) = store::read_json::<serde_json::Value>(&attempt)?
        && value.get("version").and_then(|v| v.as_str()) == Some(&build.version)
    {
        delete(&attempt, "retrying this failed version by repair", journal)?;
    }
    let take = root.join("updates/take.json");
    if let Some(value) = store::read_json::<serde_json::Value>(&take)?
        && let Some(version) = value.get("version").and_then(|v| v.as_str())
        && fastctx_types::update::release(version) < fastctx_types::update::release(&build.version)
    {
        delete(&take, "application request predates the repair version", journal)?;
    }
    Ok("renewal owners checked; lock files retained".into())
}

pub fn delete(path: &Path, why: &str, journal: &mut Journal) -> anyhow::Result<()> {
    journal.action(format!("delete {}: {why}", path.display()))?;
    std::fs::remove_file(path).with_context(|| format!("deleting {}: {why}", path.display()))
}

/// A failed plan must not be returned unchanged by ordinary staging. The worker
/// and entry gates prove no publisher is using it before it is retired.
pub fn restage(root: &Path, source: &Path, journal: &mut Journal) -> anyhow::Result<store::Plan> {
    if let Some(plan) = store::read(root)?
        && plan.phase != fastctx_types::renewal::Phase::Complete
    {
        journal.action(format!(
            "replace abandoned renewal intent {} after checking its version",
            store::directory(root).join("plan.json").display()
        ))?;
    }
    store::stage_repair(root, source)
}
