//! One owner of a package on this device, shared by its own update and all
//! hubs asking it to help. Subscribers leaving cancel only their interest;
//! the last one leaving stops the transfer and waits for its files to go.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fastctx_net::link::Link;
use fastctx_types::update::{FailureReason, Fetch, Manifest, Package, Problem};
use tokio::sync::watch;

use crate::Cache;
use crate::obtain::{Brought, Progress, SourceEvent};

pub type Log = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone, Default)]
pub struct Pool(Arc<Mutex<HashMap<PathBuf, Owner>>>, Arc<Mutex<HashMap<String, usize>>>);

/// Retains a version while an external transfer uses its verified file.
pub struct Lease {
    pool: Pool,
    version: String,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut pins = self.pool.1.lock().expect("never poisoned");
        if let Some(count) = pins.get_mut(&self.version) {
            *count -= 1;
            if *count == 0 {
                pins.remove(&self.version);
            }
        }
    }
}

struct Owner {
    local: watch::Sender<Local>,
    state: watch::Sender<State>,
    stopping: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Local {
    pub requested: bool,
    pub carried: Option<PathBuf>,
}

#[derive(Clone, Default)]
struct State {
    progress: Option<Progress>,
    local_progress: Option<Progress>,
    local_failure: Option<Problem>,
    outcome: Option<Result<Brought, Problem>>,
}

pub(crate) struct Request {
    pub version: String,
    pub target: String,
    pub package: Package,
    pub links: Vec<Link>,
    pub local: Local,
    pub network_now: bool,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

impl Pool {
    pub fn retain(&self, version: &str) -> Lease {
        *self.1.lock().expect("never poisoned").entry(version.to_string()).or_default() += 1;
        Lease { pool: self.clone(), version: version.to_string() }
    }

    /// Obtains a package once for this device, trying GitHub first; the
    /// network's source asks each supplying hub in turn. `network_now` also
    /// starts the network when a recent check could not reach GitHub.
    #[allow(clippy::too_many_arguments)]
    pub async fn obtain(
        &self,
        cache: &Cache,
        manifest: &Manifest,
        target: &str,
        links: &[&Link],
        network_now: bool,
        carried: Option<PathBuf>,
        progress: &mut (dyn FnMut(Progress) + Send),
        log: Log,
    ) -> Result<Brought, Problem> {
        let _lease = self.retain(&manifest.version);
        let package = manifest.package(target).ok_or_else(|| {
            Problem::new(FailureReason::NoPackage, format!("{} has no package for {target}", manifest.version))
        })?;
        let request = Request {
            version: manifest.version.clone(),
            target: target.to_string(),
            package: package.clone(),
            links: links.iter().filter(|link| link.keeps_versions()).map(|link| (*link).clone()).collect(),
            local: Local { requested: false, carried },
            network_now,
        };
        let brought = self.join(cache, request, progress, log).await?;
        cache.keep_manifest(manifest).map_err(|why| Problem::new(FailureReason::Disk, why))?;
        Ok(brought)
    }

    /// Joins the device's owner, asking it to use its carried package or
    /// GitHub. The hub's request number never names a local file.
    pub async fn relay(
        &self,
        cache: &Cache,
        fetch: &Fetch,
        carried: Option<PathBuf>,
        progress: &mut (dyn FnMut(Progress) + Send),
        log: Log,
    ) -> Result<Brought, Problem> {
        let request = Request {
            version: fetch.version.clone(),
            target: fetch.target.clone(),
            package: fetch.package.clone(),
            links: Vec::new(),
            local: Local { requested: true, carried },
            network_now: false,
        };
        self.join(cache, request, progress, log).await
    }

    async fn join(
        &self,
        cache: &Cache,
        request: Request,
        progress: &mut (dyn FnMut(Progress) + Send),
        log: Log,
    ) -> Result<Brought, Problem> {
        let key = cache.path(&request.version, &request.target)?;
        loop {
            let (mut state, stopping) = {
                let mut owners = self.0.lock().expect("never poisoned");
                if let Some(path) = cache.get(&request.version, &request.target) {
                    return Ok(Brought { path, github_speed: None, github_at: None });
                }
                if let Some(owner) = owners.get_mut(&key) {
                    if request.local.requested && !owner.stopping {
                        owner.local.send_replace(request.local.clone());
                    }
                    (owner.state.subscribe(), owner.stopping)
                } else {
                    let (local, source) = watch::channel(request.local.clone());
                    let (state, receiver) = watch::channel(State::default());
                    owners.insert(key.clone(), Owner { local, state: state.clone(), stopping: false });
                    let (pool, cache, key, log) = (self.clone(), cache.clone(), key.clone(), log.clone());
                    let request = Request {
                        version: request.version.clone(),
                        target: request.target.clone(),
                        package: request.package.clone(),
                        links: request.links.clone(),
                        local: request.local.clone(),
                        network_now: request.network_now,
                    };
                    tokio::spawn(async move {
                        pool.own(cache, key, request, source, state, log).await;
                    });
                    (receiver, false)
                }
            };
            loop {
                let current = state.borrow_and_update().clone();
                if let Some(heard) = if request.local.requested { current.local_progress } else { current.progress } {
                    progress(heard);
                }
                if let Some(outcome) = current.outcome {
                    if stopping {
                        break;
                    }
                    return outcome;
                }
                // A helper must not wait for the hub it is supplying after its
                // own source failed (2026-10-20).
                if request.local.requested
                    && !stopping
                    && let Some(problem) = current.local_failure
                {
                    return Err(problem);
                }
                if state.changed().await.is_err() {
                    return Err("the package owner ended without a result".into());
                }
            }
        }
    }

    async fn own(
        &self,
        cache: Cache,
        key: PathBuf,
        request: Request,
        local: watch::Receiver<Local>,
        state: watch::Sender<State>,
        log: Log,
    ) {
        let id = NEXT.fetch_add(2, Ordering::Relaxed);
        let files = (
            cache.attempt(&request.version, &request.target, id),
            cache.attempt(&request.version, &request.target, id + 1),
        );
        let outcome = match &files {
            (Ok(hub_file), Ok(local_file)) => {
                let mut sources: [Option<Progress>; 2] = [None, None];
                let mut heard = |event: SourceEvent| {
                    state.send_modify(|state| {
                        match event {
                            SourceEvent::Progress(progress) => {
                                let index = usize::from(progress.via == fastctx_types::update::Via::Network);
                                sources[index] = Some(progress);
                                if progress.via == fastctx_types::update::Via::Github {
                                    state.local_progress = Some(progress);
                                }
                            }
                            SourceEvent::Failed(via, problem) => {
                                sources[usize::from(via == fastctx_types::update::Via::Network)] = None;
                                if via == fastctx_types::update::Via::Github {
                                    state.local_failure = Some(problem);
                                }
                            }
                        }
                        // Only active sources can lead. An invalid complete file
                        // must not keep the display at 100% while another starts.
                        state.progress =
                            sources.iter().flatten().max_by_key(|p| p.received).copied().or(Some(Progress {
                                via: fastctx_types::update::Via::Github,
                                received: 0,
                                total: request.package.size,
                            }));
                    });
                };
                let work = crate::obtain::run(&cache, &request, hub_file, local_file, local, &mut heard, &log);
                tokio::pin!(work);
                loop {
                    tokio::select! {
                        result = &mut work => break result,
                        () = state.closed() => {
                            let mut owners = self.0.lock().expect("never poisoned");
                            if state.receiver_count() == 0 {
                                owners.get_mut(&key).expect("owner stays until cleanup").stopping = true;
                                break Err("nobody needs this package anymore".into());
                            }
                        }
                    }
                }
            }
            (Err(why), _) | (_, Err(why)) => Err(Problem::new(FailureReason::Disk, why.clone())),
        };
        // The futures and their open files have gone before cleanup starts.
        for file in [files.0, files.1].into_iter().flatten() {
            cleanup(&file, &log).await;
        }
        if let Err(why) = &outcome {
            log(format!("bringing {} of {}: {why}", request.target, request.version));
        }
        let mut owners = self.0.lock().expect("never poisoned");
        state.send_modify(|state| state.outcome = Some(outcome));
        owners.remove(&key);
    }

    /// Clears stale partials while excluding this process's acquisition owner.
    /// Standalone repair downloads use a separate private inventory.
    pub fn clear_incomplete(
        &self,
        cache: &Cache,
        version: &str,
        target: &str,
        mut before: impl FnMut(&std::path::Path) -> Result<(), String>,
    ) -> Result<bool, String> {
        let owners = self.0.lock().expect("never poisoned");
        if owners.contains_key(&cache.path(version, target)?) {
            return Ok(false);
        }
        for path in cache.incomplete(version, target)? {
            before(&path)?;
            std::fs::remove_file(&path).map_err(|e| format!("deleting stale package {}: {e}", path.display()))?;
        }
        Ok(true)
    }

    /// Retires unused versions without racing a newly admitted acquisition.
    pub fn prune(&self, cache: &Cache, keep: &[String], log: Log) {
        let owners = self.0.lock().expect("never poisoned");
        let mut keep = keep.to_vec();
        let pins = self.1.lock().expect("never poisoned");
        keep.extend(pins.keys().cloned());
        keep.extend(
            owners.keys().filter_map(|path| path.parent()?.file_name()?.to_str()?.strip_prefix('v').map(str::to_owned)),
        );
        let mut retired = Vec::new();
        for (version, _) in cache.versions() {
            if keep.contains(&version) {
                continue;
            }
            match cache.retire(&version) {
                Ok(Some(path)) => retired.push(path),
                Ok(None) => {}
                Err(why) => log(why),
            }
        }
        drop(pins);
        drop(owners);
        tokio::task::spawn_blocking(move || {
            for path in retired {
                if let Err(error) = std::fs::remove_dir_all(&path) {
                    log(format!("deleting {}: {error}", path.display()));
                }
            }
        });
    }

    /// Versions with a live owner, including one still cleaning up.
    pub fn active_versions(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("never poisoned")
            .keys()
            .filter_map(|path| path.parent()?.file_name()?.to_str()?.strip_prefix('v').map(str::to_owned))
            .collect()
    }
}

/// How long a try's file is tried to be deleted, as something else (an
/// antivirus, an indexer) may hold it for a moment. The owner, and everyone
/// waiting on it, waits that long at most: one left is deleted as the
/// program next starts (`Cache::clear_attempts`).
const CLEANUP_TRIES: u32 = 30;

async fn cleanup(path: &std::path::Path, log: &Log) {
    let mut last = None;
    for _ in 0..CLEANUP_TRIES {
        match tokio::fs::remove_file(path).await {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                let why = format!("removing temporary package {}: {error}", path.display());
                if last.as_ref() != Some(&why) {
                    log(why.clone());
                    last = Some(why);
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    log(format!("left temporary package {} for the next start to delete", path.display()));
}
