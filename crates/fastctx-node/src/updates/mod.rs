//! New versions of Fastctx on this machine (`world-upgrade.md`, 新版本自己往后
//! 怎样升级): the device's own choices, a look on GitHub every few hours,
//! what this machine tells each of its hubs and hears back, the packages it
//! brings here for the interface or for other machines, and its own World
//! program updated from the newest release when no application carries it,
//! or the application's release taken as another machine asks
//! (`application`). Everything kept is under `<root>/updates`.

mod application;
mod doctor;
mod legacy_repair;
mod packages;
pub(crate) mod repair;
mod retry;
mod schedule;

pub(crate) use application::Open;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_install::renewal::{read_json, write_json};
use fastctx_types::AccountName;
use fastctx_types::update::{
    ASKED_SINCE, App, Asked, Failed, Fetching, Kind, Looked, Manifest, Renewing, Report, Settings, Stage, State,
    UpdateCall,
};
use fastctx_update::Cache;

use crate::Node;

#[derive(Default)]
pub(crate) struct Updates {
    held: Mutex<Held>,
    packages: fastctx_update::pool::Pool,
    repair_launching: std::sync::atomic::AtomicBool,
    repair_logged: Mutex<String>,
    doctor: Mutex<doctor::Seen>,
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
    app_executable: Option<PathBuf>,
    /// The release someone asked from elsewhere that the application take.
    take: Option<String>,
    origin: Option<schedule::Origin>,
    fetching: Option<Fetching>,
    attempt: retry::Attempt,
    activity: Option<Stage>,
    /// How fast this machine's last download from GitHub went, in bytes a
    /// second (`GITHUB_SPEED`).
    github_speed: Option<u64>,
    /// What was kept is read: until then, news of versions from a hub the
    /// node joined first takes nothing by itself.
    started: bool,
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
    pub scheduled_at: Option<jiff::Timestamp>,
}

/// What the interface here keeps of itself for the node.
type AppFile = fastctx_install::repair::application::Registration;

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
    for why in cache(node).clear_attempts() {
        node.log.line(format_args!("deleting a package left coming: {why}"));
    }
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        held.settings = read_json(&dir.join("settings.json")).ok().flatten().unwrap_or_default();
        // Turned on is always its owner's doing: 1.0.1 beta 1 and 2 kept no
        // `chosen` and took nothing by themselves unless told to.
        if held.settings.auto {
            held.settings.chosen = true;
        }
        held.looked = read_json(&dir.join("looked.json")).ok().flatten().unwrap_or_default();
        held.looked.checking = false;
        held.github_speed = read_json(&dir.join(GITHUB_SPEED)).ok().flatten();
        if let Ok(Some(AppFile { app, carried, program, executable })) = read_json::<AppFile>(&dir.join("app.json")) {
            held.app = Some(app);
            held.carried = carried;
            held.program = program;
            held.app_executable = executable;
        }
        if let Ok(Some(take)) = read_json::<application::Take>(&dir.join("take.json")) {
            held.take = Some(take.version);
            held.chosen = take.manifest;
            held.origin = take.origin;
        }
        held.attempt = match read_json(&dir.join("attempt.json")) {
            Ok(attempt) => attempt.unwrap_or_default(),
            Err(error) => {
                node.log.line(format_args!("reading the update attempt: {error}"));
                retry::Attempt::default()
            }
        };
        held.started = true;
    }
    retry::complete(node, env!("CARGO_PKG_VERSION"));
    if let Some(version) = application(node) {
        application::took(node, &version);
    }
    prune(node, None);
    report(node);
    if !fastctx_platform::follows_releases() {
        return;
    }
    doctor::start(node);
    let node = node.clone();
    tokio::spawn(async move {
        let settled = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            let due = fastctx_update::github::wait(&node.updates.held.lock().expect("never poisoned").looked).is_zero();
            if due && tokio::time::Instant::now() >= settled {
                look(&node);
            }
            auto(&node);
            report(&node);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

/// Looks on GitHub now, and keeps what it found.
fn look(node: &Arc<Node>) {
    let previous = {
        let mut held = node.updates.held.lock().expect("never poisoned");
        if held.looked.checking {
            return;
        }
        let previous = held.looked.clone();
        held.looked.checking = true;
        previous
    };
    report(node);
    let node = node.clone();
    tokio::spawn(async move {
        let running = application(&node).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        let looked =
            fastctx_update::github::look_after(Some(&running), &previous, Some(&dir(&node).join("github-list.json")))
                .await;
        if let Some(why) = &looked.why {
            node.log.line(format_args!("looking for new versions on GitHub: {why}"));
        }
        {
            let mut held = node.updates.held.lock().expect("never poisoned");
            let mut looked = looked;
            looked.github_at = looked.github_at.max(held.looked.github_at);
            held.looked = looked;
            if let Err(error) = keep(&node, "looked.json", &held.looked) {
                node.log.line(format_args!("keeping what was found of new versions: {error}"));
            }
        }
        report(&node);
        auto(&node);
    });
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

/// How updates stand here now. What is offered is the release the owner
/// chose, while it is newer than what runs here, or else the newest on the
/// line of what runs here (`world-upgrade.md`, 正式版与测试版).
pub(crate) fn state(node: &Node) -> State {
    let links = node.links();
    let networks = links.iter().filter_map(|link| link.update_network()).collect();
    let held = node.updates.held.lock().expect("never poisoned");
    let cache = cache(node);
    let context = fastctx_update::state::Context {
        kind: kind(node, &held),
        connected: !links.is_empty(),
        hub_here: fastctx_install::service::registered_hubs(&node.root).is_ok_and(|hubs| !hubs.is_empty()),
        networks,
        cached: cache.manifests(),
        activity: held.activity.clone(),
    };
    fastctx_update::state::calculate(
        State {
            running: Some(env!("CARGO_PKG_VERSION").to_string()),
            settings: held.settings.clone(),
            chosen: held.chosen.clone(),
            looked: held.looked.clone(),
            fetching: held.fetching.clone(),
            failed: held.attempt.failed.clone().or_else(|| {
                let plan = fastctx_install::renewal::read(&node.root).ok().flatten()?;
                // A step waited on says why without having failed.
                if plan.phase != fastctx_types::renewal::Phase::Failed {
                    return None;
                }
                let version = plan.error_version.unwrap_or(plan.candidate.build.version);
                // A program this node runs already, or one older, is no
                // failure to tell now.
                if !fastctx_types::kernel::later(&version, env!("CARGO_PKG_VERSION")) {
                    return None;
                }
                Some(Failed { version, target: fastctx_update::kernel::target(), why: plan.error?, reason: None })
            }),
            retry_at: repair::retry_at(node).or_else(|| held.attempt.failed.as_ref().and(held.attempt.delay.next)),
            kept: cache.kept(),
            app: held.app.clone(),
            kernel_target: fastctx_update::kernel::target(),
            renewing: renewing(node),
            take: held.take.clone(),
            open: application::open(),
            repair_supported: true,
            repair_routing: true,
            repair: repair::snapshot(node),
            ..State::default()
        },
        context,
    )
}

/// The change of World program under way here, or the last one not taken.
fn renewing(node: &Node) -> Option<Renewing> {
    let plan = fastctx_install::renewal::read(&node.root).ok().flatten()?;
    let version = plan.candidate.build.version;
    let done = plan.phase == fastctx_types::renewal::Phase::Complete;
    (!done).then_some(Renewing { version, phase: plan.phase, error: plan.error })
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
    let state = state(node);
    let held = node.updates.held.lock().expect("never poisoned");
    let cache = cache(node);
    let mut found: Vec<Manifest> =
        [held.looked.stable.clone(), held.looked.beta.clone()].into_iter().flatten().collect();
    // A release whose package is kept here is known here too.
    found.extend(held.looked.newer.iter().cloned());
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
        // Its owner's choice is known only once its settings are read.
        relays: held.settings.relays && held.started,
        holds,
        asked: None,
        hub_here: false,
        github_speed: held.github_speed,
        target: Some(fastctx_update::kernel::target()),
        package_progress: true,
        stage: state.stage,
        auto: Some(state.auto),
        auto_chosen: held.settings.chosen && kind(node, &held) != Kind::Managed,
        kind: Some(kind(node, &held)),
        github_at: held.looked.github_at,
        check_failure: held.looked.failure.clone(),
        checked_at: held.looked.at,
        checking: held.looked.checking,
        updates_view: true,
        repair_supported: true,
        repair: state.repair,
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
    if !fastctx_platform::follows_releases()
        && matches!(
            call,
            UpdateCall::Look | UpdateCall::Fetch { .. } | UpdateCall::Kernel { .. } | UpdateCall::Choose { .. }
        )
    {
        return Err("development builds do not follow releases".into());
    }
    match call {
        UpdateCall::Progress { stage } => {
            // An installation the interface could not make is kept as any
            // failed update is, and not tried again before its time, the
            // interface opened again or not.
            if let Stage::Failed { version, problem } = &stage {
                let target = node.updates.held.lock().expect("never poisoned").app.as_ref().map(|app| app.target.clone());
                if let Some(target) = target {
                    let failure = Failed {
                        version: version.clone(),
                        target,
                        why: problem.why.clone(),
                        reason: Some(problem.reason.clone()),
                    };
                    retry::failed(node, failure);
                }
            }
            node.updates.held.lock().expect("never poisoned").activity =
                (!matches!(stage, Stage::Idle | Stage::Failed { .. })).then_some(stage);
            report(node);
        }
        UpdateCall::Preferences { auto, relays } => {
            let mut held = node.updates.held.lock().expect("never poisoned");
            let settings = held.settings.clone().with_preferences(auto, relays);
            keep(node, "settings.json", &settings).map_err(|e| e.to_string())?;
            held.settings = settings;
            drop(held);
            report(node);
        }
        UpdateCall::Repair { version, id } => repair::accept(node, version, id, "local".into())?,
        UpdateCall::State => {}
        UpdateCall::Look => {
            look(node);
            while node.updates.held.lock().expect("never poisoned").looked.checking {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        UpdateCall::Set { settings } => {
            keep(node, "settings.json", &settings).map_err(|error| error.to_string())?;
            node.updates.held.lock().expect("never poisoned").settings = settings;
            report(node);
            auto(node);
        }
        UpdateCall::App { app, carried, program, executable } => {
            application::took(node, &app.version);
            let file =
                AppFile { app: app.clone(), carried: carried.map(PathBuf::from), program: program.map(PathBuf::from), executable: executable.map(PathBuf::from) };
            let mut held = node.updates.held.lock().expect("never poisoned");
            if held.app.as_ref() != Some(&app) || held.carried != file.carried || held.program != file.program || held.app_executable != file.executable {
                keep(node, "app.json", &file).map_err(|error| error.to_string())?;
                held.app = Some(file.app);
                held.activity = None;
                held.carried = file.carried;
                held.program = file.program;
                held.app_executable = file.executable;
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
    fastctx_update::catalog::manifest(&state(node), cache(node).manifests(), &node.links(), version).await
}

/// Brings the package `target` of `version` here, in the background.
fn fetch(node: &Arc<Node>, version: String, target: String) -> Result<(), String> {
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        if held.fetching.as_ref().is_some_and(|fetching| fetching.version == version && fetching.target == target) {
            return Ok(());
        }
        if held.fetching.is_some() || matches!(held.activity, Some(Stage::Installing { .. })) {
            return Err("an update is already being downloaded or installed".into());
        }
        held.activity = None;
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
        let outcome = match manifest_of(&node, &version).await {
            Ok(manifest) => bring(&node, &manifest, &target).await,
            Err(why) => Err(why.into()),
        };
        node.updates.held.lock().expect("never poisoned").fetching = None;
        if let Err(why) = outcome {
            node.log.line(format_args!("bringing {target} of {version} here: {why}"));
            retry::failed(&node, Failed { version, target, why: why.why, reason: Some(why.reason) });
        }
        report(&node);
    });
    Ok(())
}

/// Brings a package here from GitHub, or through each supplying hub in turn
/// when GitHub fails or goes slowly (`fastctx_update::obtain`), with its progress kept for the interface.
/// Helpers can still be delivering other verified packages from this cache.
async fn bring(node: &Arc<Node>, manifest: &Manifest, target: &str) -> Result<PathBuf, fastctx_types::update::Problem> {
    let cache = cache(node);
    let links = node.links();
    let sources: Vec<_> = links.iter().map(|link| link.as_ref()).collect();
    let progressing = node.clone();
    let mut progress = move |progress: fastctx_update::Progress| {
        if let Some(fetching) = &mut progressing.updates.held.lock().expect("never poisoned").fetching
            && fetching.version == manifest.version
            && fetching.target == target
        {
            fetching.via = progress.via;
            fetching.received = progress.received;
            fetching.total = progress.total;
        }
    };
    let logging = node.clone();
    let log: fastctx_update::pool::Log = Arc::new(move |message| logging.log.line(format_args!("{message}")));
    let (looked, carried) = {
        let held = node.updates.held.lock().expect("never poisoned");
        let carried = held
            .carried
            .as_ref()
            .filter(|_| held.app.as_ref().is_some_and(|app| app.version == manifest.version))
            .map(|dir| dir.join(format!("{target}.tar.gz")))
            .filter(|path| path.is_file());
        (held.looked.clone(), carried)
    };
    let network_now = looked.github == Some(false) && looked.github_at <= looked.at;
    let brought = node
        .updates
        .packages
        .obtain(&cache, manifest, target, &sources, network_now, carried, &mut progress, log)
        .await?;
    if let Some(at) = brought.github_at {
        github_reached(node, at);
    }
    if let Some(speed) = brought.github_speed {
        github_speed(node, speed);
    }
    prune(node, Some(&manifest.version));
    Ok(brought.path)
}

/// Keeps the running, pending and freshly downloaded versions, plus active owners.
fn prune(node: &Arc<Node>, ready: Option<&str>) {
    let mut keep = vec![env!("CARGO_PKG_VERSION").to_string()];
    let held = node.updates.held.lock().expect("never poisoned");
    keep.extend(held.app.as_ref().map(|a| a.version.clone()));
    keep.extend(held.take.iter().cloned());
    keep.extend(held.fetching.as_ref().map(|f| f.version.clone()));
    keep.extend(held.chosen.as_ref().map(|m| m.version.clone()));
    keep.extend(held.activity.as_ref().and_then(Stage::version).map(str::to_owned));
    drop(held);
    keep.extend(renewing(node).map(|r| r.version));
    keep.extend(repair::snapshot(node).filter(|r| !r.success).map(|r| r.version));
    if let Some(ready) = ready {
        keep.push(ready.to_string());
    } else if let Some(ready) = cache(node).manifests().first() {
        keep.push(ready.version.clone());
    }
    let logging = node.clone();
    node.updates.packages.prune(&cache(node), &keep, Arc::new(move |why| logging.log.line(format_args!("{why}"))));
}

fn github_reached(node: &Node, at: jiff::Timestamp) {
    let mut held = node.updates.held.lock().expect("never poisoned");
    held.looked.github_at = held.looked.github_at.max(Some(at));
    if let Err(error) = keep(node, "looked.json", &held.looked) {
        node.log.line(format_args!("keeping when GitHub was reached: {error}"));
    }
}

/// Keeps how fast this machine's download from GitHub went, and tells its
/// hubs: the fastest is asked first to fetch for the network.
fn github_speed(node: &Arc<Node>, speed: u64) {
    {
        let mut held = node.updates.held.lock().expect("never poisoned");
        held.github_speed = Some(speed);
    }
    if let Err(error) = keep(node, GITHUB_SPEED, &speed) {
        node.log.line(format_args!("keeping the speed of GitHub: {error}"));
    }
    report(node);
}

pub(crate) use packages::{relay, retain as retain_package};

fn reject_update(
    node: &Node,
    version: &str,
    target: &str,
    reason: fastctx_types::update::FailureReason,
    why: String,
) -> String {
    retry::failed(
        node,
        Failed { version: version.to_string(), target: target.to_string(), why: why.clone(), reason: Some(reason) },
    );
    report(node);
    why
}

/// Updates this machine's World program to `version`, or from the release
/// offered, when that is newer than this program: brought here, checked,
/// unpacked, and handed to the same renewal an interface's carried program
/// takes, which waits for the turns under way to end. It never goes back.
/// The package on its way is shown as one fetched, and a failure kept, as
/// for the interface's; `asker` is who asked from elsewhere.
async fn kernel(node: &Arc<Node>, version: Option<&str>, asker: Option<Asker>) -> Result<(), String> {
    let offer = match version {
        // A release asked for whose manifest did not come is kept as a
        // failure of that release, to be tried again as any other.
        Some(version) => match manifest_of(node, version).await {
            Ok(offer) => offer,
            Err(why) => {
                let target = fastctx_update::kernel::target();
                retry::failed(node, Failed { version: version.to_string(), target, why: why.clone(), reason: None });
                report(node);
                return Err(why);
            }
        },
        None => {
            state(node).offer.ok_or("no release is known here yet; look again once GitHub or the network answers")?
        }
    };
    if !offer.newer_than(env!("CARGO_PKG_VERSION")) {
        let why = match version {
            Some(version) => format!("this machine runs {}; {version} is not newer", env!("CARGO_PKG_VERSION")),
            None => format!("this machine runs {}, the newest on its line", env!("CARGO_PKG_VERSION")),
        };
        return Err(reject_update(
            node,
            &offer.version,
            &fastctx_update::kernel::target(),
            fastctx_types::update::FailureReason::NotNewer,
            why,
        ));
    }
    let target = fastctx_update::kernel::target();
    if offer.package(&target).is_none() {
        // Releases carry command-line programs for Linux and Windows on x64:
        // elsewhere the World program comes with the application
        // (`world-versions.md`).
        let elsewhere =
            if cfg!(target_os = "linux") { "" } else { "; on this system it comes with the Fastctx application" };
        let why = format!(
            "{} has no command-line program for {}{elsewhere}",
            offer.version,
            fastctx_update::kernel::triple()
        );
        return Err(reject_update(node, &offer.version, &target, fastctx_types::update::FailureReason::NoPackage, why));
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
        held.activity = None;
        held.fetching = Some(Fetching {
            version: offer.version.clone(),
            target: target.clone(),
            via: fastctx_types::update::Via::Github,
            received: 0,
            total: None,
        });
    }
    let taken = take(node, &offer, &target, asker).await;
    node.updates.held.lock().expect("never poisoned").fetching = None;
    if let Err(why) = &taken {
        retry::failed(
            node,
            Failed { version: offer.version.clone(), target, why: why.why.clone(), reason: Some(why.reason.clone()) },
        );
    }
    report(node);
    taken.map_err(|problem| problem.why)
}

/// Brings the program of `offer` here and hands it to the renewal.
async fn take(
    node: &Arc<Node>,
    offer: &Manifest,
    target: &str,
    asker: Option<Asker>,
) -> Result<(), fastctx_types::update::Problem> {
    let archive = bring(node, offer, target).await?;
    let origin = asker.as_ref().and_then(schedule::Origin::from_asker);
    if schedule::withdrawn(node, &offer.version, origin.as_ref()) {
        return Ok(());
    }
    let asked = dir(node).join("asked.json");
    match asker {
        Some(Asker { by, hub, .. }) => {
            let file = AskedFile { by, from: env!("CARGO_PKG_VERSION").to_string(), hub };
            keep(node, "asked.json", &file).map_err(|error| format!("keeping who asked for the update: {error}"))?;
        }
        None => {
            let _ = std::fs::remove_file(&asked);
        }
    }
    let root = node.root.clone();
    let unpacking = dir(node).join("unpacked");
    tokio::task::spawn_blocking(move || stage_with_its_own_program(&root, &archive, &unpacking))
        .await
        .map_err(|error| error.to_string())??;
    node.log.line(format_args!("staged fastctx {} to take over once the turns under way end", offer.version));
    Ok(())
}

/// How long the release brought is given to stage itself.
const STAGING: Duration = Duration::from_secs(10 * 60);

/// Stages the release in `archive` with that release's own program, which
/// decides whether it is taken and takes over a handover left stuck: so a
/// fault in this release's staging never keeps a later release out, as one
/// in 1.0.1-beta.4's kept its machines on it (`world-upgrade.md`, 没成的，自己
/// 再试). The program's own signature comes out beside it, as an interface's
/// carried one has it; the package's signature is over the archive and never
/// goes beside the program. Where the program does not start here at all,
/// this release stages it, and says why it cannot be taken.
fn stage_with_its_own_program(root: &Path, archive: &Path, unpacking: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(unpacking);
    std::fs::create_dir_all(unpacking).map_err(|error| format!("{}: {error}", unpacking.display()))?;
    let staged = (|| {
        let program = fastctx_update::kernel::unpack(archive, unpacking)?;
        let mut command = std::process::Command::new(&program);
        command.args(["renew", "stage", "--dir"]).arg(root).arg("--package").arg(archive);
        command.stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0800_0000);
        }
        match fastctx_platform::bounded::output(command, "the new release staging itself", STAGING) {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                let said = String::from_utf8_lossy(&output.stderr);
                let said = said.trim();
                Err(if said.is_empty() { format!("the new release did not stage: {}", output.status) } else { said.to_string() })
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => Err(error.to_string()),
            Err(_) => {
                let plan = fastctx_install::renewal::stage_package(root, archive, &unpacking.join("here"))
                    .map_err(|error| format!("{error:#}"))?;
                if plan.phase != fastctx_types::renewal::Phase::Complete {
                    fastctx_install::renewal::request(root, true).map_err(|error| error.to_string())?;
                }
                Ok(())
            }
        }
    })();
    let _ = std::fs::remove_dir_all(unpacking);
    staged
}

/// Asks `machine`, this machine or another, about its updates. A machine
/// of a release before 1.0.1 beta 3, as its hub last heard, is only
/// updated to the newest on its line.
pub(crate) async fn machine(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    machine: fastctx_types::access::Written,
    mut update: UpdateCall,
) -> Result<State, fastctx_types::ipc::Failure> {
    use fastctx_types::ipc::Failure;
    use fastctx_types::remote::{RemoteCall, RemoteReply};
    if node.is_here(&machine) {
        return call(node, update).await.map_err(Failure::new);
    }
    if matches!(update, UpdateCall::App { .. } | UpdateCall::Fetch { .. } | UpdateCall::Progress { .. }) {
        return Err(Failure::new("only an interface on that machine says what it is or brings packages there"));
    }
    if matches!(update, UpdateCall::Repair { .. }) {
        let current = Box::pin(self::machine(node, acting, machine.clone(), UpdateCall::State)).await?;
        if !current.repair_supported { return Err(Failure::new(format!("{machine} runs a version too old to accept repair; arrange repair through a newer hub"))); }
        if !node.locate(&machine)?.iter().any(|link| link.versions().is_some_and(|n| n.repair_supported)) {
            return Err(Failure::new("the hub is too old to route repair; update it first"));
        }
    }
    if let UpdateCall::Preferences { auto, relays } = update {
        let current = Box::pin(self::machine(node, acting, machine.clone(), UpdateCall::State)).await?;
        update = if current.preferences {
            UpdateCall::Preferences { auto, relays }
        } else {
            UpdateCall::Set { settings: current.settings.with_preferences(auto, relays) }
        };
    }
    // What it runs, as its hub last heard; else as it says itself. A machine
    // whose release is not known is not taken for one that knows the newer
    // call.
    let heard = node.locate(&machine)?.iter().find_map(|link| {
        let view = link.view();
        link.versions()?.machines.get(view.canonical(&machine.name)).map(|running| running.kernel.clone())
    });
    let running = match heard {
        Some(kernel) => Some(kernel),
        None => match node.call(acting, &machine, fastctx_types::access::Purpose::Look, RemoteCall::Facts).await {
            Ok(Ok(RemoteReply::Facts(facts))) => Some(facts.version),
            _ => None,
        },
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
        .call(acting, &machine, fastctx_types::access::Purpose::Hands, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))??
    {
        RemoteReply::Update(state) => Ok(*state),
        _ => Err(Failure::new(format!("{machine} answered something other than how its updates stand"))),
    }
}

/// Another machine asks about this one's updates, as `asker` through its
/// hub: what only an interface here says or brings is not its to ask. An
/// update asked of a machine an application carries the World program of
/// is the application's to take (`application`).
pub(crate) async fn remote(node: &Arc<Node>, asker: Asker, update: Option<UpdateCall>) -> Result<State, String> {
    if let Some(UpdateCall::Repair { version, id }) = update {
        repair::accept(node, version, id, asker.by.to_string())?;
        return Ok(state(node));
    }
    let app = node.updates.held.lock().expect("never poisoned").app.clone();
    let version = match &update {
        None => Some(None),
        Some(UpdateCall::Kernel { version }) => Some(version.clone()),
        Some(_) => None,
    };
    if version.is_some() && !fastctx_platform::follows_releases() {
        return Err("development builds do not follow releases".into());
    }
    if let Some(requested) = &version {
        let wanted = requested.clone().or_else(|| state(node).offer.map(|manifest| manifest.version));
        let running = app.as_ref().map_or(env!("CARGO_PKG_VERSION"), |app| app.version.as_str());
        if wanted.as_deref() == Some(running) {
            return Ok(state(node));
        }
    }
    if let (Some(app), Some(version)) = (app, version.clone()) {
        let manifest = match version {
            Some(version) => manifest_of(node, &version).await?,
            None => state(node)
                .offer
                .ok_or("no release is known here yet; look again once GitHub or the network answers")?,
        };
        if !manifest.newer_than(&app.version) {
            let why = format!("the application here is Fastctx {}; {} is not newer", app.version, manifest.version);
            return Err(reject_update(
                node,
                &manifest.version,
                &app.target,
                fastctx_types::update::FailureReason::NotNewer,
                why,
            ));
        }
        if manifest.package(&app.target).is_none() {
            let why = format!("{} has no package for this computer's application ({})", manifest.version, app.target);
            return Err(reject_update(
                node,
                &manifest.version,
                &app.target,
                fastctx_types::update::FailureReason::NoPackage,
                why,
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
        Some(UpdateCall::App { .. } | UpdateCall::Fetch { .. } | UpdateCall::Progress { .. }) => {
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
    if !fastctx_platform::follows_releases() || !node.updates.held.lock().expect("never poisoned").started {
        return;
    }
    repair::scheduled(node);
    if repair::drive(node) { return; }
    schedule::reconcile(node);
    let state = state(node);
    if state.fetching.is_some() || application::busy() || (state.take.is_some() && (application::open() || !cfg!(windows)))
    {
        return;
    }
    let me = node.machine();
    let running =
        fastctx_types::update::release(state.app.as_ref().map_or(env!("CARGO_PKG_VERSION"), |a| a.version.as_str()));
    let due = node
        .links()
        .iter()
        .find_map(|link| {
            let due = link.versions()?.due.remove(&me)?;
            let asker = Asker { by: due.by, hub: link.membership().id, scheduled_at: Some(due.at) };
            (fastctx_types::update::release(&due.version) > running).then_some((due.version, asker))
        })
        .or_else(|| {
            let version = state.take.clone()?;
            if fastctx_types::update::release(&version) <= running {
                return None;
            }
            let asked = read_json::<AskedFile>(&dir(node).join("asked.json")).ok().flatten()?;
            Some((version, Asker { by: asked.by, hub: asked.hub, scheduled_at: None }))
        });
    // Updating by itself decides whether a new release is begun, not whether
    // one begun and failed is finished: that one is tried again on its
    // schedule however it was begun (`world-upgrade.md`, 没成的，自己再试).
    let unfinished = || {
        let held = node.updates.held.lock().expect("never poisoned");
        let failed = held.attempt.failed.as_ref()?;
        let refused = matches!(
            failed.reason,
            Some(fastctx_types::update::FailureReason::NoPackage | fastctx_types::update::FailureReason::NotNewer)
        );
        (!refused && failed.target == state.kernel_target && fastctx_types::update::release(&failed.version) > running)
            .then(|| failed.version.clone())
    };
    let (version, asker) = match due {
        Some((version, asker)) => (version, Some(asker)),
        None if state.app.is_none() => match within(&state).filter(|_| state.auto).or_else(unfinished) {
            Some(version) => (version, None),
            None => return,
        },
        None => return,
    };
    // A change to this release or a later one goes on by itself, a failed
    // one on its own durable recovery schedule; one to an earlier release,
    // under way or stuck, gives way to this one.
    if state.renewing.as_ref().is_some_and(|renewing| {
        fastctx_types::update::release(&renewing.version) >= fastctx_types::update::release(&version)
    }) {
        return;
    }
    // Where GitHub cannot be reached the network is the only source of a
    // package not here yet: the update waits for a hub to be connected,
    // rather than failing while the line is down and waiting an hour (a
    // room's line comes and goes). One already here goes on.
    let linked = node.links().iter().any(|link| matches!(link.status(), fastctx_net::link::Status::Online { .. }));
    if !linked && state.looked.github != Some(true) && cache(node).get(&version, &state.kernel_target).is_none() {
        return;
    }
    let Some(attempt) = retry::begin(node, &version) else { return };
    let node = node.clone();
    tokio::spawn(async move {
        let result = match asker {
            Some(asker) => {
                remote(&node, asker, Some(UpdateCall::Kernel { version: Some(version.clone()) })).await.map(|_| ())
            }
            None => kernel(&node, Some(&version), None).await,
        };
        if let Err(why) = result {
            node.log.line(format_args!("updating this machine to {version}: {why}"));
            let recorded = {
                let held = node.updates.held.lock().expect("never poisoned");
                held.attempt.version.as_deref() == Some(&version) && held.attempt.delay.attempts > attempt
            };
            if !recorded {
                retry::failed(&node, Failed { version, target: state.kernel_target, why, reason: None });
            }
            report(&node);
        }
    });
}

/// The newest release on this machine's line, newer than what runs here,
/// that does not pass the hubs' major version.
pub(crate) fn within(state: &State) -> Option<String> {
    fastctx_update::state::automatic(state)
}

/// The hubs told of versions anew.
pub(crate) fn heard(node: &Arc<Node>) {
    auto(node);
    doctor::heard(node);
}
