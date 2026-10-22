//! Every repair starts at the newest version already known on this machine.

use anyhow::{Context as _, ensure};
use std::path::Path;

pub fn select(root: &Path, running: &str, explicit: Option<&str>, serving: Option<&str>) -> anyhow::Result<String> {
    let mut versions = vec![running.to_owned()];
    versions.extend(explicit.map(str::to_owned));
    versions.extend(serving.map(str::to_owned));
    if let Some(plan) = crate::renewal::read(root)? {
        versions.push(plan.candidate.build.version);
    }
    for name in ["attempt.json", "take.json"] {
        let path = root.join("updates").join(name);
        if let Some(value) = crate::renewal::read_json::<serde_json::Value>(&path)
            .with_context(|| format!("reading repair target {}", path.display()))?
            && let Some(version) = value.get("version").and_then(|v| v.as_str())
        {
            versions.push(version.into());
        }
    }
    if let Some(app) = crate::renewal::read_json::<super::application::Registration>(&root.join("updates/app.json"))? {
        if let Ok(build) = super::application::program_build(root, &app) {
            versions.push(build.version);
        }
        versions.push(app.app.version);
    }
    if let Ok(build) = crate::program::inspect(&crate::renewal::installed(root)) {
        versions.push(build.version);
    }
    for version in &versions {
        ensure!(
            fastctx_types::update::release(version).is_some(),
            "invalid repair target {version} under {}",
            root.display()
        );
    }
    versions.into_iter().max_by_key(|v| fastctx_types::update::release(v)).context("repair has no target version")
}
