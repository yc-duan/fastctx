//! Inventory, preparation and retention refer to the same packages. Queued
//! updates survive restart and retain their version until completed or withdrawn.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fastctx_types::MachineName;
use fastctx_types::update::{Held, Manifest, Network, Report};
use fastctx_update::Cache;

use crate::Hub;
use crate::store::PackageTargets;

pub(super) fn targets(hub: &Hub, name: &MachineName, report: &Report) {
    let mut store = hub.store();
    let Some(member) = store.book.directory.member(name) else { return };
    let key = member.key;
    let kernel = report.target.clone().or_else(|| store.hub.package_targets.get(&key).and_then(|t| t.kernel.clone()));
    let targets = PackageTargets { kernel, app: report.app.as_ref().map(|app| app.target.clone()) };
    if store.hub.package_targets.get(&key) == Some(&targets) {
        return;
    }
    store.hub.package_targets.insert(key, targets);
    let current: BTreeSet<_> = store.book.directory.members.iter().map(|member| member.key).collect();
    store.hub.package_targets.retain(|key, _| current.contains(key));
    if let Err(error) = store.save_hub() {
        hub.log.line(format_args!("keeping the network's package targets: {error}"));
    }
}

fn for_targets(manifest: &Manifest, targets: &PackageTargets) -> Vec<Held> {
    let mut names: Vec<String> = targets.app.iter().chain(targets.kernel.iter()).cloned().collect();
    // Earlier headless machines report no target. Prepare the release's
    // command-line packages before announcing it to these machines too.
    if targets.kernel.is_none() && targets.app.is_none() {
        names.extend(manifest.fastctx.keys().map(|triple| format!("fastctx-{triple}")));
    }
    names
        .into_iter()
        .filter(|target| manifest.package(target).is_some())
        .map(|target| Held { version: manifest.version.clone(), target })
        .collect()
}

fn required(hub: &Hub, manifest: &Manifest) -> BTreeSet<Held> {
    demand(hub, manifest).into_keys().collect()
}

/// The packages of `manifest` the network's machines need, and how many
/// machines need each.
fn demand(hub: &Hub, manifest: &Manifest) -> BTreeMap<Held, usize> {
    let store = hub.store();
    let mut demand = BTreeMap::new();
    for member in &store.book.directory.members {
        let targets = store.hub.package_targets.get(&member.key).cloned().unwrap_or_default();
        for held in for_targets(manifest, &targets) {
            *demand.entry(held).or_insert(0) += 1;
        }
    }
    demand
}

/// Every verified package the hub can serve, including checked hand-placed files.
pub(super) fn held(hub: &Hub, cache: &Cache) -> BTreeSet<Held> {
    let mut held: BTreeSet<_> = cache.held().into_iter().collect();
    let placed: Vec<_> =
        hub.updates.lock().expect("never poisoned").placed.iter().map(|(held, mark)| (held.clone(), *mark)).collect();
    for (package, mark) in placed {
        let Some(archive) = fastctx_types::kernel::archive_of(&package.target) else { continue };
        if hub
            .releases
            .placed(&package.version, archive)
            .and_then(|file| file.metadata())
            .is_ok_and(|meta| (meta.len(), meta.modified().ok()) == mark)
        {
            held.insert(package);
        }
    }
    held
}

/// Offers are published only once all packages needed on this network are ready.
pub(super) fn ready_network(hub: &Hub, mut network: Network) -> Network {
    let cache = hub.updates.lock().expect("never poisoned").cache.clone();
    let held = held(hub, &cache);
    let ready = |manifest: &Manifest| required(hub, manifest).is_subset(&held);
    network.stable = network.stable.filter(ready);
    network.beta = network.beta.filter(ready);
    network.due.retain(|name, due| {
        let manifest = hub.updates.lock().expect("never poisoned").manifest(&due.version);
        let targets = {
            let store = hub.store();
            store.book.directory.member(name).and_then(|member| store.hub.package_targets.get(&member.key)).cloned()
        };
        manifest.is_some_and(|manifest| {
            for_targets(&manifest, &targets.unwrap_or_default()).iter().all(|package| held.contains(package))
        })
    });
    network
}

/// Only live demand retains an older version; time is never a reason to delete it.
pub(crate) fn prune(hub: &Hub) {
    let (cache, manifests) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.cache.clone(), updates.manifests().collect::<Vec<_>>())
    };
    let versions = cache.versions();
    let mut newest = std::collections::BTreeMap::<_, Manifest>::new();
    for manifest in manifests {
        let channel = fastctx_types::update::Channel::of(&manifest.version);
        if newest.get(&channel).is_none_or(|old| manifest.semver() > old.semver()) {
            newest.insert(channel, manifest);
        }
    }
    let mut keep: BTreeSet<String> = newest.into_values().map(|m| m.version).collect();
    // A queued demand and a new owner cannot enter between the decision and
    // the rename. Deleting the retired directory needs neither shared lock.
    let store = hub.store();
    let updates = hub.updates.lock().expect("never poisoned");
    keep.extend(
        store
            .hub
            .due
            .iter()
            .filter(|due| store.book.directory.by_key(&due.key).is_some())
            .map(|due| due.due.version.clone()),
    );
    keep.extend(updates.obtaining.keys().map(|held| held.version.clone()));
    keep.extend(updates.requests.values().map(|(held, _)| held.version.clone()));
    keep.extend(updates.serving.keys().map(|held| held.version.clone()));
    let mut retired = Vec::new();
    for (version, _) in versions {
        if keep.contains(&version) {
            continue;
        }
        match cache.retire(&version) {
            Ok(Some(path)) => retired.push(path),
            Ok(None) => {}
            Err(why) => hub.log.line(format_args!("{why}")),
        }
    }
    drop(updates);
    drop(store);
    let log = hub.log.clone();
    tokio::task::spawn_blocking(move || {
        for path in retired {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                log.line(format_args!("deleting retired packages {}: {error}", path.display()));
            }
        }
    });
}

/// Prepares both release lines and explicitly queued versions before telling
/// machines to update. The retry interval limits failed prefetches, not retention.
pub(crate) fn prefetch(hub: &Arc<Hub>) {
    let (policy, due) = {
        let store = hub.store();
        (store.hub.updates.clone(), store.hub.due.iter().map(|due| due.due.version.clone()).collect::<BTreeSet<_>>())
    };
    let (network, manifests, cache) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.network(policy, Default::default()), updates.manifests().collect::<Vec<_>>(), updates.cache.clone())
    };
    let unknown: BTreeSet<_> =
        due.iter().filter(|version| !manifests.iter().any(|manifest| &manifest.version == *version)).cloned().collect();
    for version in unknown {
        let key = Held { version: version.clone(), target: fastctx_types::update::MANIFEST.into() };
        let start = {
            let mut updates = hub.updates.lock().expect("never poisoned");
            let now = Instant::now();
            if updates.prefetched.get(&key).is_some_and(|at| now.duration_since(*at) < Duration::from_secs(60)) {
                false
            } else {
                updates.prefetched.insert(key, now);
                true
            }
        };
        let machine = {
            let store = hub.store();
            store
                .hub
                .due
                .iter()
                .find(|due| due.due.version == version)
                .and_then(|due| store.book.directory.by_key(&due.key))
                .map(|member| member.name.clone())
        };
        if start && let Some(machine) = machine {
            let hub = hub.clone();
            tokio::spawn(async move {
                if let Err(why) = prepare_update(&hub, &machine, &version).await {
                    hub.log.line(format_args!("preparing {version} for {machine}: {why}"));
                }
                super::tell(&hub, None);
            });
        }
    }
    let mut wanted: BTreeMap<Held, usize> = BTreeMap::new();
    for manifest in [network.stable, network.beta]
        .into_iter()
        .flatten()
        .chain(manifests.into_iter().filter(|m| due.contains(&m.version)))
    {
        for (held, machines) in demand(hub, &manifest) {
            *wanted.entry(held).or_insert(0) += machines;
        }
    }
    let held = held(hub, &cache);
    let wanted = {
        let mut updates = hub.updates.lock().expect("never poisoned");
        let now = Instant::now();
        updates.prefetched.retain(|_, at| now.duration_since(*at) < Duration::from_secs(60));
        let mut wanted: Vec<_> = wanted
            .into_iter()
            .filter(|(package, _)| {
                !held.contains(package)
                    && !updates.obtaining.contains_key(package)
                    && !updates.prefetched.contains_key(package)
            })
            .collect();
        // The package the most machines need comes first.
        wanted.sort_by(|(one, many), (other, more)| more.cmp(many).then_with(|| one.cmp(other)));
        for (package, _) in &wanted {
            updates.prefetched.insert(package.clone(), now);
        }
        wanted
    };
    if wanted.is_empty() {
        return;
    }
    // One after another, each with the whole of the ceiling in its turn
    // (`super::obtain`); machines are told as each comes.
    let hub = hub.clone();
    tokio::spawn(async move {
        for (package, _) in wanted {
            hub.log.line(format_args!("preparing {} of {} before announcing it", package.target, package.version));
            let _ = super::obtain::obtain(&hub, package, None).await;
            super::tell(&hub, None);
        }
    });
}

/// The batch waits here before issuing its first remote update. Its durable
/// demand was recorded by the ordinary update-management operation already.
pub(crate) async fn prepare_update(hub: &Arc<Hub>, name: &MachineName, version: &str) -> Result<(), String> {
    let targets = {
        let store = hub.store();
        let member = store.book.directory.member(name).ok_or_else(|| format!("there is no machine {name}"))?;
        if !store.hub.due.iter().any(|due| due.key == member.key && due.due.version == version) {
            return Err(format!("no update to {version} is queued for {name}"));
        }
        store.hub.package_targets.get(&member.key).cloned().unwrap_or_default()
    };
    let (known, releases, cache) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.manifest(version), updates.looked.releases.clone(), updates.cache.clone())
    };
    let manifest = match known {
        Some(manifest) => manifest,
        None => {
            let releases = if releases.iter().any(|release| release.version == version) {
                releases
            } else {
                let looked = fastctx_update::github::look(None).await;
                if let Some(why) = &looked.why {
                    hub.log.line(format_args!("looking for {version}: {why}"));
                }
                let releases = looked.releases.clone();
                hub.updates.lock().expect("never poisoned").looked = looked;
                releases
            };
            fastctx_update::github::manifest_of(&releases, version).await?
        }
    };
    cache.keep_manifest(&manifest)?;
    for package in for_targets(&manifest, &targets) {
        super::obtain::obtain(hub, package, None).await?;
    }
    Ok(())
}

pub(crate) fn done(hub: &Hub, name: &MachineName, generation: u64, id: u64) {
    hub.updates.lock().expect("never poisoned").requests.remove(&(name.clone(), generation, id));
    prune(hub);
}

/// A transfer retains its file even after its question has finished.
pub(super) struct Serving<'a> {
    hub: &'a Hub,
    member: &'a MachineName,
    package: Held,
}

impl<'a> Serving<'a> {
    pub(super) fn new(hub: &'a Hub, member: &'a MachineName, package: Held) -> Self {
        *hub.updates.lock().expect("never poisoned").serving.entry(package.clone()).or_default() += 1;
        Self { hub, member, package }
    }
}

impl Drop for Serving<'_> {
    fn drop(&mut self) {
        let mut updates = self.hub.updates.lock().expect("never poisoned");
        if let Some(count) = updates.serving.get_mut(&self.package) {
            *count -= 1;
            if *count == 0 {
                updates.serving.remove(&self.package);
            }
        }
        // Earlier callers cannot explicitly release a request. Its transfer
        // ending, or its session ending, is the evidence available from them.
        updates
            .requests
            .retain(|(name, _, _), (held, reports)| *reports || name != self.member || held != &self.package);
    }
}
