//! New versions of Fastctx on this machine (`world-upgrade.md`, 新版本自己往后
//! 怎样升级): the device's own choices, a look on GitHub every few hours,
//! what this machine tells each of its hubs and hears back, the packages it
//! brings here for the interface or for other machines, and its own World
//! program updated from the newest release when no application carries it.
//! Everything kept is under `<root>/updates`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_install::renewal::{read_json, write_json};
use fastctx_types::update::{
    App, Channel, Failed, Fetch, Fetching, Looked, Manifest, Network, Report, Settings, State, UpdateCall,
};
use fastctx_update::Cache;

use crate::Node;

/// How often GitHub is looked at.
const LOOK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Default)]
pub(crate) struct Updates {
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    settings: Settings,
    looked: Looked,
    app: Option<App>,
    /// Where the application keeps the command-line programs it carries.
    carried: Option<PathBuf>,
    fetching: Option<Fetching>,
    failed: Option<Failed>,
}

/// What the interface here keeps of itself for the node.
#[derive(serde::Serialize, serde::Deserialize)]
struct AppFile {
    app: App,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    carried: Option<PathBuf>,
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
        held.looked = read_json(&dir.join("looked.json")).ok().flatten().unwrap_or_default();
        if let Ok(Some(AppFile { app, carried })) = read_json::<AppFile>(&dir.join("app.json")) {
            held.app = Some(app);
            held.carried = carried;
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
    let looked = fastctx_update::github::look().await;
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
/// release on each channel any of them knows, and the policy of the one
/// that asks most.
fn network(node: &Node) -> Option<Network> {
    let mut merged: Option<Network> = None;
    for network in node.links().iter().filter_map(|link| link.versions()) {
        let Some(kept) = &mut merged else {
            merged = Some(network);
            continue;
        };
        kept.stable = fastctx_types::update::newest(kept.stable.as_ref(), network.stable.as_ref()).cloned();
        kept.beta = fastctx_types::update::newest(kept.beta.as_ref(), network.beta.as_ref()).cloned();
        if network.policy.channel == Channel::Beta {
            kept.policy.channel = Channel::Beta;
        }
        kept.policy.minimum = higher(kept.policy.minimum.take(), network.policy.minimum);
        kept.machines.extend(network.machines);
    }
    merged
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

/// How updates stand here now.
pub(crate) fn state(node: &Node) -> State {
    let network = network(node);
    let held = node.updates.held.lock().expect("never poisoned");
    let policy = network.as_ref().map(|network| network.policy.clone()).unwrap_or_default();
    let channel = held.settings.channel(&policy);
    let kept = cache(node).manifests();
    let offer = [held.looked.on(channel), network.as_ref().and_then(|network| network.on(channel))]
        .into_iter()
        .chain(kept.iter().filter(|manifest| on_channel(manifest, channel)).map(Some))
        .fold(None, fastctx_types::update::newest)
        .cloned();
    State {
        settings: held.settings.clone(),
        channel,
        looked: held.looked.clone(),
        network,
        offer,
        fetching: held.fetching.clone(),
        failed: held.failed.clone(),
        kept: cache(node).kept(),
        app: held.app.clone(),
        kernel_target: fastctx_update::kernel::target(),
    }
}

fn on_channel(manifest: &Manifest, channel: Channel) -> bool {
    channel == Channel::Beta || manifest.semver().is_some_and(|version| version.pre.is_empty())
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
    };
    drop(held);
    for link in node.links() {
        link.report_versions(report.clone());
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
        UpdateCall::App { app, carried } => {
            let file = AppFile { app: app.clone(), carried: carried.map(PathBuf::from) };
            let mut held = node.updates.held.lock().expect("never poisoned");
            if held.app.as_ref() != Some(&app) || held.carried != file.carried {
                keep(node, "app.json", &file).map_err(|error| error.to_string())?;
                held.app = Some(file.app);
                held.carried = file.carried;
                drop(held);
                report(node);
            }
        }
        UpdateCall::Fetch { version, target } => fetch(node, version, target)?,
        UpdateCall::Kernel => kernel(node).await?,
    }
    Ok(state(node))
}

/// Brings the package `target` of `version` here, in the background.
fn fetch(node: &Arc<Node>, version: String, target: String) -> Result<(), String> {
    let state = state(node);
    let manifest = [state.offer.as_ref(), state.looked.stable.as_ref(), state.looked.beta.as_ref()]
        .into_iter()
        .flatten()
        .chain(state.network.as_ref().into_iter().flat_map(|network| network.stable.iter().chain(network.beta.iter())))
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

/// Brings a package here by GitHub, then the first hub that hands them out,
/// with its progress kept for the interface; older versions go once it is.
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
    let path = fastctx_update::obtain(&cache, manifest, target, link, &mut progress).await?;
    cache.prune(&[manifest.version.as_str()]);
    Ok(path)
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
    let incoming = cache.incoming(&fetch.version, &fetch.target)?;
    if let Some(carried) = carried {
        std::fs::copy(&carried, &incoming).map_err(|error| format!("{}: {error}", carried.display()))?;
    } else if relays && let Some(url) = &fetch.package.url {
        let _ = std::fs::remove_file(&incoming);
        fastctx_update::download::from_url(url, &incoming, &mut |_, _| {}).await?;
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

/// Updates this machine's World program from the newest release on its
/// channel, when that is newer than this program: brought here, checked,
/// unpacked, and handed to the same renewal an interface's carried program
/// takes, which waits for the turns under way to end.
async fn kernel(node: &Arc<Node>) -> Result<(), String> {
    let state = state(node);
    let offer = state.offer.ok_or("no release is known here yet; look again once GitHub or the network answers")?;
    if !offer.newer_than(env!("CARGO_PKG_VERSION")) {
        return Err(format!("this machine runs {}, the newest on its channel", env!("CARGO_PKG_VERSION")));
    }
    let target = fastctx_update::kernel::target();
    if offer.package(&target).is_none() {
        return Err(format!("{} has no command-line program for {}", offer.version, fastctx_update::kernel::triple()));
    }
    let archive = bring(node, &offer, &target).await?;
    let root = node.root.clone();
    let unpacking = dir(node).join("unpacked");
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let _ = std::fs::remove_dir_all(&unpacking);
        std::fs::create_dir_all(&unpacking).map_err(|error| error.to_string())?;
        let program = fastctx_update::kernel::unpack(&archive, &unpacking)?;
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

/// Updates `machine`'s World program, this machine's or another's.
pub(crate) async fn machine(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    machine: fastctx_types::MachineName,
) -> Result<State, fastctx_types::ipc::Failure> {
    use fastctx_types::ipc::Failure;
    use fastctx_types::remote::{RemoteCall, RemoteReply};
    if node.is_me(&machine) {
        return call(node, UpdateCall::Kernel).await.map_err(Failure::new);
    }
    match node
        .call(acting, &(&machine).into(), fastctx_types::access::Purpose::Hands, RemoteCall::UpdateWorld)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))??
    {
        RemoteReply::Update(state) => Ok(*state),
        _ => Err(Failure::new(format!("{machine} answered something other than how its updates stand"))),
    }
}

/// With automatic updates on, a machine no application carries the World
/// program for takes a newer release by itself; an application takes its
/// own, which brings the program with it.
fn auto(node: &Arc<Node>) {
    let state = state(node);
    if !state.settings.auto || state.app.is_some() || state.fetching.is_some() {
        return;
    }
    let newer = state.offer.as_ref().is_some_and(|offer| offer.newer_than(env!("CARGO_PKG_VERSION")));
    if !newer || state.failed.is_some() {
        return;
    }
    let node = node.clone();
    tokio::spawn(async move {
        if let Err(why) = kernel(&node).await {
            node.log.line(format_args!("updating this machine's World program: {why}"));
        }
    });
}

/// The hubs told of versions anew.
pub(crate) fn heard(node: &Arc<Node>) {
    auto(node);
}
