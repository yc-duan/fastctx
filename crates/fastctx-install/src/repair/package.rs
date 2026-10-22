//! Package recovery without requiring a running node. A node may populate the
//! same cache through its network before starting the standalone repairer.

use anyhow::{Context as _, ensure};
use fastctx_types::update::{MANIFEST, Manifest};
use std::path::{Path, PathBuf};

pub fn cache(root: &Path) -> fastctx_update::Cache {
    fastctx_update::Cache::new(root.join("updates/packages"))
}

pub async fn manifest(root: &Path, version: &str) -> anyhow::Result<Manifest> {
    ensure!(fastctx_types::update::release(version).is_some(), "invalid repair version {version}");
    let path = cache(root).root().join(format!("v{version}")).join(MANIFEST);
    if let Some(manifest) = crate::renewal::read_json::<Manifest>(&path)? {
        ensure!(manifest.version == version, "manifest {} names another version", path.display());
        return Ok(manifest);
    }
    let looked = fastctx_update::github::look_after(None, &Default::default(), None).await;
    fastctx_update::github::manifest_of(&looked.releases, version)
        .await
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("finding repair manifest for {version}"))
}

pub async fn obtain(
    root: &Path,
    manifest: &Manifest,
    target: &str,
    journal: &mut super::Journal,
) -> anyhow::Result<PathBuf> {
    let cache = cache(root);
    let package =
        manifest.package(target).with_context(|| format!("{} has no package for {target}", manifest.version))?;
    let path = cache.path(&manifest.version, target).map_err(anyhow::Error::msg)?;
    if path.try_exists()? {
        match fastctx_update::verify::file(&path, &package.signature, &manifest.version) {
            Ok(()) => return Ok(path),
            Err(why) => {
                journal.action(format!("delete invalid package {}: {why}", path.display()))?;
                std::fs::remove_file(&path).with_context(|| format!("deleting invalid package {}", path.display()))?;
            }
        }
    }
    journal.action(format!("obtain and verify {}", path.display()))?;
    fastctx_update::obtain::obtain(&cache, manifest, target, &[], false, &mut |_| {})
        .await
        .map(|b| b.path)
        .map_err(|e| anyhow::anyhow!("obtaining {}: {e}", path.display()))
}
