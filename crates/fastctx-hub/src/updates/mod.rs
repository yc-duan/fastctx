//! The network's versions (`world-upgrade.md`, 怎样知道有新版本、从哪里拿):
//! what each machine runs and knows of; the newest release on each line
//! that any machine or the hub itself found, told to every machine with the
//! network's policy, the hub's own version and the updates kept for
//! machines away; and the packages the hub brings in for machines that
//! cannot reach GitHub, from GitHub itself or from a machine that can or
//! holds one, kept here until a newer version replaces them. Of the
//! machines' reports only what each ran is kept past its session, in
//! memory: each tells the rest again.

mod obtain;
mod transfer;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::Duration;

use std::collections::BTreeMap;

use fastctx_net::wire::Down;
use fastctx_types::MachineName;
use fastctx_types::machine_log::{Action, Actor, Outcome};
use fastctx_types::update::{Asked, Channel, Due, Held, Looked, Manifest, Network, Policy, Report, Running};
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
    /// What each machine ran when it last reported, online or away.
    last: HashMap<MachineName, Running>,
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
            last: HashMap::new(),
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

    fn network(&self, policy: Policy, due: BTreeMap<MachineName, Due>) -> Network {
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
        let mut machines: BTreeMap<MachineName, Running> =
            self.last.iter().map(|(name, running)| (name.clone(), Running { away: true, ..running.clone() })).collect();
        machines.extend(self.reports.iter().map(|(name, (_, report))| (name.clone(), running(report))));
        // Devices of 1.0.1 beta 1 and 2 follow the policy's channel: the beta
        // line takes every release, as the line of what they run does.
        let policy = Policy { channel: Channel::Beta, ..policy };
        Network { stable, beta, policy, machines, hub: Some(env!("CARGO_PKG_VERSION").to_string()), due }
    }

    /// The manifest of the release `version`, when the hub knows it.
    fn manifest(&self, version: &str) -> Option<Manifest> {
        self.manifests().find(|manifest| manifest.version == version)
    }
}

fn running(report: &Report) -> Running {
    Running {
        kernel: report.kernel.clone(),
        app: report.app.as_ref().map(|app| app.version.clone()),
        github: report.github,
        away: false,
        hub_here: report.hub_here,
    }
}

/// What `name` ran when it last told its versions, while the hub ran.
pub(crate) fn running_of(hub: &Hub, name: &MachineName) -> Option<Running> {
    hub.updates.lock().expect("never poisoned").last.get(name).cloned()
}

/// What the hub's settings say of updates: the policy, and the updates kept
/// for machines, under their names now.
fn kept(hub: &Hub) -> (Policy, BTreeMap<MachineName, Due>) {
    let store = hub.store();
    let due = store
        .hub
        .due
        .iter()
        .filter_map(|kept| store.book.directory.by_key(&kept.key).map(|member| (member.name.clone(), kept.due.clone())))
        .collect();
    (store.hub.updates.clone(), due)
}

/// The network's versions as every machine is told them now.
pub(crate) fn network(hub: &Hub) -> Network {
    let (policy, due) = kept(hub);
    hub.updates.lock().expect("never poisoned").network(policy, due)
}

/// `name` told the hub its versions on its session `generation`.
pub(crate) fn heard(hub: &Hub, name: &MachineName, generation: u64, report: Report) {
    let (kernel, asked) = (report.kernel.clone(), report.asked.clone());
    {
        let mut updates = hub.updates.lock().expect("never poisoned");
        updates.last.insert(name.clone(), running(&report));
        updates.reports.insert(name.clone(), (generation, report));
    }
    settle(hub, name, &kernel, asked);
    tell(hub, None);
}

/// `name` runs `kernel` now: an update kept for it that this release is or
/// passes was made, and one it made as someone asked goes into the machine
/// log, once.
fn settle(hub: &Hub, name: &MachineName, kernel: &str, asked: Option<Asked>) {
    let mut store = hub.store();
    let Some(key) = store.book.directory.member(name).map(|member| member.key) else { return };
    let runs = fastctx_types::update::release(kernel);
    let before = store.hub.due.len();
    store.hub.due.retain(|kept| kept.key != key || fastctx_types::update::release(&kept.due.version) > runs);
    if store.hub.due.len() != before
        && let Err(error) = store.save_hub()
    {
        hub.log.line(format_args!("keeping that {name} made the update kept for it: {error}"));
    }
    let Some(asked) = asked.filter(|asked| asked.from != kernel) else { return };
    if store.book.machine_log.updated(&key, &asked.from, kernel) {
        return;
    }
    let directory = store.book.directory.clone();
    let actor = Actor { account: None, authenticated: true, member: Some(name.clone()), peer: None };
    let action = Action::Updated { machine: name.clone(), from: asked.from, to: kernel.to_string(), by: asked.by };
    match crate::machine_log::keep(&mut store, &directory, actor, action, Outcome::Done) {
        Ok(()) => hub.machine_log_changed.notify_one(),
        Err(error) => hub.log.line(format_args!("{error}")),
    }
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
    let (policy, due) = kept(hub);
    let network = {
        let mut updates = hub.updates.lock().expect("never poisoned");
        let network = updates.network(policy, due);
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
