//! The network's versions (`fastctx_types::update`): what this machine runs
//! and knows of, told to a hub that keeps them, at each session and as it
//! changes; what the hub tells back; the packages it holds, downloaded from
//! it; and the ones it asks this machine to fetch for others, delivered.

use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_types::update::{FailureReason, Fetch, Network, PackageProgress, Problem, Report};
use serde::{Deserialize, Serialize};

use super::{Inner, Link};
use crate::data::{CHUNK, Endpoint, Stream};
use crate::wire::{Intent, Question, Told, Up};

#[derive(Default)]
pub(crate) struct Versions {
    /// The latest report, and the session it was told on.
    report: Mutex<(Option<Report>, Option<u64>)>,
    /// What the hub last told.
    network: Mutex<Option<(u64, Network)>>,
    policy: Mutex<Option<super::update_policy::UpdatePolicy>>,
    synced: std::sync::atomic::AtomicU64,
    fetching: Mutex<std::collections::HashMap<(u64, u64), tokio::sync::watch::Sender<bool>>>,
}

impl Versions {
    pub(super) fn with_policy(policy: Option<super::update_policy::UpdatePolicy>) -> Self {
        Self { policy: Mutex::new(policy), ..Self::default() }
    }
}

/// What each end of a package's data connection says before its bytes.
#[derive(Serialize, Deserialize)]
pub struct Sized {
    pub size: u64,
}

/// Keeps the package requested until its bytes are taken or the caller drops it.
pub struct PackageReady {
    pub size: u64,
    _interest: PackageInterest,
}

struct PackageInterest {
    inner: Arc<Inner>,
    connection: super::Connection,
    id: u64,
    reports: bool,
}

impl Drop for PackageInterest {
    fn drop(&mut self) {
        self.inner.asks.remove(self.id);
        if self.reports {
            let _ = self.connection.up.send(Up::PackageDone { id: self.id });
        }
    }
}

impl Inner {
    /// Tells the hub the versions of this machine, once the session takes
    /// them and again whenever they change.
    pub(crate) fn tell_versions(&self, report: Report) {
        let mut kept = self.versions.report.lock().expect("never poisoned");
        if kept.0.as_ref() == Some(&report) {
            return;
        }
        *kept = (Some(report), None);
        self.send_versions(&mut kept);
    }

    /// The session `generation` has the view: a hub that keeps versions is
    /// told this machine's.
    pub(super) fn versions_synced(&self, generation: u64) {
        self.versions.synced.store(generation, std::sync::atomic::Ordering::Release);
        let mut kept = self.versions.report.lock().expect("never poisoned");
        if kept.1 != Some(generation) {
            kept.1 = None;
            self.send_versions(&mut kept);
        }
    }

    fn send_versions(&self, kept: &mut (Option<Report>, Option<u64>)) {
        let Some(report) = &kept.0 else { return };
        if !self.view().versions {
            return;
        }
        let Some(connection) = self.connection() else { return };
        if connection.up.send(Up::Versions { report: Box::new(report.clone()) }).is_ok() {
            kept.1 = Some(connection.generation);
        }
    }

    pub(super) fn heard_versions(&self, generation: u64, network: Network) {
        let policy = super::update_policy::UpdatePolicy::of(&network);
        {
            let mut kept = self.versions.policy.lock().expect("never poisoned");
            if kept.as_ref() != Some(&policy) {
                if let Err(error) = policy.save(self.dir.path()) {
                    self.node.log(format!("keeping the network's update policy: {error}"));
                }
                *kept = Some(policy);
            }
        }
        *self.versions.network.lock().expect("never poisoned") = Some((generation, network));
        self.node.versions_changed();
    }

    /// The hub asks this machine to fetch a package for others: the node
    /// brings it here, checked, and it is delivered; or the hub is told why
    /// not, and asks another.
    pub(super) fn fetch_for_hub(self: &Arc<Self>, generation: u64, fetch: Fetch) {
        let id = fetch.id;
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        {
            let mut fetching = self.versions.fetching.lock().expect("never poisoned");
            if fetching.contains_key(&(generation, id)) {
                return;
            }
            fetching.insert((generation, id), stop);
        }
        let inner = self.clone();
        tokio::spawn(async move {
            let reports = fetch.progress;
            let (progress, mut changes) = tokio::sync::watch::channel(PackageProgress::default());
            let work = async {
                let _lease = inner.node.retain_package(&fetch.version);
                let path = inner.node.relay(fetch, progress).await?;
                inner.deliver(id, &path).await
            };
            tokio::pin!(work);
            let mut last_progress = PackageProgress::default();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            let delivered = loop {
                tokio::select! {
                    result = &mut work => break result,
                    _ = stopped.wait_for(|stop| *stop) => break Err("the hub cancelled the fetch".into()),
                    _ = tick.tick() => {
                        let Some(connection) = inner.connection().filter(|c| c.generation == generation) else {
                            break Err("the connection that requested the fetch ended".into());
                        };
                        let progress = *changes.borrow_and_update();
                        if reports && progress != last_progress {
                            last_progress = progress;
                            let _ = connection.up.send(Up::FetchProgress { id, progress });
                        }
                    }
                }
            };
            inner.versions.fetching.lock().expect("never poisoned").remove(&(generation, id));
            if let Err(why) = delivered {
                inner.node.log(format!("fetching a package for the hub's request {id}: {why}"));
                if let Some(connection) = inner.connection().filter(|c| c.generation == generation) {
                    let _ = connection.up.send(Up::Unfetched { id, why });
                }
            }
        });
    }

    pub(super) fn cancel_fetches(&self, generation: u64) {
        for ((session, _), stop) in self.versions.fetching.lock().expect("never poisoned").iter() {
            if *session == generation {
                stop.send_replace(true);
            }
        }
    }

    pub(super) fn cancel_fetch(&self, generation: u64, id: u64) {
        if let Some(stop) = self.versions.fetching.lock().expect("never poisoned").get(&(generation, id)) {
            stop.send_replace(true);
        }
    }

    async fn deliver(&self, id: u64, path: &Path) -> Result<(), String> {
        let mut file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let size = file.metadata().map_err(|error| error.to_string())?.len();
        let Endpoint::Hub { upstreams, hub, cert } = self.data_endpoint() else {
            return Err("packages go to the hub itself".to_string());
        };
        let mut stream =
            Stream::to_hub(&upstreams, &hub, cert, &self.key, |key, proof| Intent::Deliver { key, proof, id }).await?;
        stream.say(&Sized { size }).await?;
        let mut buffer = vec![0; CHUNK];
        loop {
            let read = file.read(&mut buffer).map_err(|error| format!("{}: {error}", path.display()))?;
            if read == 0 {
                break;
            }
            stream.send(&buffer[..read]).await?;
        }
        stream.finish().await
    }
}

impl Link {
    /// Queries or changes updates on a hub advertising the account view.
    pub async fn network_updates(
        &self,
        account: &fastctx_types::AccountName,
        request: fastctx_types::update::ViewCall,
    ) -> Result<fastctx_types::update::ViewReply, String> {
        if matches!(
            request,
            fastctx_types::update::ViewCall::Repair { .. }
                | fastctx_types::update::ViewCall::CancelRepair { .. }
                | fastctx_types::update::ViewCall::RepairResult { .. }
        ) && !self.versions().is_some_and(|network| network.repair_supported)
        {
            return Err("this hub is too old to arrange repairs; update the hub first".into());
        }
        if !self.versions().is_some_and(|network| network.updates_view) {
            return Err("this hub does not support the updates view; update the hub first".into());
        }
        match self.inner.ask(Question::NetworkUpdates { account: account.clone(), request }).await? {
            Told::NetworkUpdates { reply } => Ok(reply),
            _ => Err("the hub answered something other than the updates view".into()),
        }
    }

    /// Tells the hub this machine's versions, now and at every session.
    pub fn report_versions(&self, report: Report) {
        self.inner.tell_versions(report);
    }

    /// What the hub last told of the network's versions; none from a hub
    /// that does not keep them, or before it told.
    pub fn versions(&self) -> Option<Network> {
        self.inner.versions.network.lock().expect("never poisoned").as_ref().map(|(_, network)| network.clone())
    }

    /// Last received versions, or only the durable policy before the hub answers.
    /// Saved constraints never recreate machine reports or queued updates.
    pub fn update_network(&self) -> Option<Network> {
        self.versions().or_else(|| {
            self.inner.versions.policy.lock().expect("never poisoned").as_ref().map(|policy| policy.network())
        })
    }

    /// Versions received on the connection that is still open now.
    pub fn current_versions(&self) -> Option<Network> {
        let connection = self.inner.connection()?;
        self.inner
            .versions
            .network
            .lock()
            .expect("never poisoned")
            .as_ref()
            .filter(|(generation, _)| *generation == connection.generation)
            .map(|(_, network)| network.clone())
    }

    /// Whether the hub keeps the network's versions and hands out packages.
    pub fn keeps_versions(&self) -> bool {
        self.inner.view().versions
    }

    /// Asks the hub for the package `target` of `version`, answered once
    /// it holds it, bringing it in first when it has to: its size.
    pub async fn ask_package(&self, version: &str, target: &str) -> Result<u64, String> {
        self.ask_package_progress(version, target, None).await.map(|ready| ready.size)
    }

    /// Asks from the start of preparation; newer hubs report received bytes
    /// until they can answer. Older hubs receive the original question.
    pub async fn ask_package_progress(
        &self,
        version: &str,
        target: &str,
        progress: Option<tokio::sync::watch::Sender<PackageProgress>>,
    ) -> Result<PackageReady, String> {
        if !self.keeps_versions() {
            return Err("the network's hub does not hand out packages".to_string());
        }
        let began = std::time::Instant::now();
        let connection = self.inner.connected().await.ok_or("not connected to the hub")?;
        let reports = self.inner.versions.synced.load(std::sync::atomic::Ordering::Acquire) == connection.generation
            && self.inner.view().package_progress;
        let question =
            Question::Package { version: version.to_string(), target: target.to_string(), progress: reports };
        let id = self.inner.next_id();
        let (answer, answered) = tokio::sync::oneshot::channel();
        self.inner.asks.insert(id, connection.generation, answer, progress);
        let interest = PackageInterest { inner: self.inner.clone(), connection, id, reports };
        interest.connection.up.send(Up::Ask { id, question }).map_err(|_| "the connection to the hub ended")?;
        let answer = fastctx_platform::doing::outside(
            "the hub's package",
            std::time::Duration::from_secs(45 * 60).saturating_sub(began.elapsed()),
            answered,
        )
        .await
        .map_err(|late| late.to_string())?
        .map_err(|_| "the connection to the hub ended before it answered")??;
        match answer {
            Told::Package { size } => Ok(PackageReady { size, _interest: interest }),
            _ => Err("the hub answered something other than a package".to_string()),
        }
    }

    /// Records a package source's result in the node's own log.
    pub fn log_package(&self, message: String) {
        self.inner.node.log(message);
    }

    /// Downloads the package `target` of `version`, `size` bytes the hub
    /// holds (`ask_package`), into `file`, going on from what an earlier
    /// download left there. `progress` hears the bytes in the file and the
    /// package's size as they grow. The bytes are the caller's to check
    /// against the package's signature.
    pub async fn package(
        &self,
        version: &str,
        target: &str,
        size: u64,
        file: &Path,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<(), String> {
        self.package_with_reason(version, target, size, file, progress).await.map_err(|problem| problem.why)
    }

    /// Receives package bytes while keeping local disk failures distinct from
    /// transport failures. The caller still verifies the package signature.
    pub async fn package_with_reason(
        &self,
        version: &str,
        target: &str,
        size: u64,
        file: &Path,
        progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<(), Problem> {
        // Written at the end it reaches, and cut back when it is longer than
        // the package: a file opened only to append cannot be cut on Windows.
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(file)
            .map_err(|error| Problem::new(FailureReason::Disk, format!("{}: {error}", file.display())))?;
        let mut have = out
            .seek(std::io::SeekFrom::End(0))
            .map_err(|error| Problem::new(FailureReason::Disk, error.to_string()))?;
        if have > size {
            out.set_len(0).map_err(|error| Problem::new(FailureReason::Disk, error.to_string()))?;
            out.seek(std::io::SeekFrom::Start(0))
                .map_err(|error| Problem::new(FailureReason::Disk, error.to_string()))?;
            have = 0;
        }
        progress(have, Some(size));
        if have < size {
            let Endpoint::Hub { upstreams, hub, cert } = self.inner.data_endpoint() else {
                return Err("packages come from the hub itself".into());
            };
            let (version, target) = (version.to_string(), target.to_string());
            let mut stream = Stream::to_hub(&upstreams, &hub, cert, &self.inner.key, |key, proof| Intent::Package {
                key,
                proof,
                version,
                target,
                from: have,
            })
            .await?;
            let said: Sized = stream.hear().await?;
            if said.size != size {
                return Err(format!("the hub holds {} bytes of the package now, not {size}", said.size).into());
            }
            let mut written = have;
            let mut disk_error = None;
            let received = stream
                .take(size - have, |piece| {
                    if let Err(error) = out.write_all(piece) {
                        let why = format!("{}: {error}", file.display());
                        disk_error = Some(Problem::new(FailureReason::Disk, why.clone()));
                        return Err(why);
                    }
                    written += piece.len() as u64;
                    progress(written, Some(size));
                    Ok(())
                })
                .await;
            received.map_err(|why| disk_error.unwrap_or_else(|| Problem::from(why)))?;
            out.sync_all().map_err(|error| Problem::new(FailureReason::Disk, error.to_string()))?;
            stream.done().await?;
        }
        Ok(())
    }
}

/// Where a package is kept while it comes, beside where it goes.
pub fn partial(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}
