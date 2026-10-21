//! The sources of one device's package acquisition, timed from its request.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use fastctx_net::link::Link;
use fastctx_types::update::{FailureReason, Manifest, PackageProgress, Problem, SourceFailure, Via};
use tokio::sync::watch;

use crate::Cache;
use crate::pool::{Local, Log, Request};

/// Bytes delivered here by a source; subscribers see the leading active source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub via: Via,
    pub received: u64,
    pub total: Option<u64>,
}

/// Source progress and terminal failure are distinct owner events.
pub(crate) enum SourceEvent {
    Progress(Progress),
    Failed(Via, Problem),
}

/// A verified package and the speed of its local GitHub download, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brought {
    pub path: PathBuf,
    pub github_speed: Option<u64>,
    pub github_at: Option<jiff::Timestamp>,
}

/// Obtains a package with a process-wide owner for each destination path.
/// Nodes use their own `Pool` so helpers and local callers share that owner.
pub async fn obtain(
    cache: &Cache,
    manifest: &Manifest,
    target: &str,
    links: &[&Link],
    progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Brought, Problem> {
    static POOL: std::sync::OnceLock<crate::pool::Pool> = std::sync::OnceLock::new();
    let logger = links.first().map(|link| (*link).clone());
    let log: Log = std::sync::Arc::new(move |message| {
        if let Some(link) = &logger {
            link.log_package(message);
        } else {
            eprintln!("{message}");
        }
    });
    POOL.get_or_init(Default::default).obtain(cache, manifest, target, links, None, progress, log).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    cache: &Cache,
    request: &Request,
    hub_file: &Path,
    local_file: &Path,
    mut local: watch::Receiver<Local>,
    progress: &mut (dyn FnMut(SourceEvent) + Send),
    log: &Log,
) -> Result<Brought, Problem> {
    let package = &request.package;
    let version = &request.version;
    let target = &request.target;
    let shown = Mutex::new(Shown { progress });
    let mut hub_heard = |received, total| {
        shown.lock().expect("never poisoned").tell(Via::Network, received, total);
    };
    let mut github_heard = |received, total: Option<u64>| {
        let total = total.or(package.size);
        shown.lock().expect("never poisoned").tell(Via::Github, received, total);
    };
    let hub = async {
        let mut failures = Vec::new();
        for link in &request.links {
            let result = async {
                if !matches!(link.status(), fastctx_net::link::Status::Online { .. }) {
                    return Err(Problem::from("not connected to the hub"));
                }
                hub_heard(0, package.size);
                from_hub(link, version, target, package.size, hub_file, &mut hub_heard, &|| {}).await?;
                verified(hub_file, &package.signature, version).await
            }
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(why) => {
                    log(format!("bringing {target} of {version} through {}: {why}", link.hub_name()));
                    failures.push(format!("{}: {why}", link.hub_name()));
                    match tokio::fs::remove_file(hub_file).await {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(Problem::new(FailureReason::Disk, error.to_string())),
                    }
                }
            }
        }
        Err(Problem::new(FailureReason::Other, failures.join("; ")))
    };
    let local_source = local.clone();
    let github = async {
        let source = local_source.borrow().clone();
        let downloaded = source.carried.is_none();
        let speed = if let Some(carried) = source.carried {
            tokio::fs::copy(&carried, local_file)
                .await
                .map_err(|error| Problem::new(FailureReason::Disk, format!("{}: {error}", carried.display())))?;
            None
        } else {
            let url = package.url.as_deref().ok_or("the release publishes it nowhere else")?;
            crate::download::patiently_checked(url, local_file, &mut github_heard, None).await?
        };
        verified(local_file, &package.signature, version).await?;
        Ok::<_, Problem>((speed, downloaded.then(jiff::Timestamp::now)))
    };
    tokio::pin!(hub, github);
    let mut hub_going = !request.links.is_empty();
    let mut github_going = !hub_going || local.borrow().requested || local.borrow().carried.is_some();
    let mut github_tried = github_going;
    let mut failures = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(45 * 60));
    tokio::pin!(deadline);
    loop {
        if !github_tried && (!hub_going || local.borrow().requested) {
            // A hub can ask this device to supply the very package it is waiting
            // for (2026-10-21). That helper must use the independent local source.
            github_going = true;
            github_tried = true;
        }
        tokio::select! {
            result = &mut hub, if hub_going => {
                hub_going = false;
                match result.and_then(|()| cache.keep(version, target, hub_file).map_err(|e| Problem::new(FailureReason::Disk, e))) {
                    Ok(path) => return Ok(Brought { path, github_speed: None, github_at: None }),
                    Err(why) => {
                        log(format!("bringing {target} of {version} from the network: {why}"));
                        shown.lock().expect("never poisoned").failed(Via::Network, why.clone());
                        failures.push((Via::Network, why));
                    }
                }
            }
            result = &mut github, if github_going => {
                github_going = false;
                match result.and_then(|(speed, github_at)| cache.keep(version, target, local_file).map_err(|e| Problem::new(FailureReason::Disk, e)).map(|path| Brought { path, github_speed: speed, github_at })) {
                    Ok(brought) => return Ok(brought),
                    Err(why) => {
                        log(format!("bringing {target} of {version} from the local source: {why}"));
                        shown.lock().expect("never poisoned").failed(Via::Github, why.clone());
                        failures.push((Via::Github, why));
                    }
                }
            }
            _ = local.changed(), if !github_tried => {}
            () = &mut deadline => return Err(Problem::new(FailureReason::Sources { sources: failures.iter().map(|(via,p)| SourceFailure { via: *via, why: p.why.clone() }).collect() }, format!("bringing {target} of {version} took more than 45 minutes"))),
        }
        if !hub_going && !github_going && github_tried {
            let reason = if failures.iter().any(|(_, p)| p.reason == FailureReason::Signature) {
                FailureReason::Signature
            } else if failures.iter().any(|(_, p)| p.reason == FailureReason::Disk) {
                FailureReason::Disk
            } else {
                FailureReason::Sources {
                    sources: failures.iter().map(|(via, p)| SourceFailure { via: *via, why: p.why.clone() }).collect(),
                }
            };
            let why = failures.iter().map(|(via, p)| format!("{via:?}: {}", p.why)).collect::<Vec<_>>().join("; ");
            return Err(Problem::new(reason, why));
        }
    }
}

/// `verify::file` on a blocking thread: reading a whole package to check
/// its signature holds no async worker meanwhile.
async fn verified(path: &Path, signature: &str, version: &str) -> Result<(), Problem> {
    let (path, signature, version) = (path.to_path_buf(), signature.to_string(), version.to_string());
    tokio::task::spawn_blocking(move || crate::verify::file(&path, &signature, &version))
        .await
        .map_err(|error| Problem::new(FailureReason::Signature, format!("checking the package's signature: {error}")))?
        .map_err(|why| Problem::new(FailureReason::Signature, why))
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
) -> Result<(), Problem> {
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
                    progress(0, expected.or(heard.total));
                } else { reporting = false; }
            }
        }
    };
    let size = ready_now.size;
    ready();
    let mut heard = |received, _| progress(received, Some(size));
    let result = link.package_with_reason(version, target, size, file, &mut heard).await;
    drop(ready_now);
    result
}

struct Shown<'a> {
    progress: &'a mut (dyn FnMut(SourceEvent) + Send),
}

impl Shown<'_> {
    fn failed(&mut self, via: Via, problem: Problem) {
        (self.progress)(SourceEvent::Failed(via, problem));
    }

    fn tell(&mut self, via: Via, received: u64, total: Option<u64>) {
        (self.progress)(SourceEvent::Progress(Progress { via, received, total }));
    }
}
