//! The network's versions (`world-upgrade.md`, 怎样知道有新版本、从哪里拿):
//! what each machine runs and knows of; the newest release on each channel
//! that any machine or the hub itself found, told to every machine with the
//! network's policy; and the packages the hub brings in for machines that
//! cannot reach GitHub, from GitHub itself or from a machine that can or
//! holds one, kept here until a newer version replaces them. Nothing of the
//! machines' reports is kept past their sessions: each tells it again.

mod obtain;
mod transfer;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::Duration;

use fastctx_net::wire::Down;
use fastctx_types::MachineName;
use fastctx_types::update::{Held, Looked, Manifest, Network, Policy, Report, Running};
use fastctx_update::Cache;
use tokio::sync::oneshot;

pub(crate) use obtain::{ask, unfetched};
pub(crate) use transfer::{deliver, serve};

use crate::{Hub, Out};

/// How often the hub looks on GitHub itself.
const LOOK_EVERY: Duration = Duration::from_secs(6 * 3600);

pub(crate) struct Updates {
    /// Each machine's latest report, with the session it came on.
    reports: HashMap<MachineName, (u64, Report)>,
    /// What the hub found when it last looked on GitHub.
    looked: Looked,
    /// What every machine was last told.
    told: Option<Network>,
    /// The packages being brought in, and who waits for each.
    obtaining: HashMap<Held, Vec<oneshot::Sender<Result<u64, String>>>>,
    /// The requests machines were sent to fetch a package (`Down::Fetch`).
    fetches: HashMap<u64, Fetching>,
    cache: Cache,
}

struct Fetching {
    helper: MachineName,
    generation: u64,
    held: Held,
    done: Option<oneshot::Sender<Result<(), String>>>,
}

impl Updates {
    pub fn new(dir: &Path) -> Self {
        Self {
            reports: HashMap::new(),
            looked: Looked::default(),
            told: None,
            obtaining: HashMap::new(),
            fetches: HashMap::new(),
            cache: Cache::new(dir.join("updates")),
        }
    }

    /// Every release the hub knows the manifest of.
    fn manifests(&self) -> impl Iterator<Item = Manifest> + '_ {
        self.reports
            .values()
            .flat_map(|(_, report)| report.found.iter().cloned())
            .chain(self.looked.stable.iter().cloned())
            .chain(self.looked.beta.iter().cloned())
            .chain(self.cache.manifests())
    }

    fn network(&self, policy: Policy) -> Network {
        let mut stable: Option<Manifest> = None;
        let mut beta: Option<Manifest> = None;
        for manifest in self.manifests() {
            let Some(version) = manifest.semver() else { continue };
            let slot = if version.pre.is_empty() { &mut stable } else { &mut beta };
            if slot.as_ref().is_none_or(|kept| kept.semver().is_none_or(|kept| version > kept)) {
                *slot = Some(manifest);
            }
        }
        if beta.as_ref().is_some_and(|beta| stable.as_ref().is_some_and(|stable| stable.semver() > beta.semver())) {
            beta = None;
        }
        let machines = self
            .reports
            .iter()
            .map(|(name, (_, report))| {
                let running = Running {
                    kernel: report.kernel.clone(),
                    app: report.app.as_ref().map(|app| app.version.clone()),
                    github: report.github,
                };
                (name.clone(), running)
            })
            .collect();
        Network { stable, beta, policy, machines }
    }

    /// The manifest of the release `version`, when the hub knows it.
    fn manifest(&self, version: &str) -> Option<Manifest> {
        self.manifests().find(|manifest| manifest.version == version)
    }
}

fn policy(hub: &Hub) -> Policy {
    hub.store().hub.updates.clone()
}

/// The network's versions as every machine is told them now.
pub(crate) fn network(hub: &Hub) -> Network {
    let policy = policy(hub);
    hub.updates.lock().expect("never poisoned").network(policy)
}

/// `name` told the hub its versions on its session `generation`.
pub(crate) fn heard(hub: &Hub, name: &MachineName, generation: u64, report: Report) {
    hub.updates.lock().expect("never poisoned").reports.insert(name.clone(), (generation, report));
    tell(hub, None);
}

/// The session `generation` of `name`, which reads versions, opened: it is
/// told them.
pub(crate) fn opened(hub: &Hub, name: &MachineName) {
    tell(hub, Some(name));
}

/// The session `generation` of `name` ended: what it reported goes, and
/// whatever it was asked to fetch is asked of another.
pub(crate) fn closed(hub: &Hub, name: &MachineName, generation: u64) {
    let had = {
        let mut updates = hub.updates.lock().expect("never poisoned");
        for fetching in updates.fetches.values_mut() {
            if fetching.helper == *name
                && fetching.generation == generation
                && let Some(done) = fetching.done.take()
            {
                let _ = done.send(Err(format!("{name} left before it delivered")));
            }
        }
        match updates.reports.get(name) {
            Some((reported, _)) if *reported == generation => updates.reports.remove(name).is_some(),
            _ => false,
        }
    };
    if had {
        tell(hub, None);
    }
}

/// Tells every machine that reads versions what changed, or, with `only`,
/// tells that one machine all of it.
pub(crate) fn tell(hub: &Hub, only: Option<&MachineName>) {
    let policy = policy(hub);
    let network = {
        let mut updates = hub.updates.lock().expect("never poisoned");
        let network = updates.network(policy);
        if only.is_none() && updates.told.as_ref() == Some(&network) {
            return;
        }
        if only.is_none() {
            updates.told = Some(network.clone());
        }
        network
    };
    let sessions = hub.sessions();
    for (name, session) in sessions.iter() {
        if session.versions && only.is_none_or(|only| only == name) {
            let _ = session.out.send(Out::Frame(Down::Versions { network: Box::new(network.clone()) }));
        }
    }
}

/// Looks on GitHub now and every few hours, while the hub runs.
pub(crate) async fn look_every(hub: Weak<Hub>) {
    // A hub just started lets its machines connect first.
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        let looked = fastctx_update::github::look().await;
        let Some(hub) = hub.upgrade() else { return };
        if let Some(why) = &looked.why {
            hub.log.line(format_args!("looking for new versions on GitHub: {why}"));
        }
        {
            let mut updates = hub.updates.lock().expect("never poisoned");
            // A failed look keeps what the last one found.
            if looked.why.is_none() {
                updates.looked = looked;
            } else {
                updates.looked.at = looked.at;
                updates.looked.github = looked.github;
            }
        }
        tell(&hub, None);
        drop(hub);
        tokio::time::sleep(LOOK_EVERY).await;
    }
}

/// Keeps only the newest release on each channel, and `kept`.
fn prune(hub: &Arc<Hub>, kept: &str) {
    let network = network(hub);
    let mut keep = vec![kept];
    keep.extend(network.stable.as_ref().map(|manifest| manifest.version.as_str()));
    keep.extend(network.beta.as_ref().map(|manifest| manifest.version.as_str()));
    hub.updates.lock().expect("never poisoned").cache.prune(&keep);
}
