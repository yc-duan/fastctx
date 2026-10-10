//! The node: the one long-lived process on each machine, and the only writer
//! of the World's state there.
//!
//! It holds the World's lock for as long as it runs, so a machine has one
//! node per World; it listens on the World's local endpoint, where every
//! client (commands, the MCP adapter of each agent session, the TUI, the
//! supervisors of running jobs) reaches it; it holds the machine's link to
//! each hub it is logged in to; and on start
//! it records what happened while it was away, such as jobs whose
//! supervisor did not survive a restart of the machine.

pub mod activity;
mod admission;
mod admitted;
mod agents;
mod around;
mod asked;
mod attach;
mod attachments;
mod claude;
pub mod claude_request;
mod codex;
mod colleagues;
mod composer;
mod consent;
mod control;
mod copy;
mod delivery;
mod desk;
mod devices;
mod file_read;
mod file_search;
pub mod handoff;
mod here;
mod homes;
mod inbox;
mod job_ends;
mod job_watch;
mod jobs;
mod leases;
mod ledger;
mod letters;
mod lifetime;
mod log;
mod look;
mod machine;
mod management;
mod on;
mod on_delivery;
mod on_stop;
mod pages;
mod reach;
mod renewal;
mod renewal_legacy;
pub mod replacement;
mod resident_storage;
mod residents;
mod server;
mod server_invitation;
mod session_live;
mod session_options;
mod session_record;
mod since;
mod speech;
mod stop;
mod talk;
mod town;
mod turns;
mod uninstall;
mod waiting;
mod watch;
mod watchdog;
mod who;
mod words;
mod world;
mod world_program;
mod write;
mod write_command;

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context as _, anyhow};
use fastctx_hosts::codex::launch;
use fastctx_net::ids::HubId;
use fastctx_net::link::Link;
use fastctx_platform::ipc::{Endpoint, Listener};
use fastctx_platform::{FileLock, ensure_private_dir};
use fastctx_types::MachineName;
use fastctx_types::layout::WorldDir;

/// What every connection shares.
pub(crate) struct Node {
    /// Everything fastctx keeps for this user; what `FASTCTX_HOME` names for
    /// the programs the node starts.
    root: PathBuf,
    world: WorldDir,
    /// This program's own copy under the World, which everything the node
    /// starts from itself runs from (`own.rs`).
    fastctx: PathBuf,
    /// This machine's name, the same on every hub; it changes only when a
    /// computer on no hub logs in under another.
    machine: RwLock<MachineName>,
    jobs: jobs::Jobs,
    job_watches: job_watch::Watches,
    residents: residents::Residents,
    /// This machine's Codex app-server, holding the Codex residents the
    /// World started.
    codex: codex::hosts::CodexHosts,
    home_changes: std::sync::Mutex<()>,
    unclaimed: codex::Unclaimed,
    /// Claude sessions, as the World's plugin reports them.
    claude: claude::Claude,
    desk: desk::Desk,
    watchers: watch::Watchers,
    composers: composer::Composers,
    file_searches: file_search::Searches,
    log: Arc<log::Log>,
    /// The link to each hub this computer is logged in to (`reach`).
    links: RwLock<std::collections::BTreeMap<HubId, Arc<Link>>>,
    /// Held while the projects' ledgers are taken or written, one at a time.
    syncing: tokio::sync::Mutex<()>,
    /// Held while a town's record or journals are written from here, from
    /// reading what is there to writing what comes next.
    writing: tokio::sync::Mutex<()>,
    /// What went out with an answer of a World tool and is not yet known to
    /// have been taken: no other answer hands it over meanwhile.
    taking: std::sync::Mutex<std::collections::HashSet<(fastctx_types::ResidentId, fastctx_types::NoticeId)>>,
    /// The `on` jobs here whose sender is following them now: their end
    /// goes to it, not in a letter, unless it leaves first (`on`).
    following: std::sync::Mutex<std::collections::HashMap<PathBuf, usize>>,
    job_ends: job_ends::Ends,
    /// Words coming for residents here, and for residents elsewhere that
    /// work here, for the `run` calls waiting.
    words: words::Words,
    /// Jobs run for residents here on the machines they work on, answered
    /// whole, until their hosts have the answers (`here`).
    dispatched: here::Dispatched,
    births: here::Births,
    /// The jobs `on` sent from here to other machines, by machine and id,
    /// as a letter telling one's end finds them (`on`).
    on_deliveries: on_delivery::Deliveries,
    on_stopping: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    on_delivering: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    notifying_jobs: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    effects_ready: std::sync::atomic::AtomicBool,
    effects_changed: tokio::sync::Notify,
    /// Which resident's offered tools may have changed, or everyone's
    /// (`WatchTools`).
    tools: tokio::sync::broadcast::Sender<Option<fastctx_types::ResidentId>>,
    /// Whether turns may start, while a guardian may ask for the World to
    /// be handed on.
    handoff: Arc<handoff::Handoff>,
    renewal: renewal::Startup,
    supervised: bool,
    lifetime: fastctx_types::node_lifetime::NodeLifetime,
}

impl Node {
    pub(crate) async fn ready(&self) {
        loop {
            let changed = self.effects_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.effects_ready.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            changed.await;
        }
    }
    /// The tools `resident` is offered, or every session here, may have
    /// changed.
    pub(crate) fn tools_changed(&self, resident: Option<&fastctx_types::ResidentId>) {
        let _ = self.tools.send(resident.cloned());
    }

    pub(crate) fn machine(&self) -> MachineName {
        self.machine.read().expect("never poisoned").clone()
    }

    pub(crate) fn set_machine(&self, name: MachineName) {
        *self.machine.write().expect("never poisoned") = name;
    }
}

/// Runs the node of the World kept under `fastctx_root` until the process
/// is ended.
/// Puts away what an old fastctx left on this machine, when this World
/// lives where the old versions did, and logs every step; the record of it
/// is `upgrade-clean.json` beside the World.
fn legacy(node: &Node) {
    match fastctx_install::upgrade::is_legacy_root(&node.root) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            node.log.line(format_args!("could not tell whether an old fastctx lived here: {error}"));
            return;
        }
    }
    let mut places = match fastctx_install::apply::Places::here() {
        Ok(places) => places,
        Err(error) => {
            node.log.line(format_args!("cleaning up after an old fastctx: {error}"));
            return;
        }
    };
    places.root = node.root.clone();
    places.world = node.world.path().to_path_buf();
    match fastctx_install::upgrade::clean(&places) {
        Ok(report) => {
            for path in &report.changed {
                node.log.line(format_args!("put away what an old fastctx left: {}", path.display()));
            }
            for notice in &report.notices {
                node.log.line(format_args!("left what an old fastctx left, {}: {}", notice.location, notice.reason));
            }
            for leftover in &report.leftovers {
                node.log.line(format_args!("an old fastctx's leftover: {leftover}"));
            }
        }
        Err(error) => node.log.line(format_args!("cleaning up after an old fastctx: {error}")),
    }
}

/// Runs the node for the World under `fastctx_root`; `supervision` is the
/// guardian's channel when a guardian keeps it (`handoff`).
pub async fn run(fastctx_root: &Path, supervision: Option<handoff::Supervision>) -> anyhow::Result<()> {
    run_replacing(fastctx_root, supervision, None).await
}

/// Runs a prepared local successor, or the ordinary node when no digest is
/// supplied. A successor waits for the current node to release the World.
pub async fn run_replacing(
    fastctx_root: &Path,
    supervision: Option<handoff::Supervision>,
    successor: Option<&str>,
) -> anyhow::Result<()> {
    run_prepared(fastctx_root, supervision, successor, None, None).await
}

pub async fn run_prepared(
    fastctx_root: &Path,
    supervision: Option<handoff::Supervision>,
    successor: Option<&str>,
    coordinator: Option<&fastctx_types::ProcessIdentity>,
    startup_gate: Option<FileLock>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !fastctx_root.join("uninstalled.json").try_exists()?,
        "this World was uninstalled; reinstall it before starting a node"
    );
    if let Some(coordinator) = coordinator {
        replacement::validate(
            fastctx_root,
            successor.ok_or_else(|| anyhow!("prepared node has no successor digest"))?,
            coordinator,
        )?;
    }
    let world = WorldDir::under(fastctx_root);
    let mut renewal = renewal::Startup::prepare(fastctx_root, successor, coordinator.is_some()).await?;
    let supervision = match supervision {
        Some(supervision) => {
            Some(tokio::task::block_in_place(|| supervision.prepare()).context("talking to the guardian")?)
        }
        None => None,
    };
    ensure_private_dir(world.path()).with_context(|| format!("preparing {}", world.path().display()))?;
    let _lock = match supervision.is_some() || renewal.successor {
        // A successor prepares, then waits to be let in, then waits for the
        // node before it to let the World go.
        true => {
            let lock = world.node_lock();
            tokio::task::spawn_blocking(move || FileLock::acquire(&lock)).await??
        }
        false => FileLock::try_acquire(&world.node_lock())?
            .ok_or_else(|| anyhow!("a node is already running for the World in {}", world.path().display()))?,
    };
    if renewal.successor
        && fastctx_install::renewal::read(fastctx_root)?.is_some_and(|plan| plan.candidate.build != renewal.build)
    {
        return Ok(());
    }
    renewal.acquired();
    let log = Arc::new(log::Log::open(&world.node_log())?);
    // The node usually has no terminal: a panic in any of its tasks is
    // written to its log, or it would leave no trace.
    let panics = log::Log::open(&world.node_log())?;
    std::panic::set_hook(Box::new(move |info| panics.line(format_args!("panicked: {info}"))));
    let machine = machine::name(&world)?;
    log.line(format_args!("node {} starting as {machine}", env!("CARGO_PKG_VERSION")));
    let lifetime = lifetime::inspect(supervision.is_some())
        .inspect_err(|error| log.line(format_args!("could not check the node's process containment: {error}")))
        .context("checking the node's process containment")?;
    if let Some(warning) = lifetime.warning() {
        log.line(&warning);
        eprintln!("fastctx: {warning}");
    }
    let fastctx = world_program::prepare(fastctx_root).context("preparing the stable World program")?;

    let reconciled = jobs::reconcile(&world)?;
    for path in &reconciled.interrupted {
        log.line(format_args!("marked interrupted: {}", path.display()));
    }
    for (path, error) in &reconciled.problems {
        log.line(format_args!("could not check {}: {error}", path.display()));
    }

    let (residents, problems) = residents::Residents::load(&world)?;
    let unfinished = residents.unfinished();
    for (path, error) in &problems {
        log.line(format_args!("could not read {}: {error}", path.display()));
    }

    let memberships = world::load(&world).context("reading this computer's memberships")?;
    let endpoint = Endpoint::for_world(world.path())?;
    let mut listener = Listener::bind(&endpoint)?;
    log.line(format_args!("listening at {endpoint}"));
    drop(startup_gate);

    // The app-server runs with the user's own environment, as every program
    // the node starts does, never with whatever started the node. The one
    // exception is which Codex: a `CODEX_HOME` the node itself was given
    // names it, for the node as for the app-server, which reads the same
    // variable.
    let mut environment = fastctx_platform::world_process_environment().context("reading the user's environment")?;
    if let Some(codex_home) = std::env::var_os("CODEX_HOME") {
        environment.retain(|(name, _)| !name.eq_ignore_ascii_case("CODEX_HOME"));
        environment.push(("CODEX_HOME".into(), codex_home));
    }
    let home = fastctx_platform::user_home().context("finding the home directory")?;
    let codex_log = log.clone();
    let (codex, codex_events) = codex::hosts::CodexHosts::new(
        launch::codex_home(&home, &environment),
        world.path().join("codex"),
        environment,
        Arc::new(move |line| codex_log.line(line)),
    );
    let claude = claude::Claude::new(&world)?;
    let node = Arc::new(Node {
        root: fastctx_root.to_path_buf(),
        world,
        fastctx,
        machine: RwLock::new(machine),
        jobs: jobs::Jobs::new(),
        job_watches: Default::default(),
        residents,
        codex,
        home_changes: Default::default(),
        unclaimed: codex::Unclaimed::default(),
        claude,
        desk: desk::Desk::default(),
        watchers: watch::Watchers::default(),
        composers: composer::Composers::default(),
        file_searches: file_search::Searches::default(),
        log,
        links: RwLock::new(std::collections::BTreeMap::new()),
        syncing: tokio::sync::Mutex::new(()),
        writing: tokio::sync::Mutex::new(()),
        taking: Default::default(),
        following: Default::default(),
        job_ends: Default::default(),
        words: Default::default(),
        dispatched: Default::default(),
        births: Default::default(),
        on_deliveries: Default::default(),
        on_stopping: Default::default(),
        on_delivering: Default::default(),
        notifying_jobs: Default::default(),
        effects_ready: std::sync::atomic::AtomicBool::new(false),
        effects_changed: tokio::sync::Notify::new(),
        // A watcher that falls behind tells its session to list again, so
        // the capacity only bounds how many changes are told one by one.
        tools: tokio::sync::broadcast::channel(64).0,
        handoff: Arc::new(handoff::Handoff::default()),
        renewal,
        supervised: supervision.is_some(),
        lifetime,
    });
    // The third version's projects are kept as this computer's own, to be
    // carried into its account at its hub.
    let (kept, problems) =
        ledger::third::migrate(&node.world, &node.machine()).context("keeping the third version's projects")?;
    for town in kept {
        node.log.line(format_args!("kept {town}, a project of the network's third version, to carry into the hub"));
    }
    for problem in problems {
        node.log.line(format_args!("could not keep a project of the network's third version: {problem}"));
    }
    for problem in ledger::finish(&node.world).context("reading the towns' ledgers")? {
        node.log.line(format_args!("could not finish a ledger entry: {problem}"));
    }
    town::guard::start(&node).context("watching the towns' ledgers")?;
    for membership in memberships {
        node.log.line(format_args!("on the hub {} ({}) as {}", membership.hub_name, membership.id, membership.name));
        world::start(&node, membership).context("starting a link to a hub")?;
    }
    on_stop::resume(&node).await;
    codex::adopt(&node).await;
    codex::listen(node.clone(), codex_events);
    codex::replay(&node).await;
    let mut unfinished: std::collections::BTreeSet<_> = unfinished.into_iter().collect();
    unfinished.extend(node.residents.unfinished());
    claude::replay(&node).await;
    claude::start(&node).context("writing the World's Claude Code plugin")?;

    // What a node that stopped left owed, now that letters can be sent: the
    // turns it stopped under end, what it was handing over is settled, and
    // the ends of turns and jobs not yet told are told.
    speech::restore(&node).await;
    unfinished.extend(node.residents.unfinished());
    turns::recover(&node, unfinished.into_iter().collect()).await;
    composer::recover(&node).await.map_err(anyhow::Error::from)?;
    node.effects_ready.store(true, std::sync::atomic::Ordering::Release);
    node.effects_changed.notify_waiters();
    composer::resume(&node);
    speech::recover(&node).await;
    on_delivery::resume(&node);
    let jobs_node = node.clone();
    tokio::spawn(async move {
        let mut delay = on::Backoff::default();
        loop {
            match inbox::settle_jobs(&jobs_node).await {
                Ok(problems) if problems.is_empty() => break,
                Ok(problems) => {
                    for (path, error) in problems {
                        jobs_node.log.line(format_args!("could not tell what {} owes: {error}", path.display()));
                    }
                }
                Err(error) => jobs_node.log.line(format_args!("recovering job notifications: {error}")),
            }
            tokio::time::sleep(delay.next()).await;
        }
    });

    // What an old fastctx left here is put away in the background; nothing
    // still in use is touched (`world-upgrade.md`).
    let cleaning = node.clone();
    tokio::task::spawn_blocking(move || legacy(&cleaning));

    if let Some(supervision) = supervision {
        handoff::follow(node.clone(), supervision);
    }

    match fastctx_install::renewal::read(&node.root) {
        Ok(Some(plan))
            if plan.phase != fastctx_types::renewal::Phase::Failed
                && (plan.phase != fastctx_types::renewal::Phase::Complete
                    || plan.candidate.build != node.renewal.build) =>
        {
            if let Err(error) = fastctx_install::renewal::request(&node.root, false) {
                node.log.line(format_args!("could not resume executable renewal: {error:#}"));
            }
        }
        Err(error) => node.log.line(format_args!("could not read executable renewal: {error}")),
        _ => {}
    }

    let watchdog = watchdog::Watchdog::start(&node.world.node_log()).context("starting the control-loop watchdog")?;
    loop {
        let accepted = tokio::select! {
            // A ready watchdog request precedes a flood of new clients;
            // its scheduling must not depend on random select fairness.
            biased;
            () = watchdog.requested() => {
                if node.links().iter().any(|link| link.failed()) {
                    node.log.line("a link task ended unexpectedly; exiting for the service to restart it");
                    std::process::exit(1);
                }
                continue;
            },
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok(connection) => {
                watchdog.progress();
                tokio::spawn(server::serve(node.clone(), connection));
            }
            // One client failing to connect does not stop the others.
            Err(error) => node.log.line(format_args!("accepting a connection: {error}")),
        }
    }
}
