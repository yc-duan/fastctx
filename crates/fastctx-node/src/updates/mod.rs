//! New versions of Fastctx on this machine (`world-upgrade.md`, 新版本自己往后
//! 怎样升级): the device's own choices, a look on GitHub every few hours,
//! what this machine tells each of its hubs and hears back, the packages it
//! brings here for the interface or for other machines, and its own World
//! program updated from the newest release when no application carries it,
//! or the application's release taken as another machine asks
//! (`application`). Everything kept is under `<root>/updates`.

mod application;

pub(crate) use application::Open;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_install::renewal::{read_json, write_json};
use fastctx_types::AccountName;
use fastctx_types::update::{
    App, Asked, Channel, Failed, Fetch, Fetching, Kind, Looked, Manifest, Network, Renewing, Report, Settings, State,
    UpdateCall,
};
use fastctx_update::Cache;

use crate::Node;

/// How often GitHub is looked at.
const LOOK_EVERY: Duration = Duration::from_secs(6 * 3600);

/// How long a release this machine tried to take by itself waits before it
/// is tried again.
const TRY_AGAIN: Duration = Duration::from_secs(3600);

#[derive(Default)]
pub(crate) struct Updates {
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    settings: Settings,
    looked: Looked,
    /// The release the owner chose to update to, when not the newest.
    chosen: Option<Manifest>,
    app: Option<App>,
    /// Where the application keeps the command-line programs it carries.
    carried: Option<PathBuf>,
    /// The World program the application carries.
    program: Option<PathBuf>,
    /// The release someone asked from elsewhere that the application take.
    take: Option<String>,
    fetching: Option<Fetching>,
    failed: Option<Failed>,
    /// The release last tried by itself, and when.
    tried: Option<(String, std::time::Instant)>,
    /// How fast this machine's last download from GitHub went, in bytes a
    /// second (`GITHUB_SPEED`).
    github_speed: Option<u64>,
}

/// Where the speed of the last download from GitHub is kept, for the hub
/// to choose who fetches for the network (`fastctx_types::update::Report`).
const GITHUB_SPEED: &str = "github-speed.json";

/// Who asked this machine from elsewhere for the World program it changes
/// to, and through which hub: told that hub once it runs it (`Asked`).
#[derive(serde::Serialize, serde::Deserialize)]
struct AskedFile {
    by: AccountName,
    from: String,
    hub: fastctx_net::ids::HubId,
}

/// Who asks for an update made here, when not the user at this machine.
pub(crate) struct Asker {
    pub by: AccountName,
    pub hub: fastctx_net::ids::HubId,
}

/// What the interface here keeps of itself for the node.
#[derive(serde::Serialize, serde::Deserialize)]
struct AppFile {
    app: App,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    carried: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    program: Option<PathBuf>,
}

fn dir(node: &Node) -> PathBuf {
    node.root.join("updates")
}

/// Keeps `value` as `name` in the updates' directory.
fn keep(node: &Node, name: &str, value: &impl serde::Serialize) -> std::io::Result<()> {
    let dir = dir(node);
    std::fs::create_dir_all(&dir)?;
    write_json(&dir.join(name), value)
}

fn cache(node: &Node) -> Cache {
    Cache::new(dir(node).join("packages"))
}

/// Reads what was kept, and looks on GitHub from now on.
pub(crate) fn start(node: &Arc<Node>) {
    let dir = dir(node);
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        held.settings = read_json(&dir.join("settings.json")).ok().flatten().unwrap_or_default();
        // Turned on is always its owner's doing: 1.0.1 beta 1 and 2 kept no
        // `chosen` and took nothing by themselves unless told to.
        if held.settings.auto {
            held.settings.chosen = true;
        }
        held.looked = read_json(&dir.join("looked.json")).ok().flatten().unwrap_or_default();
        held.github_speed = read_json(&dir.join(GITHUB_SPEED)).ok().flatten();
        if let Ok(Some(AppFile { app, carried, program })) = read_json::<AppFile>(&dir.join("app.json")) {
            held.app = Some(app);
            held.carried = carried;
            held.program = program;
        }
        if let Ok(Some(take)) = read_json::<application::Take>(&dir.join("take.json")) {
            held.take = Some(take.version);
        }
    }
    report(node);
    if !fastctx_platform::follows_releases() {
        return;
    }
    let node = node.clone();
    tokio::spawn(async move {
        // The first look waits for the node to settle, and for the next due
        // time when one was made lately.
        let since = node.updates.held.lock().expect("never poisoned").looked.at;
        let gone = since.map(|at| jiff::Timestamp::now().duration_since(at).unsigned_abs());
        let mut wait = gone.map_or(Duration::ZERO, |gone| LOOK_EVERY.saturating_sub(gone)).max(Duration::from_secs(90));
        loop {
            tokio::time::sleep(wait).await;
            look(&node).await;
            auto(&node);
            wait = LOOK_EVERY;
        }
    });
}

/// Looks on GitHub now, and keeps what it found.
async fn look(node: &Arc<Node>) {
    // What can be taken here is newer than the application, when one
    // carries the World program, or than the program itself.
    let running = application(node).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let looked = fastctx_update::github::look(Some(&running)).await;
    if let Some(why) = &looked.why {
        node.log.line(format_args!("looking for new versions on GitHub: {why}"));
    }
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        // A failed look keeps what the last one found.
        if looked.why.is_none() {
            held.looked = looked;
        } else {
            held.looked.at = looked.at;
            held.looked.github = looked.github;
            held.looked.why = looked.why;
        }
        if let Err(error) = keep(node, "looked.json", &held.looked) {
            node.log.line(format_args!("keeping what was found of new versions: {error}"));
        }
    }
    report(node);
}

/// What the hubs this machine is on tell of versions, as one: the newest
/// release on each line any of them knows, the policy of the one that asks
/// most, the oldest hub, and the updates any of them keeps for machines away.
fn network(node: &Node) -> Option<Network> {
    let mut merged: Option<Network> = None;
    for network in node.links().iter().filter_map(|link| link.versions()) {
        let Some(kept) = &mut merged else {
            merged = Some(network);
            continue;
        };
        kept.stable = fastctx_types::update::newest(kept.stable.as_ref(), network.stable.as_ref()).cloned();
        kept.beta = fastctx_types::update::newest(kept.beta.as_ref(), network.beta.as_ref()).cloned();
        kept.policy.minimum = higher(kept.policy.minimum.take(), network.policy.minimum);
        kept.policy.managed_auto &= network.policy.managed_auto;
        // A hub that does not say what it runs takes nothing from what
        // another says: the oldest a hub is known to run still holds.
        kept.hub = match (kept.hub.take(), network.hub) {
            (Some(a), Some(b)) => {
                Some(if fastctx_types::update::release(&b) < fastctx_types::update::release(&a) { b } else { a })
            }
            (known, unknown) => known.or(unknown),
        };
        kept.machines.extend(network.machines);
        kept.due.extend(network.due);
    }
    merged
}

/// What kind of device this machine is for its updates.
fn kind(node: &Node, held: &Held) -> Kind {
    if crate::managed::is_managed(node) {
        Kind::Managed
    } else if held.app.is_some() {
        Kind::App
    } else {
        Kind::Headless
    }
}

/// The newest major version every hub this machine is on runs, which a
/// release taken by itself does not pass (`world-upgrade.md`, 自动更新): none
/// outside any network, or where the machine itself keeps a hub, which would
/// otherwise never move to a new one. Where no hub says what it runs, the
/// major version running here: an older hub does not say, and is not passed.
fn hub_major(node: &Node, network: Option<&Network>) -> Option<u64> {
    let hubs_here = fastctx_install::service::registered_hubs(&node.root).is_ok_and(|hubs| !hubs.is_empty());
    if hubs_here || node.links().is_empty() {
        return None;
    }
    let said = network.and_then(|network| network.hub.as_deref()).and_then(fastctx_types::update::release);
    said.or_else(|| fastctx_types::update::release(env!("CARGO_PKG_VERSION"))).map(|version| version.major)
}

/// The higher of two minimum versions, either of which may be missing.
fn higher(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => {
            Some(if fastctx_types::update::release(&b) > fastctx_types::update::release(&a) { b } else { a })
        }
        (a, b) => a.or(b),
    }
}

/// How updates stand here now. What is offered is the release the owner
/// chose, while it is newer than what runs here, or else the newest on the
/// line of what runs here (`world-upgrade.md`, 正式版与测试版).
pub(crate) fn state(node: &Node) -> State {
    let network = network(node);
    let held = node.updates.held.lock().expect("never poisoned");
    let channel = Channel::of(env!("CARGO_PKG_VERSION"));
    let kept = cache(node).manifests();
    let newest = [held.looked.on(channel), network.as_ref().and_then(|network| network.on(channel))]
        .into_iter()
        .chain(kept.iter().filter(|manifest| channel.takes(&manifest.version)).map(Some))
        .fold(None, fastctx_types::update::newest)
        .cloned();
    let chosen = held.chosen.clone().filter(|chosen| chosen.newer_than(env!("CARGO_PKG_VERSION")));
    let offer = chosen.clone().or(newest);
    let auto = held.settings.auto_for(kind(node, &held), network.as_ref().map(|network| &network.policy));
    let hub_major = hub_major(node, network.as_ref());
    State {
        running: Some(env!("CARGO_PKG_VERSION").to_string()),
        settings: held.settings.clone(),
        channel,
        auto,
        chosen,
        hub_major,
        looked: held.looked.clone(),
        network,
        offer,
        fetching: held.fetching.clone(),
        failed: held.failed.clone(),
        kept: cache(node).kept(),
        app: held.app.clone(),
        kernel_target: fastctx_update::kernel::target(),
        renewing: renewing(node),
        take: held.take.clone(),
        open: application::open(),
    }
}

/// The change of World program under way here, or the last one not taken.
fn renewing(node: &Node) -> Option<Renewing> {
    let plan = fastctx_install::renewal::read(&node.root).ok().flatten()?;
    let version = plan.candidate.build.version;
    let done = plan.phase == fastctx_types::renewal::Phase::Complete;
    (!done && version != env!("CARGO_PKG_VERSION")).then_some(Renewing {
        version,
        phase: plan.phase,
        error: plan.error,
    })
}

/// The version of the application that carries this machine's World
/// program, when one does.
pub(crate) fn application(node: &Node) -> Option<String> {
    node.updates.held.lock().expect("never poisoned").app.as_ref().map(|app| app.version.clone())
}

/// The hubs this machine runs, as each hub's settings name it.
fn hubs_here(node: &Node) -> Vec<fastctx_net::ids::HubId> {
    #[derive(serde::Deserialize)]
    struct Named {
        id: fastctx_net::ids::HubId,
    }
    let directories = fastctx_install::service::registered_hubs(&node.root).unwrap_or_default();
    directories
        .iter()
        .filter_map(|directory| read_json::<Named>(&directory.join("hub.json")).ok().flatten().map(|hub| hub.id))
        .collect()
}

/// What this machine tells its hubs of versions now.
pub(crate) fn report(node: &Node) {
    if !fastctx_platform::follows_releases() {
        return;
    }
    let held = node.updates.held.lock().expect("never poisoned");
    let cache = cache(node);
    let mut found: Vec<Manifest> =
        [held.looked.stable.clone(), held.looked.beta.clone()].into_iter().flatten().collect();
    // A release whose package is kept here is known here too.
    found.extend(cache.manifests().into_iter().take(1));
    found.sort_by_key(|manifest| std::cmp::Reverse(manifest.semver()));
    found.dedup_by(|a, b| a.version == b.version);
    let mut holds = cache.held();
    if let (Some(app), Some(carried)) = (&held.app, &held.carried) {
        for triple in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
            if carried.join(format!("fastctx-{triple}.tar.gz")).is_file() {
                holds.push(fastctx_types::update::Held {
                    version: app.version.clone(),
                    target: format!("fastctx-{triple}"),
                });
            }
        }
    }
    let report = Report {
        kernel: env!("CARGO_PKG_VERSION").to_string(),
        app: held.app.clone(),
        github: held.looked.github,
        found,
        relays: held.settings.relays,
        holds,
        asked: None,
        hub_here: false,
        github_speed: held.github_speed,
        target: Some(fastctx_update::kernel::target()),
    };
    drop(held);
    let hubs = hubs_here(node);
    let asked = read_json::<AskedFile>(&dir(node).join("asked.json"))
        .ok()
        .flatten()
        .filter(|asked| asked.from != env!("CARGO_PKG_VERSION"));
    for link in node.links() {
        let mut report = report.clone();
        let hub = link.membership().id;
        if let Some(asked) = asked.as_ref().filter(|asked| asked.hub == hub) {
            report.asked = Some(Asked { by: asked.by.clone(), from: asked.from.clone() });
        }
        report.hub_here = hubs.contains(&hub);
        link.report_versions(report);
    }
}

/// An interface here asks.
pub(crate) async fn call(node: &Arc<Node>, call: UpdateCall) -> Result<State, String> {
    match call {
        UpdateCall::State => {}
        UpdateCall::Look => look(node).await,
        UpdateCall::Set { settings } => {
            keep(node, "settings.json", &settings).map_err(|error| error.to_string())?;
            node.updates.held.lock().expect("never poisoned").settings = settings;
            report(node);
            auto(node);
        }
        UpdateCall::App { app, carried, program } => {
            application::took(node, &app.version);
            let file =
                AppFile { app: app.clone(), carried: carried.map(PathBuf::from), program: program.map(PathBuf::from) };
            let mut held = node.updates.held.lock().expect("never poisoned");
            if held.app.as_ref() != Some(&app) || held.carried != file.carried || held.program != file.program {
                keep(node, "app.json", &file).map_err(|error| error.to_string())?;
                held.app = Some(file.app);
                held.carried = file.carried;
                held.program = file.program;
                drop(held);
                report(node);
            }
        }
        UpdateCall::Fetch { version, target } => fetch(node, version, target)?,
        UpdateCall::Kernel { version } => kernel(node, version.as_deref(), None).await?,
        UpdateCall::Choose { version } => {
            let chosen = match version {
                Some(version) => Some(manifest_of(node, &version).await?),
                None => None,
            };
            node.updates.held.lock().expect("never poisoned").chosen = chosen;
        }
    }
    Ok(state(node))
}

/// The manifest of the release `version`: as this machine or its hubs know
/// it, or as GitHub has it.
async fn manifest_of(node: &Arc<Node>, version: &str) -> Result<Manifest, String> {
    let state = state(node);
    let known = [state.offer.as_ref(), state.chosen.as_ref(), state.looked.stable.as_ref(), state.looked.beta.as_ref()]
        .into_iter()
        .flatten()
        .chain(state.looked.newer.iter())
        .chain(state.network.as_ref().into_iter().flat_map(|network| network.stable.iter().chain(network.beta.iter())))
        .find(|manifest| manifest.version == version)
        .cloned()
        .or_else(|| cache(node).manifests().into_iter().find(|manifest| manifest.version == version));
    match known {
        Some(manifest) => Ok(manifest),
        None => fastctx_update::github::manifest_of(&state.looked.releases, version).await,
    }
}

/// Brings the package `target` of `version` here, in the background.
fn fetch(node: &Arc<Node>, version: String, target: String) -> Result<(), String> {
    let state = state(node);
    let manifest =
        [state.offer.as_ref(), state.chosen.as_ref(), state.looked.stable.as_ref(), state.looked.beta.as_ref()]
            .into_iter()
            .flatten()
            .chain(state.looked.newer.iter())
            .chain(
                state.network.as_ref().into_iter().flat_map(|network| network.stable.iter().chain(network.beta.iter())),
            )
            .find(|manifest| manifest.version == version)
            .cloned()
            .ok_or_else(|| format!("no release {version} is known here"))?;
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        if held.fetching.as_ref().is_some_and(|fetching| fetching.version == version && fetching.target == target) {
            return Ok(());
        }
        held.failed = None;
        held.fetching = Some(Fetching {
            version: version.clone(),
            target: target.clone(),
            via: fastctx_types::update::Via::Github,
            received: 0,
            total: None,
        });
    }
    let node = node.clone();
    tokio::spawn(async move {
        let outcome = bring(&node, &manifest, &target).await;
        let mut held = node.updates.held.lock().expect("never poisoned");
        held.fetching = None;
        if let Err(why) = outcome {
            node.log.line(format_args!("bringing {target} of {version} here: {why}"));
            held.failed = Some(Failed { version, target, why });
        }
        drop(held);
        report(&node);
    });
    Ok(())
}

/// Brings a package here by the first hub that hands them out, or GitHub
/// (`fastctx_update::obtain`), with its progress kept for the interface;
/// older versions go once it is.
async fn bring(node: &Arc<Node>, manifest: &Manifest, target: &str) -> Result<PathBuf, String> {
    let cache = cache(node);
    let links = node.links();
    let link = links.iter().find(|link| link.keeps_versions()).map(|link| link.as_ref());
    let progressing = node.clone();
    let mut progress = move |progress: fastctx_update::Progress| {
        if let Some(fetching) = &mut progressing.updates.held.lock().expect("never poisoned").fetching {
            fetching.via = progress.via;
            fetching.received = progress.received;
            fetching.total = progress.total;
        }
    };
    let brought = fastctx_update::obtain(&cache, manifest, target, link, &mut progress).await?;
    cache.prune(&[manifest.version.as_str()]);
    if let Some(speed) = brought.github_speed {
        github_speed(node, speed);
    }
    Ok(brought.path)
}

/// Keeps how fast this machine's download from GitHub went, and tells its
/// hubs: the fastest is asked first to fetch for the network.
fn github_speed(node: &Arc<Node>, speed: u64) {
    node.updates.held.lock().expect("never poisoned").github_speed = Some(speed);
    if let Err(error) = keep(node, GITHUB_SPEED, &speed) {
        node.log.line(format_args!("keeping the speed of GitHub: {error}"));
    }
    report(node);
}

/// The hub asks this machine for a package for others: one held here, one
/// the application here carries, or, when this machine fetches for others,
/// one from GitHub.
pub(crate) async fn relay(node: &Arc<Node>, fetch: Fetch) -> Result<PathBuf, String> {
    let cache = cache(node);
    if let Some(path) = cache.get(&fetch.version, &fetch.target) {
        return Ok(path);
    }
    let (relays, carried) = {
        let held = node.updates.held.lock().expect("never poisoned");
        let carried = held
            .carried
            .clone()
            .filter(|_| held.app.as_ref().is_some_and(|app| app.version == fetch.version))
            .map(|dir| dir.join(format!("{}.tar.gz", fetch.target)))
            .filter(|path| path.is_file());
        (held.settings.relays, carried)
    };
    // A file of the hub's request alone: this machine may bring the same
    // package for itself meanwhile.
    let incoming = cache.attempt(&fetch.version, &fetch.target, fetch.id)?;
    if let Some(carried) = carried {
        std::fs::copy(&carried, &incoming).map_err(|error| format!("{}: {error}", carried.display()))?;
    } else if relays && let Some(url) = &fetch.package.url {
        let _ = std::fs::remove_file(&incoming);
        let speed = fastctx_update::download::patiently(url, &incoming, &mut |_, _| {}).await;
        if let Ok(Some(speed)) = speed {
            github_speed(node, speed);
        }
        if let Err(why) = speed {
            let _ = std::fs::remove_file(&incoming);
            return Err(why);
        }
    } else {
        return Err("this machine neither holds the package nor fetches for others".to_string());
    }
    let (path, signature, version) = (incoming.clone(), fetch.package.signature.clone(), fetch.version.clone());
    tokio::task::spawn_blocking(move || fastctx_update::verify::file(&path, &signature, &version))
        .await
        .map_err(|error| error.to_string())?
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&incoming);
        })?;
    let path = cache.keep(&fetch.version, &fetch.target, &incoming)?;
    report(node);
    Ok(path)
}

/// Updates this machine's World program to `version`, or from the release
/// offered, when that is newer than this program: brought here, checked,
/// unpacked, and handed to the same renewal an interface's carried program
/// takes, which waits for the turns under way to end. It never goes back.
/// The package on its way is shown as one fetched, and a failure kept, as
/// for the interface's; `asker` is who asked from elsewhere.
async fn kernel(node: &Arc<Node>, version: Option<&str>, asker: Option<Asker>) -> Result<(), String> {
    let offer = match version {
        Some(version) => manifest_of(node, version).await?,
        None => {
            state(node).offer.ok_or("no release is known here yet; look again once GitHub or the network answers")?
        }
    };
    if !offer.newer_than(env!("CARGO_PKG_VERSION")) {
        return Err(match version {
            Some(version) => format!("this machine runs {}; {version} is not newer", env!("CARGO_PKG_VERSION")),
            None => format!("this machine runs {}, the newest on its line", env!("CARGO_PKG_VERSION")),
        });
    }
    let target = fastctx_update::kernel::target();
    if offer.package(&target).is_none() {
        // Releases carry command-line programs for Linux and Windows on x64:
        // elsewhere the World program comes with the application
        // (`world-versions.md`).
        let elsewhere =
            if cfg!(target_os = "linux") { "" } else { "; on this system it comes with the Fastctx application" };
        return Err(format!(
            "{} has no command-line program for {}{elsewhere}",
            offer.version,
            fastctx_update::kernel::triple()
        ));
    }
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        match &held.fetching {
            // Asked again while it comes, the update goes on as it is.
            Some(fetching) if fetching.version == offer.version && fetching.target == target => return Ok(()),
            Some(fetching) => {
                return Err(format!(
                    "{} of {} is on its way here; once it is, ask again",
                    fetching.target, fetching.version
                ));
            }
            None => {}
        }
        held.failed = None;
        held.fetching = Some(Fetching {
            version: offer.version.clone(),
            target: target.clone(),
            via: fastctx_types::update::Via::Github,
            received: 0,
            total: None,
        });
    }
    let taken = take(node, &offer, &target, asker).await;
    let mut held = node.updates.held.lock().expect("never poisoned");
    held.fetching = None;
    if let Err(why) = &taken {
        held.failed = Some(Failed { version: offer.version.clone(), target, why: why.clone() });
    }
    drop(held);
    report(node);
    taken
}

/// Brings the program of `offer` here and hands it to the renewal.
async fn take(node: &Arc<Node>, offer: &Manifest, target: &str, asker: Option<Asker>) -> Result<(), String> {
    let archive = bring(node, offer, target).await?;
    let asked = dir(node).join("asked.json");
    match asker {
        Some(Asker { by, hub }) => {
            let file = AskedFile { by, from: env!("CARGO_PKG_VERSION").to_string(), hub };
            keep(node, "asked.json", &file).map_err(|error| format!("keeping who asked for the update: {error}"))?;
        }
        None => {
            let _ = std::fs::remove_file(&asked);
        }
    }
    let root = node.root.clone();
    let unpacking = dir(node).join("unpacked");
    // The release's signature goes beside the program, as an interface's
    // carried one has it: the Windows guardian takes a program in by it,
    // with no administrator (`world-upgrade.md`).
    let signature = offer.package(target).map(|package| package.signature.clone());
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let _ = std::fs::remove_dir_all(&unpacking);
        std::fs::create_dir_all(&unpacking).map_err(|error| error.to_string())?;
        let program = fastctx_update::kernel::unpack(&archive, &unpacking)?;
        if let Some(signature) = signature {
            let mut beside = program.clone().into_os_string();
            beside.push(".sig");
            std::fs::write(&beside, signature).map_err(|error| format!("keeping the program's signature: {error}"))?;
        }
        let staged = stage(&root, &program);
        let _ = std::fs::remove_dir_all(&unpacking);
        staged
    })
    .await
    .map_err(|error| error.to_string())??;
    node.log.line(format_args!("staged fastctx {} to take over once the turns under way end", offer.version));
    Ok(())
}

fn stage(root: &Path, program: &Path) -> Result<(), String> {
    let plan = fastctx_install::renewal::stage(root, program, None).map_err(|error| error.to_string())?;
    if plan.phase != fastctx_types::renewal::Phase::Complete {
        fastctx_install::renewal::request(root, true).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Asks `machine`, this machine or another, about its updates. A machine
/// of a release before 1.0.1 beta 3, as its hub last heard, is only
/// updated to the newest on its line.
pub(crate) async fn machine(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    machine: fastctx_types::MachineName,
    update: UpdateCall,
) -> Result<State, fastctx_types::ipc::Failure> {
    use fastctx_types::ipc::Failure;
    use fastctx_types::remote::{RemoteCall, RemoteReply};
    if node.is_me(&machine) {
        return call(node, update).await.map_err(Failure::new);
    }
    // What it runs, as its hub last heard; else as it says itself. A machine
    // whose release is not known is not taken for one that knows the newer
    // call.
    let heard = network(node).and_then(|network| network.machines.get(&machine).map(|running| running.kernel.clone()));
    let running = match heard {
        Some(kernel) => Some(kernel),
        None => {
            match node.call(acting, &(&machine).into(), fastctx_types::access::Purpose::Look, RemoteCall::Facts).await {
                Ok(Ok(RemoteReply::Facts(facts))) => Some(facts.version),
                _ => None,
            }
        }
    };
    let older = running
        .as_deref()
        .map(|kernel| fastctx_types::update::release(kernel) < fastctx_types::update::release(ASKED_SINCE));
    let call = match (update, older) {
        (update, Some(false)) => RemoteCall::Updates { update },
        // Every release takes this one.
        (UpdateCall::Kernel { version: None }, _) => RemoteCall::UpdateWorld,
        (_, Some(true)) => {
            return Err(Failure::new(format!(
                "{machine} runs a release before {ASKED_SINCE}, which only updates to the newest on its line"
            )));
        }
        (_, None) => {
            return Err(Failure::new(format!(
                "what {machine} runs is not known now, and a release before {ASKED_SINCE} only updates to the newest on its line; ask again once it is connected"
            )));
        }
    };
    match node
        .call(acting, &(&machine).into(), fastctx_types::access::Purpose::Hands, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))??
    {
        RemoteReply::Update(state) => Ok(*state),
        _ => Err(Failure::new(format!("{machine} answered something other than how its updates stand"))),
    }
}

/// The first release that takes `RemoteCall::Updates`.
const ASKED_SINCE: &str = "1.0.1-beta.3";

/// Another machine asks about this one's updates, as `asker` through its
/// hub: what only an interface here says or brings is not its to ask. An
/// update asked of a machine an application carries the World program of
/// is the application's to take (`application`).
pub(crate) async fn remote(node: &Arc<Node>, asker: Asker, update: Option<UpdateCall>) -> Result<State, String> {
    let app = node.updates.held.lock().expect("never poisoned").app.clone();
    let version = match &update {
        None => Some(None),
        Some(UpdateCall::Kernel { version }) => Some(version.clone()),
        Some(_) => None,
    };
    if let (Some(app), Some(version)) = (app, version.clone()) {
        let manifest = match version {
            Some(version) => manifest_of(node, &version).await?,
            None => state(node)
                .offer
                .ok_or("no release is known here yet; look again once GitHub or the network answers")?,
        };
        if !manifest.newer_than(&app.version) {
            return Err(format!("the application here is Fastctx {}; {} is not newer", app.version, manifest.version));
        }
        if manifest.package(&app.target).is_none() {
            return Err(format!(
                "{} has no package for this computer's application ({})",
                manifest.version, app.target
            ));
        }
        application::ask(node, manifest, asker, &app.target)?;
        return Ok(state(node));
    }
    match update {
        None => kernel(node, None, Some(asker)).await.map(|()| state(node)),
        Some(UpdateCall::Kernel { version }) => {
            kernel(node, version.as_deref(), Some(asker)).await.map(|()| state(node))
        }
        Some(UpdateCall::App { .. } | UpdateCall::Fetch { .. }) => {
            Err(format!("only an interface on {} says what it is or brings packages there", node.machine()))
        }
        Some(update) => call(node, update).await,
    }
}

/// A machine no application carries the World program for takes a newer
/// release by itself when it updates by itself, or when the network keeps
/// an update for it that it has not made; an application takes its own,
/// which brings the program with it. Taken by itself, a release never
/// passes the hubs' major version (`hub_major`); one the network keeps for
/// it was asked for by someone, as one the owner picks is.
fn auto(node: &Arc<Node>) {
    let state = state(node);
    if state.app.is_some() || state.fetching.is_some() || state.renewing.is_some() {
        return;
    }
    let me = node.machine();
    let running = fastctx_types::update::release(env!("CARGO_PKG_VERSION"));
    let due = node.links().iter().find_map(|link| {
        let due = link.versions()?.due.remove(&me)?;
        let asker = Asker { by: due.by, hub: link.membership().id };
        (fastctx_types::update::release(&due.version) > running).then_some((due.version, asker))
    });
    let (version, asker) = match due {
        Some((version, asker)) => (version, Some(asker)),
        None if state.auto => match within(&state) {
            Some(version) => (version, None),
            None => return,
        },
        None => return,
    };
    {
        let held = node.updates.held.lock().expect("never poisoned");
        if held.tried.as_ref().is_some_and(|(tried, at)| *tried == version && at.elapsed() < TRY_AGAIN) {
            return;
        }
    }
    // Taken or not, a release is not tried again for a while: one that put
    // in place a program of another version would be taken over and over.
    node.updates.held.lock().expect("never poisoned").tried = Some((version.clone(), std::time::Instant::now()));
    let node = node.clone();
    tokio::spawn(async move {
        if let Err(why) = kernel(&node, Some(&version), asker).await {
            node.log.line(format_args!("updating this machine's World program to {version}: {why}"));
        }
    });
}

/// The newest release on this machine's line, newer than what runs here,
/// that does not pass the hubs' major version.
pub(crate) fn within(state: &State) -> Option<String> {
    let running = fastctx_types::update::release(env!("CARGO_PKG_VERSION"));
    let fits = |version: &str| {
        let release = fastctx_types::update::release(version);
        state.channel.takes(version)
            && release > running
            && state.hub_major.is_none_or(|major| release.as_ref().is_some_and(|release| release.major <= major))
    };
    if let Some(offer) = &state.offer
        && fits(&offer.version)
    {
        return Some(offer.version.clone());
    }
    state.looked.releases.iter().map(|release| &release.version).find(|version| fits(version)).cloned()
}

/// The hubs told of versions anew.
pub(crate) fn heard(node: &Arc<Node>) {
    auto(node);
}
