//! What GitHub publishes: every release that carries a manifest, and the
//! newest of each line with its manifest. The stable line takes only
//! releases not marked as test releases; the beta line takes all.

use fastctx_types::update::{Looked, MANIFEST, Manifest, REPOSITORY, Release as Listed};
use jiff::Timestamp;
use serde::Deserialize;

/// How many releases one page of the list holds.
const PAGE: usize = 100;
/// The most manifests of newer releases read in one look.
const NEWER: usize = 8;
/// The most pages read: the newest thousand releases.
const PAGES: usize = 10;

/// The list of releases is read from here unless `FASTCTX_UPDATE_RELEASES`
/// names another list of the same shape, as a drill's own server does; that
/// one is read whole, as one page.
fn releases_url(page: usize) -> Option<String> {
    match std::env::var("FASTCTX_UPDATE_RELEASES") {
        Ok(url) => (page == 1).then_some(url),
        Err(_) => Some(format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page={PAGE}&page={page}")),
    }
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
    published_at: Option<Timestamp>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Looks on GitHub now; with `running`, reads the manifests of the
/// releases newer than it too, a few at most.
pub async fn look(running: Option<&str>) -> Looked {
    let at = Some(Timestamp::now());
    match found().await {
        Ok((stable, beta, releases)) => {
            let newer = match running {
                Some(running) => newer(&releases, running).await,
                None => Vec::new(),
            };
            Looked { at, github: Some(true), why: None, stable, beta, releases, newer }
        }
        Err(Failure { reached, why }) => Looked {
            at,
            github: Some(reached),
            why: Some(why),
            stable: None,
            beta: None,
            releases: Vec::new(),
            newer: Vec::new(),
        },
    }
}

/// The manifests of the releases newer than `running`, the newest first: a
/// release whose manifest cannot be read now is left out, and read again at
/// the next look.
async fn newer(releases: &[Listed], running: &str) -> Vec<Manifest> {
    let floor = fastctx_types::update::release(running);
    let mut found = Vec::new();
    for release in
        releases.iter().filter(|release| fastctx_types::update::release(&release.version) > floor).take(NEWER)
    {
        if let Ok(manifest) = of(release).await {
            found.push(manifest);
        }
    }
    found
}

struct Failure {
    /// Whether GitHub answered at all.
    reached: bool,
    why: String,
}

/// Every release not a draft that carries a manifest, the newest first.
async fn listed() -> Result<Vec<Listed>, Failure> {
    let unreached = |why: String| Failure { reached: false, why };
    let mut all = Vec::new();
    for page in 1..=PAGES {
        let Some(url) = releases_url(page) else { break };
        let response = fastctx_net::web::get(&url, "application/vnd.github+json", 0).await.map_err(unreached)?;
        if response.status != 200 {
            return Err(Failure {
                reached: true,
                why: format!("GitHub answered {} for the releases", response.status),
            });
        }
        let body = response.bytes(LIMIT).await.map_err(unreached)?;
        let releases: Vec<Release> = serde_json::from_slice(&body)
            .map_err(|error| Failure { reached: true, why: format!("reading GitHub's releases: {error}") })?;
        let full = releases.len() == PAGE;
        all.extend(releases.into_iter().filter(|release| !release.draft).filter_map(|release| {
            let version = release.tag_name.strip_prefix('v')?;
            fastctx_types::update::release(version)?;
            let manifest = release.assets.iter().find(|asset| asset.name == MANIFEST)?;
            Some(Listed {
                version: version.to_string(),
                published: release.published_at,
                prerelease: release.prerelease,
                manifest: Some(manifest.browser_download_url.clone()),
            })
        }));
        if !full {
            break;
        }
    }
    all.sort_by(|a, b| fastctx_types::update::release(&b.version).cmp(&fastctx_types::update::release(&a.version)));
    Ok(all)
}

async fn found() -> Result<(Option<Manifest>, Option<Manifest>, Vec<Listed>), Failure> {
    let releases = listed().await?;
    let newest = |stable: bool| releases.iter().find(|release| !(stable && release.prerelease));
    let (stable, any) = (newest(true), newest(false));
    let stable_manifest = match stable {
        Some(release) => Some(of(release).await?),
        None => None,
    };
    let beta_manifest = match any {
        Some(release) if stable.is_some_and(|stable| stable.version == release.version) => None,
        Some(release) => Some(of(release).await?),
        None => None,
    };
    Ok((stable_manifest, beta_manifest, releases))
}

/// The manifest of `release`, as listed.
async fn of(release: &Listed) -> Result<Manifest, Failure> {
    let url = release
        .manifest
        .as_deref()
        .ok_or_else(|| Failure { reached: true, why: format!("the release {} lists no manifest", release.version) })?;
    manifest(url, &release.version).await
}

/// The manifest of the release `version` in a listing that has it.
pub async fn manifest_of(releases: &[Listed], version: &str) -> Result<Manifest, String> {
    let release = releases
        .iter()
        .find(|release| release.version == version)
        .ok_or_else(|| format!("GitHub lists no release {version} with a manifest"))?;
    of(release).await.map_err(|failure| failure.why)
}

async fn manifest(url: &str, version: &str) -> Result<Manifest, Failure> {
    let response =
        fastctx_net::web::get(url, "application/json", 0).await.map_err(|why| Failure { reached: false, why })?;
    if response.status != 200 {
        return Err(Failure { reached: true, why: format!("GitHub answered {} for {url}", response.status) });
    }
    let body = response.bytes(LIMIT).await.map_err(|why| Failure { reached: false, why })?;
    let manifest: Manifest = serde_json::from_slice(&body)
        .map_err(|error| Failure { reached: true, why: format!("reading the manifest of {version}: {error}") })?;
    if manifest.version != version {
        return Err(Failure {
            reached: true,
            why: format!("the release {version} published a manifest for {}", manifest.version),
        });
    }
    Ok(manifest)
}
