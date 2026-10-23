//! The network's versions (`world-upgrade.md`, 怎样知道有新版本、从哪里拿):
//! what each machine runs and knows of; the newest release on each line
//! that any machine or the hub itself found, told to every machine with the
//! network's policy, the hub's own version and the updates kept for
//! machines away; and the packages the hub brings in for machines that
//! cannot reach GitHub, from GitHub itself or from a machine that can or
//! holds one, kept here until a newer version replaces them. Of the
//! machines' reports only what each ran is kept past its session, in
//! memory: each tells the rest again.

mod history;
mod view;
mod repair;
pub(crate) use view::request as view_request;
mod attempt;
mod cleanup;

use attempt::{Attempt, Race};
mod discovery;
mod inventory;
mod obtain;
mod receipt_writer;
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

pub(crate) use obtain::{ask, bring_in, progress, unfetched};
pub(crate) use transfer::{deliver, serve};

use crate::{Hub, Out};

/// One of the archives a hub cannot pack itself (`HELD_ARCHIVES`), of
/// `version`, as the hub holds it among its update packages, checked when
/// it was brought in: where it is.
pub(crate) fn held_archive(hub: &Hub, version: &str, archive: &str) -> Option<std::path::PathBuf> {
    let package = fastctx_types::kernel::held_package(archive)?;
    let cache = hub.updates.lock().expect("never poisoned").cache.clone();
    cache.get(version, package)
}

/// The SHA-256 of such an archive the hub holds, for the line that
/// downloads it; none while it does not, and when `bring` it is brought in
/// then, so that a line made a few minutes later finds it.
pub(crate) async fn held_digest(hub: &Arc<Hub>, version: &str, archive: &str, bring: bool) -> Option<String> {
    let Some(path) = held_archive(hub, version, archive) else {
        if bring && let Some(package) = fastctx_types::kernel::held_package(archive) {
            bring_in(hub, version.to_owned(), package.to_owned());
        }
        return None;
    };
    let hashed = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        use sha2::Digest as _;
        let mut file = std::fs::File::open(path)?;
        let mut hash = sha2::Sha256::new();
        std::io::copy(&mut file, &mut hash)?;
        Ok(format!("{:x}", hash.finalize()))
    })
    .await;
    match hashed {
        Ok(Ok(digest)) => Some(digest),
        Ok(Err(error)) => {
            hub.log.line(format_args!("reading the Windows archive of {version}: {error}"));
            None
        }
        Err(error) => {
            hub.log.line(format_args!("reading the Windows archive of {version}: {error}"));
            None
        }
    }
}

pub(crate) struct Updates {
    /// Each machine's latest report, with the session it came on.
    reports: HashMap<MachineName, (u64, Report)>,
    /// What each machine ran when it last reported, online or away.
    last: history::History,
    history_writer: tokio::sync::watch::Sender<history::History>,
    catalog: BTreeMap<String, Manifest>,
    /// What the hub found when it last looked on GitHub.
    looked: Looked,
    check: Arc<tokio::sync::Mutex<()>>,
    /// What every machine was last told.
    told: HashMap<MachineName, Network>,
    /// The packages being brought in, and who waits for each.
    obtaining: HashMap<Held, obtain::Obtaining>,
    requests: HashMap<(MachineName, u64, u64), (Held, bool)>,
    serving: HashMap<Held, usize>,
    /// The requests machines were sent to fetch a package (`Down::Fetch`).
    fetches: HashMap<u64, Fetching>,
    cache: Cache,
    /// Places for the packages handed to machines at once, so updates leave
    /// the hub's line to everything else that passes on it.
    handing: Arc<tokio::sync::Semaphore>,
    /// The one package coming in at a time: a release's packages come one
    /// after another, each at the whole of the ceiling (`obtain::bring`).
    coming: Arc<tokio::sync::Mutex<()>>,
    /// The ceiling every byte the hub takes in shares, from GitHub or from
    /// its machines (`fastctx_update::throttle`).
    intake: Arc<fastctx_update::throttle::Throttle>,
    /// How fast the hub's own last download from GitHub went, in bytes a
    /// second, beside its machines' (`obtain::next`).
    own_speed: Option<u64>,
    /// The packages placed by hand among the releases that were checked
    /// against their signatures, as each file was then (`placed`).
    placed: HashMap<Held, (u64, Option<std::time::SystemTime>)>,
    /// What was brought in before anyone asked, and when (`prefetch`).
    prefetched: HashMap<Held, std::time::Instant>,
}

/// How many packages the hub hands to machines at once; the rest wait.
const HANDED_AT_ONCE: usize = 4;

struct Fetching {
    helper: MachineName,
    generation: u64,
    held: Held,
    done: Option<oneshot::Sender<Result<(), String>>>,
    attempt: Arc<Attempt>,
    progress: bool,
}

impl Updates {
    /// The hub's record of updates; what tries of an earlier run of the hub
    /// left of packages coming is deleted, and said in `left`.
    pub fn new(dir: &Path, due: &[crate::store::DueFor], left: &mut Vec<String>) -> Result<Self, String> {
        let cache = Cache::new(dir.join("updates"));
        left.extend(
            cache.clear_attempts().into_iter().map(|failed| format!("deleting a package left coming: {failed}")),
        );
        let (last, history_writer) = history::open(cache.root())?;
        let looked = discovery::read(cache.root()).unwrap_or_else(|why| {
            left.push(format!("reading discovered releases: {why}"));
            Looked::default()
        });
        let catalog = cache
            .manifests()
            .into_iter()
            .chain(last.values().flat_map(|l| l.report.found.iter().cloned()))
            .chain(due.iter().filter_map(|d| d.due.manifest.clone()))
            .map(|m| (m.version.clone(), m))
            .collect();
        Ok(Self {
            reports: HashMap::new(),
            last,
            history_writer,
            catalog,
            looked,
            check: Arc::new(tokio::sync::Mutex::new(())),
            told: HashMap::new(),
            obtaining: HashMap::new(),
            requests: HashMap::new(),
            serving: HashMap::new(),
            fetches: HashMap::new(),
            cache,
            handing: Arc::new(tokio::sync::Semaphore::new(HANDED_AT_ONCE)),
            coming: Arc::new(tokio::sync::Mutex::new(())),
            intake: Arc::new(fastctx_update::throttle::Throttle::new(fastctx_types::update::INTAKE)),
            own_speed: None,
            placed: HashMap::new(),
            prefetched: HashMap::new(),
        })
    }

    /// Every release the hub knows the manifest of.
    fn manifests(&self) -> impl Iterator<Item = Manifest> + '_ {
        self.reports
            .values()
            .flat_map(|(_, report)| report.found.iter().cloned())
            .chain(self.looked.stable.iter().cloned())
            .chain(self.looked.beta.iter().cloned())
            .chain(self.looked.newer.iter().cloned())
            .chain(self.catalog.values().cloned())
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
        let machines = self.reports.iter().map(|(name, (_, report))| (name.clone(), running(report))).collect();
        // Devices of 1.0.1 beta 1 and 2 follow the policy's channel: the beta
        // line takes every release, as the line of what they run does.
        let policy = Policy { channel: Channel::Beta, ..policy };
        Network {
            stable,
            beta,
            policy,
            machines,
            hub: Some(env!("CARGO_PKG_VERSION").to_string()),
            due,
            updates_view: true,
            repair_supported: true,
            repairs: BTreeMap::new(),
        }
    }

    /// The manifest of the release `version`, when the hub knows it.
    pub(crate) fn manifest(&self, version: &str) -> Option<Manifest> {
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
        repair_supported: report.repair_supported,
    }
}

/// What `name` ran when it last told its versions, while the hub ran.
pub(crate) fn running_of(hub: &Hub, key: &fastctx_net::sign::PublicKey) -> Option<Running> {
    hub.updates.lock().expect("never poisoned").last.get(key).map(|last| running(&last.report))
}

/// What the hub's settings say of updates: the policy, and the updates kept
/// for machines, under their names now.
fn kept(hub: &Hub) -> (Policy, BTreeMap<MachineName, Due>) {
    let store = hub.store();
    let due = store
        .hub
        .due
        .iter()
        .filter_map(|kept| {
            store.book.directory.by_key(&kept.key).map(|member| {
                let mut due = kept.due.clone();
                due.manifest = None;
                due.problem = None;
                (member.name.clone(), due)
            })
        })
        .collect();
    (store.hub.updates.clone(), due)
}

/// The network's versions as every machine is told them now.
pub(crate) fn network(hub: &Hub) -> Network {
    let (policy, due) = kept(hub);
    let directory = hub.store().book.directory.clone();
    let mut network = {
        let updates = hub.updates.lock().expect("never poisoned");
        let mut network = updates.network(policy, due);
        for (key, last) in &updates.last {
            if let Some(member) = directory.by_key(key) {
                network
                    .machines
                    .entry(member.name.clone())
                    .or_insert_with(|| Running { away: true, ..running(&last.report) });
            }
        }
        network
    };
    network.machines.retain(|name, _| directory.member(name).is_some());
    network.repairs = hub.store().hub.repairs.iter().filter_map(|kept| directory.by_key(&kept.key).map(|member| {
        let mut due = kept.due.clone();
        due.manifest = None;
        (member.name.clone(), due)
    })).collect();
    inventory::ready_network(hub, network)
}

/// `name` told the hub its versions on its session `generation`.
pub(crate) fn heard(hub: &Arc<Hub>, name: &MachineName, generation: u64, mut report: Report) {
    repair::merge_legacy_report(hub, name, &mut report);
    if let Some(result) = &report.repair { repair::settle(hub, name, result); }
    inventory::targets(hub, name, &report);
    let (kernel, app, asked) =
        (report.kernel.clone(), report.app.as_ref().map(|a| a.version.clone()), report.asked.clone());
    let directory = hub.store().book.directory.clone();
    let Some(key) = directory.member(name).map(|m| m.key) else { return };
    let planning_changed = {
        let mut updates = hub.updates.lock().expect("never poisoned");
        let changed = updates.reports.get(name).is_none_or(|(_, previous)| {
            let mut previous = previous.clone();
            previous.stage = report.stage.clone();
            previous.checking = report.checking;
            previous != report
        });
        if changed {
            for manifest in &report.found {
                updates.catalog.insert(manifest.version.clone(), manifest.clone());
            }
        }
        updates.last.insert(key, history::LastReport { at: jiff::Timestamp::now(), report: report.clone() });
        updates.last.retain(|key, _| directory.by_key(key).is_some());
        updates.history_writer.send_replace(updates.last.clone());
        updates.reports.insert(name.clone(), (generation, report));
        changed
    };
    if let Err(error) = crate::hub_machine::settle(hub) {
        hub.log.line(format_args!("revoking shares of the hub machine: {error}"));
    }
    if planning_changed {
        hub.directory_changed.notify_one();
        settle(hub, name, &kernel, app.as_deref(), asked);
    }
    let mut due_changed = false;
    let stage = hub.updates.lock().expect("never poisoned").last.get(&key).map(|last| last.report.stage.clone());
    if let Some(fastctx_types::update::Stage::Failed { version, problem }) = stage {
        let mut store = hub.store();
        // A machine brought to a newer release than it was asked for fails
        // the arrangement all the same.
        let reached = fastctx_types::update::release(&version);
        if let Some(due) = store
            .hub
            .due
            .iter_mut()
            .find(|due| due.key == key && reached.is_some() && reached >= fastctx_types::update::release(&due.due.version))
            && due.due.problem.as_ref() != Some(&problem)
        {
            due.due.problem = Some(problem);
            due_changed = true;
            if let Err(error) = store.save_hub() {
                hub.log.line(format_args!("keeping the failed arrangement for {name}: {error}"));
            }
        }
    }
    // Progress updates the durable account view without repeating discovery
    // or rebuilding every member's version notice on each byte sample.
    if planning_changed {
        prefetch(hub);
    }
    if planning_changed || due_changed {
        tell(hub, None);
    }
}

/// `name` runs `kernel` now: an update kept for it that this release is or
/// passes was made, and one it made as someone asked goes into the machine
/// log, once.
fn settle(hub: &Hub, name: &MachineName, kernel: &str, app: Option<&str>, asked: Option<Asked>) {
    let mut store = hub.store();
    let Some(key) = store.book.directory.member(name).map(|member| member.key) else { return };
    let mut runs = fastctx_types::update::release(kernel);
    if let Some(app) = app {
        runs = runs.min(fastctx_types::update::release(app));
    }
    let before = store.hub.due.len();
    store.hub.due.retain(|kept| kept.key != key || fastctx_types::update::release(&kept.due.version) > runs);
    if store.hub.due.len() != before {
        if let Err(error) = store.save_hub() {
            hub.log.line(format_args!("keeping that {name} made the update kept for it: {error}"));
        }
        drop(store);
        prune(hub);
        store = hub.store();
    }
    let Some(asked) = asked.filter(|asked| asked.from != kernel) else { return };
    if store.book.machine_log.updated(&key, &asked.from, kernel) {
        return;
    }
    let directory = store.book.directory.clone();
    let actor = Actor { account: None, authenticated: true, member: Some(name.clone()), peer: None };
    let action = Action::Updated { machine: name.clone(), from: asked.from, to: kernel.to_string(), by: asked.by };
    match crate::machine_log::keep(&mut store, &directory, actor, action, Outcome::Done) {
        Ok(()) => {
            hub.machine_log_changed.notify_one();
            hub.directory_changed.notify_one();
        }
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
        updates.requests.retain(|(member, session, _), _| member != name || *session != generation);
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
    let network = network(hub);
    let directory = hub.store().book.directory.clone();
    let recipients: Vec<_> = hub
        .sessions()
        .iter()
        .filter(|(name, session)| session.versions && only.is_none_or(|only| only == *name))
        .map(|(name, session)| (name.clone(), session.out.clone()))
        .collect();
    for (name, out) in recipients {
        let name = &name;
        let mut cut = network.clone();
        let accounts = directory.member(name).map(|m| &m.logins);
        let visible = |machine: &MachineName| {
            directory.same(name, machine)
                || accounts.is_some_and(|accounts| accounts.iter().any(|a| directory.may_update(a, machine)))
        };
        cut.machines.retain(|machine, _| visible(machine));
        cut.due.retain(|machine, _| visible(machine));
        cut.repairs.retain(|machine, _| visible(machine));
        if let Some(due) = cut.repairs.get_mut(directory.canonical(name)) {
            due.manifest = directory.member(name).and_then(|member| hub.store().hub.repairs.iter()
                .find(|kept| kept.key == member.key && kept.due.id == due.id).and_then(|kept| kept.due.manifest.clone()));
        }
        // Only the executor needs the full manifest. Repeating it for every
        // managed machine exceeds the control-frame limit (2026-10-20).
        if let Some(due) = cut.due.get_mut(directory.canonical(name)) {
            let store = hub.store();
            if let Some(details) = directory.member(name).and_then(|member| {
                store.hub.due.iter().find(|kept| kept.key == member.key && kept.due.version == due.version)
            }) {
                due.manifest = details.due.manifest.clone();
                due.problem = details.due.problem.clone();
            }
        }
        inventory::offer_to(hub, name, &mut cut);
        let mut updates = hub.updates.lock().expect("never poisoned");
        if only.is_none() && updates.told.get(name) == Some(&cut) {
            continue;
        }
        updates.told.insert(name.clone(), cut.clone());
        drop(updates);
        let _ = out.send(Out::Frame(Down::Versions { network: Box::new(cut) }));
    }
}

/// Looks on GitHub now and every few hours, while the hub runs.
pub(crate) async fn look_every(hub: Weak<Hub>) {
    // A hub just started lets its machines connect first.
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        let Some(current) = hub.upgrade() else { return };
        discovery::check(&current, false).await;
        drop(current);
        // An on-demand check can shorten the next deadline while this loop sleeps.
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

/// What `health.json` says of the packages the hub keeps for the network
/// (`world-network.md`, 中枢看着自己): which it holds, which it brings in
/// now, and which machines fetch them for it.
pub(crate) fn health(hub: &Hub) -> serde_json::Value {
    let named = |held: &Held| format!("{} {}", held.target, held.version);
    let (cache, coming, helping) = {
        let updates = hub.updates.lock().expect("never poisoned");
        let coming: Vec<String> = updates.obtaining.keys().map(named).collect();
        let helping: Vec<serde_json::Value> = updates
            .fetches
            .values()
            .map(|fetching| serde_json::json!({ "member": fetching.helper.to_string(), "package": named(&fetching.held) }))
            .collect();
        (updates.cache.clone(), coming, helping)
    };
    let mut held = inventory::held(hub, &cache);
    held.extend(hub.releases.held());
    let held: Vec<String> = held.iter().map(named).collect();
    serde_json::json!({ "held": held, "coming": coming, "fetching_for_it": helping })
}

pub(crate) use inventory::{done, prefetch, prepare_update, prune};

/// The package `held` placed by hand among the releases as the archive it
/// is, checked against the package's signature once per state of the file:
/// handed out from there, never kept a second time among the update
/// packages. Its file, opened, and size.
pub(super) fn placed(hub: &Hub, held: &Held) -> Option<(std::fs::File, u64)> {
    use std::io::Seek as _;
    let archive = fastctx_types::kernel::archive_of(&held.target)?;
    let mut file = hub.releases.placed(&held.version, archive).ok()?;
    let metadata = file.metadata().ok()?;
    let mark = (metadata.len(), metadata.modified().ok());
    let signature = {
        let updates = hub.updates.lock().expect("never poisoned");
        if updates.placed.get(held) == Some(&mark) {
            return Some((file, mark.0));
        }
        updates.manifest(&held.version)?.package(&held.target)?.signature.clone()
    };
    if let Err(why) = fastctx_update::verify::reader(&mut file, &signature, &held.version) {
        hub.log.line(format_args!("{archive} placed for {} is not its package: {why}", held.version));
        return None;
    }
    file.rewind().ok()?;
    hub.updates.lock().expect("never poisoned").placed.insert(held.clone(), mark);
    Some((file, mark.0))
}
