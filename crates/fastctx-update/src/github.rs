//! Patient release checks, conditional listings and independently readable manifests.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use fastctx_types::update::{CheckFailure, Looked, MANIFEST, Manifest, REPOSITORY, ReachFailure, Release as Listed};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

const PAGE: usize = 100;
const NEWER: usize = 8;
const PAGES: usize = 10;
const LIMIT: usize = 4 << 20;
const CHECK_TIME: Duration = Duration::from_secs(120);

fn releases_url(page: usize) -> Option<String> {
    match std::env::var("FASTCTX_UPDATE_RELEASES") {
        Ok(url) => (page == 1).then_some(url),
        Err(_) => Some(format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page={PAGE}&page={page}")),
    }
}

#[derive(Clone, Deserialize, Serialize)]
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

#[derive(Clone, Deserialize, Serialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct Page {
    etag: Option<String>,
    releases: Vec<Release>,
}

type Pages = BTreeMap<String, Page>;

#[derive(Clone)]
struct Failure {
    reached: bool,
    kind: CheckFailure,
    why: String,
}

impl From<fastctx_net::web::Failure> for Failure {
    fn from(error: fastctx_net::web::Failure) -> Self {
        Self { reached: false, kind: CheckFailure::Unreachable { reason: error.reason }, why: error.why }
    }
}

/// Performs one bounded check, preserving earlier discoveries on failure.
/// `cache` keeps ETags and their corresponding pages together across restarts.
pub async fn look_after(running: Option<&str>, previous: &Looked, cache: Option<&Path>) -> Looked {
    let started = Instant::now();
    let mut pages: Pages = cache
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut last = None;
    let mut reached_at = previous.github_at;
    let work = async {
        for delay in [0, 3, 12, 45] {
            if delay > 0 {
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }
            match listed(&mut pages).await {
                Ok(releases) => {
                    reached_at = Some(Timestamp::now());
                    return Ok(releases);
                }
                Err(failure) => {
                    if failure.reached {
                        reached_at = Some(Timestamp::now());
                    }
                    let retry = matches!(
                        failure.kind,
                        CheckFailure::Unreachable { .. } | CheckFailure::Http { status: 500..=599 }
                    );
                    last = Some(failure.clone());
                    if !retry {
                        return Err(failure);
                    }
                }
            }
        }
        Err(last.clone().expect("each attempt failed"))
    };
    let result = match tokio::time::timeout(CHECK_TIME, work).await {
        Ok(result) => result,
        Err(_) => Err(last.unwrap_or(Failure {
            reached: false,
            kind: CheckFailure::Unreachable { reason: ReachFailure::Timeout },
            why: "checking GitHub took more than two minutes".into(),
        })),
    };
    if let Some(path) = cache {
        let saved = (|| -> Result<(), String> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let bytes = serde_json::to_vec(&pages).map_err(|e| e.to_string())?;
            fastctx_platform::publish(path, &bytes, fastctx_platform::Permissions::Private).map_err(|e| e.to_string())
        })();
        if let Err(why) = saved {
            eprintln!("keeping GitHub's conditional listing: {why}");
        }
    }
    let mut looked = previous.clone();
    looked.checking = false;
    looked.at = Some(Timestamp::now());
    looked.github_at = reached_at;
    match result {
        Ok(releases) => {
            looked.github = Some(true);
            looked.why = None;
            looked.failure = None;
            looked.failures = 0;
            // Each manifest stands alone. Failure of one never discards the list
            // or the other line. A later check tries missing manifests again.
            let mut wanted: Vec<&Listed> = releases
                .iter()
                .filter(|r| {
                    running
                        .is_some_and(|v| fastctx_types::update::release(&r.version) > fastctx_types::update::release(v))
                })
                .take(NEWER)
                .collect();
            wanted.extend(releases.first());
            wanted.extend(releases.iter().find(|r| !r.prerelease));
            wanted.sort_by(|a, b| a.version.cmp(&b.version));
            wanted.dedup_by(|a, b| a.version == b.version);
            // What an earlier check read stands while GitHub still lists it; a
            // release taken down goes with it.
            let mut manifests: BTreeMap<_, _> = previous
                .newer
                .iter()
                .chain(previous.stable.iter())
                .chain(previous.beta.iter())
                .filter(|manifest| releases.iter().any(|release| release.version == manifest.version))
                .map(|manifest| (manifest.version.clone(), manifest.clone()))
                .collect();
            let mut failure = None;
            // Bound manifest work separately: missing assets must not turn an
            // otherwise successful check into minutes per listed release.
            let read = async {
                for release in wanted {
                    match of(release).await {
                        Ok(manifest) => {
                            manifests.insert(manifest.version.clone(), manifest);
                        }
                        Err(why) => {
                            if failure.is_none() || matches!(why.kind, CheckFailure::RateLimited { .. }) {
                                failure = Some(why);
                            }
                        }
                    }
                }
            };
            if tokio::time::timeout(CHECK_TIME.saturating_sub(started.elapsed()).min(Duration::from_secs(60)), read)
                .await
                .is_err()
                && failure.is_none()
            {
                failure = Some(Failure {
                    reached: true,
                    kind: CheckFailure::Unreachable { reason: ReachFailure::Timeout },
                    why: "reading GitHub's release manifests timed out".into(),
                });
            }
            let mut manifests: Vec<_> = manifests.into_values().collect();
            manifests.sort_by_key(|m| std::cmp::Reverse(m.semver()));
            looked.stable = manifests.iter().find(|m| m.semver().is_some_and(|v| v.pre.is_empty())).cloned();
            // The beta line is a test release newer than every release out.
            looked.beta = manifests.first().filter(|m| m.semver().is_some_and(|v| !v.pre.is_empty())).cloned();
            manifests.truncate(NEWER);
            looked.newer = manifests;
            looked.releases = releases;
            if let Some(failure) = failure {
                looked.why = Some(failure.why);
                looked.failure = Some(failure.kind);
                looked.failures = previous.failures.saturating_add(1);
            }
        }
        Err(failure) => {
            looked.github = Some(failure.reached);
            looked.why = Some(failure.why);
            looked.failure = Some(failure.kind);
            looked.failures = previous.failures.saturating_add(1);
        }
    }
    let seconds = if looked.failure.is_some() {
        (900u64.saturating_mul(1 << looked.failures.saturating_sub(1).min(5))).min(21600)
    } else {
        21600
    };
    let now = Timestamp::now();
    let jitter = (now.as_nanosecond().unsigned_abs() as u64 ^ u64::from(std::process::id())) % (seconds / 10 + 1);
    let mut next = now.checked_add(Duration::from_secs(seconds + jitter)).ok();
    if let Some(CheckFailure::RateLimited { until: Some(until) }) = &looked.failure {
        next = Some(next.map_or(*until, |next| next.max(*until)));
    }
    looked.next = next;
    looked
}

/// Time until the next check; old records use the six-hour cadence.
pub fn wait(looked: &Looked) -> Duration {
    let next = looked.next.or_else(|| {
        looked
            .at
            .and_then(|at| at.checked_add(Duration::from_secs(if looked.why.is_some() { 900 } else { 21600 })).ok())
    });
    next.filter(|next| *next > Timestamp::now())
        .map_or(Duration::ZERO, |next| next.duration_since(Timestamp::now()).unsigned_abs())
}

fn answered(response: &fastctx_net::web::Response) -> Failure {
    let header = |name: &str| response.headers.get(name).and_then(|v| v.to_str().ok());
    let now = Timestamp::now();
    let retry = header("retry-after").and_then(|s| {
        s.parse::<u64>().ok().and_then(|s| now.checked_add(Duration::from_secs(s)).ok()).or_else(|| {
            jiff::civil::DateTime::strptime("%a, %d %b %Y %H:%M:%S GMT", s)
                .ok()
                .and_then(|date| date.to_zoned(jiff::tz::TimeZone::UTC).ok())
                .map(|z| z.timestamp())
        })
    });
    let reset =
        header("x-ratelimit-reset").and_then(|s| s.parse::<i64>().ok()).and_then(|s| Timestamp::from_second(s).ok());
    let limited = response.status == 429
        || (response.status == 403
            && (retry.is_some() || reset.is_some() || header("x-ratelimit-remaining") == Some("0")));
    Failure {
        reached: true,
        kind: if limited {
            CheckFailure::RateLimited { until: retry.into_iter().chain(reset).max() }
        } else {
            CheckFailure::Http { status: response.status }
        },
        why: format!("GitHub answered {} for {}", response.status, response.url),
    }
}

async fn listed(pages: &mut Pages) -> Result<Vec<Listed>, Failure> {
    let mut all = Vec::new();
    for page in 1..=PAGES {
        let Some(url) = releases_url(page) else { break };
        let previous = pages.get(&url);
        let response = fastctx_net::web::get_conditional(
            &url,
            "application/vnd.github+json",
            0,
            previous.and_then(|p| p.etag.as_deref()),
        )
        .await?;
        let releases = if response.status == 304 {
            previous
                .ok_or_else(|| Failure {
                    reached: true,
                    kind: CheckFailure::Unreadable,
                    why: "GitHub returned 304 without a cached listing".into(),
                })?
                .releases
                .clone()
        } else {
            if response.status != 200 {
                return Err(answered(&response));
            }
            let etag = response.headers.get("etag").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let body = response.bytes_checked(LIMIT).await.map_err(|e| {
                let mut failure = Failure::from(e);
                failure.reached = true;
                failure
            })?;
            let releases: Vec<Release> = serde_json::from_slice(&body).map_err(|e| Failure {
                reached: true,
                kind: CheckFailure::Unreadable,
                why: format!("reading GitHub's releases: {e}"),
            })?;
            pages.insert(url, Page { etag, releases: releases.clone() });
            releases
        };
        let full = releases.len() == PAGE;
        all.extend(releases.into_iter().filter(|r| !r.draft).filter_map(|r| {
            let version = r.tag_name.strip_prefix('v')?;
            fastctx_types::update::release(version)?;
            let manifest = r.assets.iter().find(|a| a.name == MANIFEST)?;
            Some(Listed {
                version: version.into(),
                published: r.published_at,
                prerelease: r.prerelease,
                manifest: Some(manifest.browser_download_url.clone()),
            })
        }));
        if !full {
            break;
        }
    }
    all.sort_by_key(|r| std::cmp::Reverse(fastctx_types::update::release(&r.version)));
    Ok(all)
}

async fn of(release: &Listed) -> Result<Manifest, Failure> {
    let url = release.manifest.as_deref().ok_or_else(|| Failure {
        reached: true,
        kind: CheckFailure::Unreadable,
        why: format!("the release {} lists no manifest", release.version),
    })?;
    let response = fastctx_net::web::get_conditional(url, "application/json", 0, None).await?;
    if response.status != 200 {
        return Err(answered(&response));
    }
    let body = response.bytes_checked(LIMIT).await.map_err(|e| {
        let mut failure = Failure::from(e);
        failure.reached = true;
        failure
    })?;
    let manifest: Manifest = serde_json::from_slice(&body).map_err(|e| Failure {
        reached: true,
        kind: CheckFailure::Unreadable,
        why: format!("reading the manifest of {}: {e}", release.version),
    })?;
    if manifest.version != release.version {
        return Err(Failure {
            reached: true,
            kind: CheckFailure::Unreadable,
            why: format!("the release {} published a manifest for {}", release.version, manifest.version),
        });
    }
    Ok(manifest)
}

pub async fn manifest_of(releases: &[Listed], version: &str) -> Result<Manifest, String> {
    let release = releases
        .iter()
        .find(|r| r.version == version)
        .ok_or_else(|| format!("GitHub lists no release {version} with a manifest"))?;
    tokio::time::timeout(Duration::from_secs(50), of(release))
        .await
        .map_err(|_| format!("the manifest of {version} did not arrive within 50 seconds"))?
        .map_err(|failure| failure.why)
}
