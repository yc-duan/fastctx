//! Bringing a package here. A machine of a network asks its hub first,
//! which holds the package or brings it in once for the whole network
//! (`world-network.md`, 中枢替网络存的东西); from the hub it comes, with
//! GitHub tried beside it when the hub's is slow (`crate::pace`), and from
//! GitHub alone when the hub cannot give it or there is none. Whatever way
//! it came, it is kept only once its signature checks out.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use fastctx_net::link::Link;
use fastctx_types::update::{Manifest, Via};

use crate::Cache;
use crate::pace::{Pace, SAMPLE, Verdict};

/// How far a package has come.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub via: Via,
    pub received: u64,
    pub total: Option<u64>,
}

/// A package brought here: where it is kept, and how fast GitHub gave it
/// when it came from there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brought {
    pub path: PathBuf,
    pub github_speed: Option<u64>,
}

/// Brings the package `target` of `manifest`'s release into `cache`, by
/// `link`'s hub first when it hands packages out, and says where it is.
pub async fn obtain(
    cache: &Cache,
    manifest: &Manifest,
    target: &str,
    link: Option<&Link>,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Brought, String> {
    let version = &manifest.version;
    let package = manifest.package(target).ok_or_else(|| format!("{version} has no package for {target}"))?;
    let held = |cache: &Cache| cache.get(version, target).map(|path| Brought { path, github_speed: None });
    if let Some(brought) = held(cache) {
        return Ok(brought);
    }
    let (url, signature) = (package.url.as_deref(), package.signature.as_str());
    let mut failures = Vec::new();
    let mut github_tried = false;
    match link {
        Some(link) if link.keeps_versions() => match link.ask_package(version, target).await {
            Ok(size) => {
                // This machine may be the one the hub asked to fetch it.
                if let Some(brought) = held(cache) {
                    return Ok(brought);
                }
                let sources = Sources { size, url, signature };
                match from_hub(cache, manifest, target, link, sources, progress, &mut github_tried).await {
                    Ok(brought) => return Ok(brought),
                    Err(why) => failures.push(why),
                }
            }
            Err(why) => failures.push(format!("from the network: {why}")),
        },
        Some(_) => failures.push("the network's hub does not hand out packages".to_string()),
        None if url.is_none() => failures.push("it is held only by machines of a network".to_string()),
        None => {}
    }
    if let Some(url) = url.filter(|_| !github_tried) {
        let incoming = cache.incoming(version, target)?;
        let mut heard = |received, total| progress(Progress { via: Via::Github, received, total });
        match from_github(url, &incoming, signature, version, &mut heard).await {
            Ok(speed) => return finish(cache, manifest, target, &incoming, speed),
            Err(why) => failures.push(why),
        }
    }
    Err(failures.join("; "))
}

/// What is known of the package the hub holds: its size, where GitHub has
/// it when it does, and its signature.
struct Sources<'a> {
    size: u64,
    url: Option<&'a str>,
    signature: &'a str,
}

/// The package of `size` bytes the hub holds, downloaded from it; GitHub,
/// when the release publishes it there, is tried beside the hub once the
/// hub's download is slow or fails, and whichever has the whole package
/// first, checked, is kept.
async fn from_hub(
    cache: &Cache,
    manifest: &Manifest,
    target: &str,
    link: &Link,
    sources: Sources<'_>,
    progress: &mut (dyn FnMut(Progress) + Send),
    github_tried: &mut bool,
) -> Result<Brought, String> {
    let Sources { size, url, signature } = sources;
    let version = &manifest.version;
    let hub_file = cache.incoming(version, target)?;
    let github_file = cache.attempt(version, target, 0)?;
    let _ = std::fs::remove_file(&github_file);
    // Both sources tell one progress: the one further on is shown.
    let shown = Mutex::new(Shown { progress, hub: 0, github: 0 });
    let hub_pace = Mutex::new(Pace::new(Instant::now()));
    let mut hub_heard = |received, total| {
        hub_pace.lock().expect("never poisoned").heard(Instant::now(), received);
        shown.lock().expect("never poisoned").tell(Via::Network, received, total);
    };
    let mut github_heard = |received, total| shown.lock().expect("never poisoned").tell(Via::Github, received, total);
    let from_the_hub = link.package(version, target, size, &hub_file, &mut hub_heard);
    // Nothing of GitHub's is asked until it is polled.
    let from_github_too = async {
        match url {
            Some(url) => from_github(url, &github_file, signature, version, &mut github_heard).await,
            None => Err("the release publishes it nowhere else".to_string()),
        }
    };
    tokio::pin!(from_the_hub, from_github_too);
    let mut failures = Vec::new();
    let (mut hub_going, mut github_going) = (true, false);
    let mut look = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            came = &mut from_the_hub, if hub_going => {
                hub_going = false;
                match came.and_then(|()| crate::verify::file(&hub_file, signature, version)) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&github_file);
                        return finish(cache, manifest, target, &hub_file, None);
                    }
                    Err(why) => {
                        let _ = std::fs::remove_file(&hub_file);
                        failures.push(format!("from the network: {why}"));
                    }
                }
            }
            came = &mut from_github_too, if github_going => {
                github_going = false;
                match came {
                    Ok(speed) => {
                        let _ = std::fs::remove_file(&hub_file);
                        return finish(cache, manifest, target, &github_file, speed);
                    }
                    Err(why) => failures.push(why),
                }
            }
            _ = look.tick() => {}
        }
        // GitHub joins once the hub's download fails or is slow.
        let slow = !hub_going
            || hub_pace.lock().expect("never poisoned").verdict(Instant::now(), Some(size), SAMPLE)
                != Verdict::Promising;
        if slow && url.is_some() && !*github_tried {
            *github_tried = true;
            github_going = true;
        }
        if !hub_going && !github_going {
            return Err(failures.join("; "));
        }
    }
}

/// The progress shown while two sources bring one package.
struct Shown<'a> {
    progress: &'a mut (dyn FnMut(Progress) + Send),
    hub: u64,
    github: u64,
}

impl Shown<'_> {
    fn tell(&mut self, via: Via, received: u64, total: Option<u64>) {
        match via {
            Via::Network => self.hub = received,
            Via::Github => self.github = received,
        }
        if received >= self.hub.max(self.github) {
            (self.progress)(Progress { via, received, total });
        }
    }
}

/// Downloads the package from GitHub into `file` and checks it against
/// `signature`: its speed.
async fn from_github(
    url: &str,
    file: &Path,
    signature: &str,
    version: &str,
    heard: &mut (dyn FnMut(u64, Option<u64>) + Send),
) -> Result<Option<u64>, String> {
    let speed = crate::download::patiently(url, file, heard).await?;
    crate::verify::file(file, signature, version).inspect_err(|_| {
        let _ = std::fs::remove_file(file);
    })?;
    Ok(speed)
}

fn finish(
    cache: &Cache,
    manifest: &Manifest,
    target: &str,
    incoming: &Path,
    github_speed: Option<u64>,
) -> Result<Brought, String> {
    cache.keep_manifest(manifest)?;
    Ok(Brought { path: cache.keep(&manifest.version, target, incoming)?, github_speed })
}
