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
    let index = hub.index_of(&store);
    let directory = fastctx_net::directory_index::Indexed::new(&store.book.directory, &index);
    let Some(member) = directory.member(name) else { return };
    let key = member.key;
    let kernel = report.target.clone().or_else(|| store.hub.package_targets.get(&key).and_then(|t| t.kernel.clone()));
    let targets = PackageTargets { kernel, app: report.app.as_ref().map(|app| app.target.clone()) };
    if store.hub.package_targets.get(&key) == Some(&targets) {
        return;
    }
    let before = store.hub.package_targets.insert(key, targets);
    if let Err(error) = store.save_package_target(&key) {
        match before {
            Some(before) => {
                store.hub.package_targets.insert(key, before);
            }
            None => {
                store.hub.package_targets.remove(&key);
            }
        }
        hub.log.line(format_args!("keeping the network's package targets: {error}"));
    }
}

pub(super) fn for_targets(manifest: &Manifest, targets: &PackageTargets) -> Vec<Held> {
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

fn needs_network(hub: &Hub, name: &MachineName) -> bool {
    let Some(key) = hub.store().book.directory.member(name).map(|m| m.key) else { return false };
    hub.updates.lock().expect("never poisoned").last.get(&key).is_some_and(|last| last.report.needs_network())
}

pub(super) struct Offers {
    through: BTreeMap<MachineName, PackageTargets>,
    known: Vec<Manifest>,
    held: BTreeSet<Held>,
}

impl Offers {
    pub(super) fn new(hub: &Hub, directory: &fastctx_net::directory_index::Indexed<'_>) -> Self {
        let store = hub.store();
        let updates = hub.updates.lock().expect("never poisoned");
        let through: BTreeMap<_, _> = updates
            .last
            .iter()
            .filter(|(_, last)| last.report.needs_network())
            .filter_map(|(key, _)| {
                directory.by_key(key).map(|member| {
                    (member.name.clone(), store.hub.package_targets.get(key).cloned().unwrap_or_default())
                })
            })
            .collect();
        let known = updates.manifests().collect();
        let cache = updates.cache.clone();
        drop(updates);
        drop(store);
        let held = if through.is_empty() { BTreeSet::new() } else { held(hub, &cache) };
        Self { through, known, held }
    }

    pub(super) fn scope(&self, name: &MachineName) -> Option<(Option<String>, Option<String>)> {
        self.through.get(name).map(|targets| (targets.kernel.clone(), targets.app.clone()))
    }

    pub(super) fn offer_to(&self, name: &MachineName, network: &mut Network) {
        let Some(targets) = self.through.get(name) else { return };
        network.stable = network.stable.take().and_then(|newest| standing(&self.known, &newest, targets, &self.held));
        network.beta = network.beta.take().and_then(|newest| standing(&self.known, &newest, targets, &self.held));
    }
}

/// The release of `newest`'s line a machine updating through the network is
/// offered: `newest` once the hub holds its packages for `targets` whole, and
/// until then, or if one of them never comes, the newest it does hold whole.
fn standing(
    known: &[Manifest],
    newest: &Manifest,
    targets: &PackageTargets,
    held: &BTreeSet<Held>,
) -> Option<Manifest> {
    let ready = |m: &Manifest| {
        let wanted = for_targets(m, targets);
        !wanted.is_empty() && wanted.iter().all(|p| held.contains(p))
    };
    if ready(newest) {
        return Some(newest.clone());
    }
    let ceiling = newest.semver()?;
    known
        .iter()
        .filter(|m| m.semver().is_some_and(|v| v.pre.is_empty() == ceiling.pre.is_empty() && v < ceiling))
        .filter(|m| ready(m))
        .max_by_key(|m| m.semver())
        .cloned()
}

/// The packages of `manifest` the network's machines need, and how many
/// machines need each.
fn demand(hub: &Hub, manifest: &Manifest) -> BTreeMap<Held, usize> {
    let store = hub.store();
    let updates = hub.updates.lock().expect("never poisoned");
    let mut demand = BTreeMap::new();
    for member in &store.book.directory.members {
        if updates.last.get(&member.key).is_none_or(|last| !last.report.needs_network()) {
            continue;
        }
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
    let manifests: Vec<Manifest> = hub.updates.lock().expect("never poisoned").manifests().collect();
    let targets_of = |name: &MachineName| {
        let store = hub.store();
        store.book.directory.member(name).and_then(|member| store.hub.package_targets.get(&member.key)).cloned()
    };
    // A machine asked to update when it comes back is brought to the newest
    // release of the line and major version it was asked for that the
    // network has whole for it: it was asked to catch up, and a newer one
    // came while it was away.
    for (name, due) in network.due.iter_mut() {
        let channel = fastctx_types::update::Channel::of(&due.version);
        let major = fastctx_types::update::release(&due.version).map(|version| version.major);
        let through = needs_network(hub, name);
        let targets = targets_of(name).unwrap_or_default();
        // Never to a release without a package for what the machine runs.
        let packaged = |manifest: &Manifest| {
            targets.app.iter().chain(targets.kernel.iter()).all(|target| manifest.package(target).is_some())
        };
        let newest = manifests
            .iter()
            .filter(|manifest| {
                channel.takes(&manifest.version)
                    && manifest.newer_than(&due.version)
                    && manifest.semver().map(|version| version.major) == major
                    && packaged(manifest)
                    && (!through || for_targets(manifest, &targets).iter().all(|package| held.contains(package)))
            })
            .max_by_key(|manifest| manifest.semver());
        if let Some(newest) = newest {
            due.version = newest.version.clone();
            due.manifest = None;
        }
    }
    network.due.retain(|name, due| {
        if !needs_network(hub, name) {
            return true;
        }
        let manifest =
            due.manifest.clone().or_else(|| hub.updates.lock().expect("never poisoned").manifest(&due.version));
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
    let (cache, manifests, through_network) = {
        let updates = hub.updates.lock().expect("never poisoned");
        let through: Vec<_> =
            updates.last.iter().filter(|(_, last)| last.report.needs_network()).map(|(key, _)| *key).collect();
        (updates.cache.clone(), updates.manifests().collect::<Vec<_>>(), through)
    };
    let held = held(hub, &cache);
    let versions = cache.versions();
    let mut newest = std::collections::BTreeMap::<_, Manifest>::new();
    for manifest in &manifests {
        let channel = fastctx_types::update::Channel::of(&manifest.version);
        if newest.get(&channel).is_none_or(|old| manifest.semver() > old.semver()) {
            newest.insert(channel, manifest.clone());
        }
    }
    let mut keep: BTreeSet<String> = newest.values().map(|m| m.version.clone()).collect();
    // What a machine updating through the network is offered while a newer
    // release comes in stays until the newer one is whole for it.
    {
        let store = hub.store();
        for key in &through_network {
            let targets = store.hub.package_targets.get(key).cloned().unwrap_or_default();
            for top in newest.values() {
                keep.extend(standing(&manifests, top, &targets, &held).map(|m| m.version));
            }
        }
    }
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
    keep.extend(
        store
            .hub
            .repairs
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
    if hub.moving.frozen() {
        return;
    }
    let (policy, due) = {
        let store = hub.store();
        (
            store.hub.updates.clone(),
            store
                .hub
                .due
                .iter()
                .map(|due| due.due.version.clone())
                .chain(store.hub.repairs.iter().map(|due| due.due.version.clone()))
                .collect::<BTreeSet<_>>(),
        )
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
                .or_else(|| {
                    store
                        .hub
                        .repairs
                        .iter()
                        .find(|r| r.due.version == version)
                        .and_then(|r| store.book.directory.by_key(&r.key))
                        .map(|m| m.name.clone())
                })
        };
        if start && let Some(machine) = machine {
            let hub = hub.clone();
            tokio::spawn(async move {
                if hub.moving.frozen() {
                    return;
                }
                let failed = match prepare_update(&hub, &machine, &version).await {
                    Ok(()) => false,
                    Err(why) => {
                        hub.log.line(format_args!("preparing {version} for {machine}: {why}"));
                        super::view::problem(&hub, &version, None, Some(why));
                        true
                    }
                };
                super::tell(&hub, None);
                if failed {
                    again(hub).await;
                }
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
        let mut failed = false;
        for (package, _) in wanted {
            if hub.moving.frozen() {
                break;
            }
            hub.log.line(format_args!("preparing {} of {} before announcing it", package.target, package.version));
            let version = package.version.clone();
            let target = package.target.clone();
            let outcome = super::obtain::obtain(&hub, package, None).await;
            failed |= outcome.is_err();
            super::view::problem(&hub, &version, Some(&target), outcome.err());
            super::tell(&hub, None);
        }
        if failed {
            again(hub).await;
        }
    });
}

/// After a package or a release's manifest did not come, the hub tries
/// again a while later by itself: the machines waiting for it may have
/// nobody to ask for it again, and may reach nothing but the hub.
async fn again(hub: Arc<Hub>) {
    tokio::time::sleep(PREFETCH_AGAIN).await;
    prefetch(&hub);
}

const PREFETCH_AGAIN: Duration = Duration::from_secs(5 * 60);

/// The batch waits here before issuing its first remote update. Its durable
/// demand was recorded by the ordinary update-management operation already.
pub(crate) async fn prepare_update(hub: &Arc<Hub>, name: &MachineName, version: &str) -> Result<(), String> {
    let targets = {
        let store = hub.store();
        if hub.moving.frozen() {
            return Err("the hub is handing its records over; prepare this update at its new place".into());
        }
        let member = store.book.directory.member(name).ok_or_else(|| format!("there is no machine {name}"))?;
        if !store.hub.due.iter().any(|due| due.key == member.key && due.due.version == version)
            && !store.hub.repairs.iter().any(|due| due.key == member.key && due.due.version == version)
        {
            return Err(format!("no update to {version} is queued for {name}"));
        }
        store.hub.package_targets.get(&member.key).cloned().unwrap_or_default()
    };
    let (known, previous, cache) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.manifest(version), updates.looked.clone(), updates.cache.clone())
    };
    let manifest = match known {
        Some(manifest) => manifest,
        None => {
            let releases = if previous.releases.iter().any(|release| release.version == version) {
                previous.releases
            } else {
                let looked = super::discovery::check(hub, true).await;
                if let Some(why) = &looked.why {
                    hub.log.line(format_args!("looking for {version}: {why}"));
                }
                looked.releases
            };
            fastctx_update::github::manifest_of(&releases, version).await?
        }
    };
    cache.keep_manifest(&manifest)?;
    hub.updates.lock().expect("never poisoned").catalog.insert(manifest.version.clone(), manifest.clone());
    keep_prepared_manifest(hub, &manifest)?;
    if !needs_network(hub, name) {
        return Ok(());
    }
    for package in for_targets(&manifest, &targets) {
        super::obtain::obtain(hub, package, None).await?;
    }
    Ok(())
}

fn keep_prepared_manifest(hub: &Hub, manifest: &Manifest) -> Result<(), String> {
    let mut store = hub.store();
    // A fetch begun before the move can finish after its final snapshot
    // (2026-10-29). The store lock also orders this check with freeze's flush.
    if hub.moving.frozen() {
        return Err("the hub is handing its records over; prepare this update at its new place".into());
    }
    for due in &mut store.hub.due {
        if due.due.version == manifest.version {
            std::sync::Arc::make_mut(due).due.manifest = Some(manifest.clone());
        }
    }
    for due in &mut store.hub.repairs {
        if due.due.version == manifest.version {
            std::sync::Arc::make_mut(due).due.manifest = Some(manifest.clone());
        }
    }
    store.save_hub().map_err(|e| e.to_string())
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
