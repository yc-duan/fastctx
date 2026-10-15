//! What GitHub publishes: the newest release on each channel that carries a
//! manifest. The stable channel takes only releases not marked as test
//! releases; the beta channel takes all.

use fastctx_types::update::{Looked, MANIFEST, Manifest, REPOSITORY};
use jiff::Timestamp;
use serde::Deserialize;

/// The list of releases is read from here unless `FASTCTX_UPDATE_RELEASES`
/// names another list of the same shape, as a drill's own server does.
fn releases_url() -> String {
    std::env::var("FASTCTX_UPDATE_RELEASES")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=30"))
}

/// The most a list of releases or a manifest may be.
const LIMIT: usize = 4 << 20;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Looks on GitHub now.
pub async fn look() -> Looked {
    let at = Some(Timestamp::now());
    match found().await {
        Ok((stable, beta)) => Looked { at, github: Some(true), why: None, stable, beta },
        Err(Failure { reached, why }) => Looked { at, github: Some(reached), why: Some(why), stable: None, beta: None },
    }
}

struct Failure {
    /// Whether GitHub answered at all.
    reached: bool,
    why: String,
}

async fn found() -> Result<(Option<Manifest>, Option<Manifest>), Failure> {
    let url = releases_url();
    let unreached = |why: String| Failure { reached: false, why };
    let response = fastctx_net::web::get(&url, "application/vnd.github+json", 0).await.map_err(unreached)?;
    if response.status != 200 {
        return Err(Failure { reached: true, why: format!("GitHub answered {} for the releases", response.status) });
    }
    let body = response.bytes(LIMIT).await.map_err(unreached)?;
    let releases: Vec<Release> = serde_json::from_slice(&body)
        .map_err(|error| Failure { reached: true, why: format!("reading GitHub's releases: {error}") })?;
    let with_manifest = |release: &Release| {
        let version = release.tag_name.strip_prefix('v')?;
        let version = fastctx_types::update::release(version)?;
        let asset = release.assets.iter().find(|asset| asset.name == MANIFEST)?;
        Some((version, asset.browser_download_url.clone()))
    };
    let newest = |stable: bool| {
        releases
            .iter()
            .filter(|release| !(release.draft || stable && release.prerelease))
            .filter_map(with_manifest)
            .max_by(|a, b| a.0.cmp(&b.0))
    };
    let (stable, any) = (newest(true), newest(false));
    let stable_manifest = match &stable {
        Some((version, url)) => Some(manifest(url, version).await?),
        None => None,
    };
    let beta_manifest = match any {
        Some((version, _)) if stable.as_ref().is_some_and(|(stable, _)| *stable == version) => None,
        Some((version, url)) => Some(manifest(&url, &version).await?),
        None => None,
    };
    Ok((stable_manifest, beta_manifest))
}

async fn manifest(url: &str, version: &semver::Version) -> Result<Manifest, Failure> {
    let response =
        fastctx_net::web::get(url, "application/json", 0).await.map_err(|why| Failure { reached: false, why })?;
    if response.status != 200 {
        return Err(Failure { reached: true, why: format!("GitHub answered {} for {url}", response.status) });
    }
    let body = response.bytes(LIMIT).await.map_err(|why| Failure { reached: false, why })?;
    let manifest: Manifest = serde_json::from_slice(&body)
        .map_err(|error| Failure { reached: true, why: format!("reading the manifest of {version}: {error}") })?;
    if manifest.semver().as_ref() != Some(version) {
        return Err(Failure {
            reached: true,
            why: format!("the release {version} published a manifest for {}", manifest.version),
        });
    }
    Ok(manifest)
}
