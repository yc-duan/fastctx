//! The sources of one device's package acquisition, timed from its request.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fastctx_net::link::Link;
use fastctx_types::update::{Manifest, PackageProgress, Via};
use tokio::sync::watch;

use crate::Cache;
use crate::pace::{Pace, Verdict};
use crate::pool::{Local, Log, Request};

/// How far a package has come, including preparation at its hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub via: Via,
    pub received: u64,
    pub total: Option<u64>,
}

/// A verified package and the speed of its local GitHub download, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brought {
    pub path: PathBuf,
    pub github_speed: Option<u64>,
}

/// Obtains a package with a process-wide owner for each destination path.
/// Nodes use their own `Pool` so helpers and local callers share that owner.
pub async fn obtain(
    cache: &Cache,
    manifest: &Manifest,
    target: &str,
    link: Option<&Link>,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Brought, String> {
    static POOL: std::sync::OnceLock<crate::pool::Pool> = std::sync::OnceLock::new();
    let logger = link.cloned();
    let log: Log = std::sync::Arc::new(move |message| {
        if let Some(link) = &logger {
            link.log_package(message);
        } else {
            eprintln!("{message}");
        }
    });
    POOL.get_or_init(Default::default).obtain(cache, manifest, target, link, progress, log).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    cache: &Cache,
    request: &Request,
    hub_file: &Path,
    local_file: &Path,
    mut local: watch::Receiver<Local>,
    progress: &mut (dyn FnMut(Progress) + Send),
    log: &Log,
) -> Result<Brought, String> {
    let package = &request.package;
    let version = &request.version;
    let target = &request.target;
    let started = Instant::now();
    let hub_pace = Mutex::new(Pace::new(started));
    let hub_total = Mutex::new(package.size);
    // Once the hub holds the package, its handing waits its turn among the
    // machines it hands packages to at once: that wait is no slowness, and
    // the hub's pace counts from the first byte it hands.
    let hub_queued = AtomicBool::new(false);
    let shown = Mutex::new(Shown { progress });
    let mut hub_heard = |received, total| {
        let mut pace = hub_pace.lock().expect("never poisoned");
        if hub_queued.swap(false, Ordering::Relaxed) {
            *pace = Pace::new(Instant::now());
        }
        pace.heard(Instant::now(), received);
        drop(pace);
        *hub_total.lock().expect("never poisoned") = total;
        shown.lock().expect("never poisoned").tell(Via::Network, received, total);
    };
    let mut github_heard = |received, total| shown.lock().expect("never poisoned").tell(Via::Github, received, total);
    let hub = async {
        let link = request.link.as_ref().ok_or("no hub supplies packages")?;
        let ready = || hub_queued.store(true, Ordering::Relaxed);
        from_hub(link, version, target, package.size, hub_file, &mut hub_heard, &ready).await?;
        verified(hub_file, &package.signature, version).await?;
        Ok::<_, String>(())
    };
    let local_source = local.clone();
    let github = async {
        let source = local_source.borrow().clone();
        let speed = if let Some(carried) = source.carried {
            tokio::fs::copy(&carried, local_file).await.map_err(|error| format!("{}: {error}", carried.display()))?;
            None
        } else {
            let url = package.url.as_deref().ok_or("the release publishes it nowhere else")?;
            crate::download::patiently(url, local_file, &mut github_heard, None).await?
        };
        verified(local_file, &package.signature, version).await?;
        Ok::<_, String>(speed)
    };
    tokio::pin!(hub, github);
    let (mut hub_going, mut github_going) = (request.link.is_some(), false);
    let mut github_tried = false;
    let mut failures = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    let deadline = tokio::time::sleep(Duration::from_secs(45 * 60));
    tokio::pin!(deadline);
    loop {
        let verdict = if hub_queued.load(Ordering::Relaxed) {
            Verdict::Promising
        } else {
            hub_pace.lock().expect("never poisoned").verdict(Instant::now(), *hub_total.lock().expect("never poisoned"))
        };
        if !github_tried && (!hub_going || local.borrow().requested || verdict != Verdict::Promising) {
            github_tried = true;
            github_going = true;
            log(format!(
                "bringing {target} of {version}: starting the local source; hub active={hub_going}, pace={verdict:?}, requested as helper={}",
                local.borrow().requested
            ));
        }
        tokio::select! {
            result = &mut hub, if hub_going => {
                hub_going = false;
                match result.and_then(|()| cache.keep(version, target, hub_file)) {
                    Ok(path) => return Ok(Brought { path, github_speed: None }),
                    Err(why) => {
                        log(format!("bringing {target} of {version} from the network: {why}"));
                        failures.push(format!("from the network: {why}"));
                    }
                }
            }
            result = &mut github, if github_going => {
                github_going = false;
                match result.and_then(|speed| cache.keep(version, target, local_file).map(|path| Brought { path, github_speed: speed })) {
                    Ok(brought) => return Ok(brought),
                    Err(why) => {
                        log(format!("bringing {target} of {version} from the local source: {why}"));
                        failures.push(format!("from the local source: {why}"));
                    }
                }
            }
            _ = local.changed(), if !github_tried => {}
            _ = tick.tick() => {}
            () = &mut deadline => return Err(format!("bringing {target} of {version} took more than 45 minutes; {}", failures.join("; "))),
        }
        if !hub_going && github_tried && !github_going {
            return Err(failures.join("; "));
        }
    }
}

/// `verify::file` on a blocking thread: reading a whole package to check
/// its signature holds no async worker meanwhile.
async fn verified(path: &Path, signature: &str, version: &str) -> Result<(), String> {
    let (path, signature, version) = (path.to_path_buf(), signature.to_string(), version.to_string());
    tokio::task::spawn_blocking(move || crate::verify::file(&path, &signature, &version))
        .await
        .map_err(|error| format!("checking the package's signature: {error}"))?
}

/// The package from the hub: asked for, prepared there when it holds none
/// yet, then handed. `ready` hears that the hub holds it and its handing
/// is next, after a wait in the hub's queue.
async fn from_hub(
    link: &Link,
    version: &str,
    target: &str,
    expected: Option<u64>,
    file: &Path,
    progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ready: &(dyn Fn() + Send + Sync),
) -> Result<(), String> {
    let (sender, mut preparation) = watch::channel(PackageProgress::default());
    let ask = link.ask_package_progress(version, target, Some(sender));
    tokio::pin!(ask);
    let mut reporting = true;
    let ready_now = loop {
        tokio::select! {
            result = &mut ask => break result?,
            changed = preparation.changed(), if reporting => {
                if changed.is_ok() {
                    let heard = *preparation.borrow_and_update();
                    // Preparation and the final transfer are consecutive work.
                    progress(heard.received, heard.total.map(|total| total.saturating_add(expected.unwrap_or(total))));
                } else { reporting = false; }
            }
        }
    };
    let size = ready_now.size;
    let prepared = preparation.borrow().received;
    ready();
    let mut heard = |received, _| progress(prepared + received, Some(prepared + size));
    let result = link.package(version, target, size, file, &mut heard).await;
    drop(ready_now);
    result
}

struct Shown<'a> {
    progress: &'a mut (dyn FnMut(Progress) + Send),
}

impl Shown<'_> {
    fn tell(&mut self, via: Via, received: u64, total: Option<u64>) {
        (self.progress)(Progress { via, received, total });
    }
}
