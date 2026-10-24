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

/// A checked package of a release, and whether it had to be brought: a
/// cache without it is not broken, so bringing it is no repair of its own.
pub struct Obtained {
    pub path: PathBuf,
    pub brought: bool,
}

pub async fn obtain(
    root: &Path,
    manifest: &Manifest,
    target: &str,
    journal: &mut super::Journal,
) -> anyhow::Result<Obtained> {
    let cache = cache(root);
    let package =
        manifest.package(target).with_context(|| format!("{} has no package for {target}", manifest.version))?;
    let path = cache.path(&manifest.version, target).map_err(anyhow::Error::msg)?;
    clear_incomplete(root, &cache, &manifest.version, target, journal)?;
    if path.try_exists()? {
        match fastctx_update::verify::file(&path, &package.signature, &manifest.version) {
            Ok(()) => return Ok(Obtained { path, brought: false }),
            Err(why) => {
                journal.action(format!("delete invalid package {}: {why}", path.display()))?;
                std::fs::remove_file(&path).with_context(|| format!("deleting invalid package {}", path.display()))?;
            }
        }
    }
    // A live node owns its numeric partial names and may clear them on startup.
    // Standalone acquisition must not enter that inventory until it is complete.
    let temporary = tempfile::Builder::new()
        .prefix("repair-acquisition-")
        .tempdir_in(root.join("updates"))
        .with_context(|| format!("preparing isolated acquisition under {}", root.display()))?;
    let isolated = fastctx_update::Cache::new(temporary.path());
    let brought = fastctx_update::obtain::obtain(&isolated, manifest, target, &[], false, &mut |_| {})
        .await
        .map_err(|e| anyhow::anyhow!("obtaining {}: {e}", path.display()))?;
    cache.keep_manifest(manifest).map_err(anyhow::Error::msg)?;
    let mut input = std::fs::File::open(&brought.path)
        .with_context(|| format!("reading verified package {}", brought.path.display()))?;
    let mut output = fastctx_platform::atomic::Staged::create(&path, Some(0o600))
        .with_context(|| format!("staging verified package {}", path.display()))?;
    use std::io::Read as _;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count =
            input.read(&mut buffer).with_context(|| format!("reading verified package {}", brought.path.display()))?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count]).with_context(|| format!("writing verified package {}", path.display()))?;
    }
    output.commit().with_context(|| format!("publishing verified package {}", path.display()))?;
    drop(input);
    temporary.close().context("removing isolated package acquisition")?;
    Ok(Obtained { path, brought: true })
}

fn clear_incomplete(
    root: &Path,
    cache: &fastctx_update::Cache,
    version: &str,
    target: &str,
    journal: &mut super::Journal,
) -> anyhow::Result<()> {
    let paths = cache.incomplete(version, target).map_err(anyhow::Error::msg)?;
    if paths.is_empty() {
        return Ok(());
    }
    let world = fastctx_types::layout::WorldDir::under(root);
    fastctx_platform::ensure_private_dir(&crate::renewal::directory(root))?;
    fastctx_platform::ensure_private_dir(world.path())?;
    let Some(_entry) =
        fastctx_platform::FileLock::try_acquire(&crate::renewal::directory(root).join("node-entry.lock"))?
    else {
        return Ok(());
    };
    let Some(_node) = fastctx_platform::FileLock::try_acquire(&world.node_lock())? else { return Ok(()) };
    for path in paths {
        journal
            .action(format!("delete stale package partial {}: node is absent and no node can enter", path.display()))?;
        std::fs::remove_file(&path).with_context(|| format!("deleting stale package {}", path.display()))?;
    }
    Ok(())
}
