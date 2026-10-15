//! Bringing a package here: from where its release published it, and when
//! that cannot be reached or there is no such place, through the network's
//! hub. Whatever way it came, it is kept only once its signature checks out.

use std::path::PathBuf;

use fastctx_net::link::Link;
use fastctx_types::update::{Manifest, Via};

use crate::Cache;

/// How far a package has come.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub via: Via,
    pub received: u64,
    pub total: Option<u64>,
}

/// Brings the package `target` of `manifest`'s release into `cache`, by
/// GitHub first and then `link`'s hub, and says where it is.
pub async fn obtain(
    cache: &Cache,
    manifest: &Manifest,
    target: &str,
    link: Option<&Link>,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<PathBuf, String> {
    let version = &manifest.version;
    let package = manifest.package(target).ok_or_else(|| format!("{version} has no package for {target}"))?;
    if let Some(path) = cache.get(version, target) {
        return Ok(path);
    }
    let incoming = cache.incoming(version, target)?;
    let mut failures = Vec::new();
    if let Some(url) = &package.url {
        let mut heard = |received, total| progress(Progress { via: Via::Github, received, total });
        match crate::download::from_url(url, &incoming, &mut heard).await {
            Ok(()) => match crate::verify::file(&incoming, &package.signature, version) {
                Ok(()) => return finish(cache, manifest, target, &incoming),
                Err(why) => {
                    let _ = std::fs::remove_file(&incoming);
                    failures.push(why);
                }
            },
            Err(why) => failures.push(why),
        }
    }
    match link {
        Some(link) if link.keeps_versions() => {
            // What came from elsewhere is not continued from another source.
            if !failures.is_empty() {
                let _ = std::fs::remove_file(&incoming);
            }
            let mut heard = |received, total| progress(Progress { via: Via::Network, received, total });
            match link.package(version, target, &incoming, &mut heard).await {
                Ok(()) => match crate::verify::file(&incoming, &package.signature, version) {
                    Ok(()) => return finish(cache, manifest, target, &incoming),
                    Err(why) => {
                        let _ = std::fs::remove_file(&incoming);
                        failures.push(format!("from the network: {why}"));
                    }
                },
                Err(why) => failures.push(format!("from the network: {why}")),
            }
        }
        Some(_) => failures.push("the network's hub does not hand out packages".to_string()),
        None if package.url.is_none() => failures.push("it is held only by machines of a network".to_string()),
        None => {}
    }
    Err(failures.join("; "))
}

fn finish(cache: &Cache, manifest: &Manifest, target: &str, incoming: &std::path::Path) -> Result<PathBuf, String> {
    cache.keep_manifest(manifest)?;
    cache.keep(&manifest.version, target, incoming)
}
