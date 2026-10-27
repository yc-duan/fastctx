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

mod account_records;
pub mod activity;
mod admission;
mod admitted;
mod adoption;
mod afoot;
mod again;
mod agent_activity;
mod agents;
mod around;
mod asked;
mod assists;
mod attach;
mod attachment_files;
mod attachments;
mod authorization;
mod books;
mod claude;
pub mod claude_control_claim;
pub mod claude_native_goal;
pub mod claude_request;
mod codex;
mod codex_title;
mod collab;
mod colleagues;
mod completion_debt;
mod completion_delivery;
mod composer;
mod consent;
pub mod context;
mod control;
mod control_letters;
mod control_posts;
mod control_queue;
mod copy;
mod data;
mod delivery;
mod desk;
mod devices;
mod facts;
mod features;
mod file_events;
mod file_read;
mod file_search;
mod firewall;
mod focus;
mod forget;
mod forks;
mod gate_waits;
mod gates;
mod gather;
mod goals;
pub mod handoff;
mod held_history;
mod here;
mod homes;
mod hub_arrival;
mod hub_moving;
mod inbox;
mod initializing;
mod intake;
mod job_reader;
mod job_receipts;
mod job_watch;
mod jobs;
mod labels;
mod learned;
mod leases;
mod ledger;
mod letters;
mod lifecycle;
mod lifetime;
mod lines;
mod log;
mod login_identity;
mod logins;
mod look;
mod machine;
mod managed;
mod management;
mod mcp;
mod memory;
mod obligations;
mod on;
mod on_delivery;
mod on_steps;
mod on_stop;
mod outbox;
mod own_collab;
mod pages;
mod plan;
mod profile;
mod program_collection;
mod proxy_setting;
mod ranked;
mod reach;
mod received;
mod recovery;
mod removed;
mod renewal;
mod renewal_legacy;
pub mod replacement;
mod resident_storage;
mod residents;
mod retention;
mod run_facts;
mod run_receipts;
mod run_stops;
mod scratch;
mod sentinels;
mod server;
mod server_invitation;
mod session_live;
mod session_options;
mod session_record;
mod shown;
mod signals;
mod since;
mod speech;
mod stats;
mod step_command;
mod stop;
mod stops;
mod stuck;
mod talk;
pub mod thumbnail;
pub mod timers;
mod town;
mod turn_evidence;
mod turns;
mod uninstall;
mod unpublished;
mod unstarted;
mod updates;
mod usage;
mod user_images;
mod voice;
mod waiting;
mod watch;
mod watchdog;
mod who;
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
    /// This machine's name, the same on every hub, and the names it had: it
    /// changes when a computer on no hub logs in under another, and when a
    /// hub renames it (`world::follow_rename`).
    machine: RwLock<machine::MachineFile>,
    jobs: jobs::Jobs,
    timers: std::sync::OnceLock<Arc<timers::store::Store>>,
    turn_activity: turn_evidence::Activities,
    goals: goals::Services,
    sentinel_waits: Arc<dyn turn_evidence::waiting::SentinelWaits>,
    sentinels: sentinels::Services,
    mcp_keeping: std::sync::Mutex<()>,
    job_watches: job_watch::Watches,
    disk: ranked::Disk,
    /// Each conversation's place here, deleted alone (`data::places`).
    places: data::places::Places,
    disk_changed: tokio::sync::Notify,
    disk_due: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    job_readers: std::sync::Mutex<std::collections::HashMap<PathBuf, usize>>,
    residents: residents::Residents,
    contexts: context::Updates,
    context_deliveries: context::Deliveries,
    scratch_deliveries: scratch::Deliveries,
    native_context: codex::native_context::Observers,
    /// This machine's Codex app-server, holding the Codex residents the
    /// World started.
    codex: codex::hosts::CodexHosts,
    /// What this machine knows of its agents' logins, and the scans and
    /// changes of them under way (`logins`).
    logins: logins::Logins,
    unclaimed: codex::Unclaimed,
    /// Threads Codex spawned as agents of residents' threads
    /// (`codex::spawned`).
    codex_agents: codex::spawned::Agents,
    /// Claude sessions, as the World's plugin reports them.
    claude: claude::Claude,
    desk: desk::Desk,
    watchers: watch::Watchers,
    attached: lifecycle::attached::Attached,
    idle_probes: lifecycle::cooldown::Cooldowns,
    retrying: lifecycle::retry::Retrying,
    composers: composer::Composers,
    actions: control_queue::Actions,
    file_searches: file_search::Searches,
    line_sources: lines::Sources,
    log: Arc<log::Log>,
    /// The link to each hub this computer is logged in to (`reach`).
    links: RwLock<std::collections::BTreeMap<HubId, Arc<Link>>>,
    /// Moves whenever the ways to other machines may have changed, on any
    /// link (`Node::reopened`).
    ways: tokio::sync::watch::Sender<u64>,
    /// Moves whenever a hub's view may have changed, or this computer's name
    /// did: what is afoot here is told again then (`afoot`).
    views: tokio::sync::watch::Sender<u64>,
    /// Moves whenever what an interface shows of the World may have changed,
    /// beyond the residents and jobs here (`shown`).
    shown: tokio::sync::watch::Sender<u64>,
    memory_changes: tokio::sync::watch::Sender<u64>,
    /// What the other machines said of themselves (`facts`).
    facts: facts::Facts,
    /// Deletions of conversations still to be asked of hubs (`forget`).
    forget_asks: forget::Asks,
    /// What of the node's work is stuck, as it last looked at itself.
    stuck: stuck::Stuck,
    /// What the agents' logins here were used for (`usage`).
    usage: usage::Usage,
    /// Held while the projects' ledgers are taken or written, one at a time.
    syncing: ranked::Syncing,
    ledger_updates: ledger::syncing::Updates,
    /// Held while standalone settings are read and replaced.
    writing: ranked::Writing,
    /// What went out with an answer of a World tool and is not yet known to
    /// have been taken: no other answer hands it over meanwhile.
    taking: std::sync::Mutex<std::collections::HashSet<(fastctx_types::ResidentId, fastctx_types::NoticeId)>>,
    /// Stops asked for while words were on their way, kept until the turn
    /// those words start is known (`stops`).
    stops: stops::Stops,
    /// The `on` jobs here whose sender is following them now: their end
    /// goes to it, not in a letter, unless it leaves first (`on`).
    following: std::sync::Mutex<std::collections::HashMap<PathBuf, usize>>,
    /// Each job's end is claimed and told by one at a time.
    job_ends: gates::Gates,
    /// Each destination of a copy is written by one copy at a time (`copy`).
    copying: gates::Gates,
    /// What copies here read of files, for the tries that go on after them.
    learned: learned::Learned,
    /// Words coming for residents here, and for residents elsewhere that
    /// work here, for the `run` calls waiting.
    words: signals::Signals,
    /// Turns of residents here, and of residents elsewhere that work here,
    /// interrupted: the job a `run` call of the turn waits for stops.
    interrupts: signals::Signals,
    /// Jobs run for residents here on the machines they work on, answered
    /// whole, until their hosts have the answers (`here`).
    dispatched: here::Dispatched,
    births: here::Births,
    /// The jobs `on` sent from here to other machines, by machine and id,
    /// as a letter telling one's end finds them (`on`).
    on_deliveries: on_delivery::Deliveries,
    received: received::Received,
    control_letters: control_letters::Receiving,
    receipts_changed: tokio::sync::Notify,
    outbox: outbox::Outbox,
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
    /// Who watches the interface's optional parts (`features`).
    features: features::Switches,
    own_collab: own_collab::Keeping,
    renewal: renewal::Startup,
    /// New versions of Fastctx here (`updates`).
    updates: updates::Updates,
    supervised: bool,
    lifetime: fastctx_types::node_lifetime::NodeLifetime,
    /// When this node started.
    started: jiff::Timestamp,
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

    /// What an interface shows of the World may have changed.
    pub(crate) fn memory_changed(&self) {
        self.memory_changes.send_modify(|value| *value = value.wrapping_add(1));
        self.shown_changed();
    }

    pub(crate) fn shown_changed(&self) {
        self.shown.send_modify(|changes| *changes = changes.wrapping_add(1));
    }

    /// A hub's view may have changed, or this computer's name did: what is
    /// afoot here, and what an interface shows, may change with it.
    pub(crate) fn views_changed(&self) {
        self.views.send_modify(|changes| *changes = changes.wrapping_add(1));
        self.shown_changed();
    }

    pub(crate) fn machine(&self) -> MachineName {
        self.machine.read().expect("never poisoned").name.clone()
    }

    /// Whether `machine` names this computer: its name now, or one it had.
    /// What was written under a name it had, a conversation's records, a
    /// job's, a letter, is this computer's all the same.
    pub(crate) fn is_me(&self, machine: &MachineName) -> bool {
        self.machine.read().expect("never poisoned").is(machine)
    }

    /// Takes the name and the names it had that `machine.json` keeps now.
    pub(crate) fn set_names(&self, names: machine::MachineFile) {
        self.residents.set_here(names.name.clone());
        *self.machine.write().expect("never poisoned") = names;
    }

    /// This computer's name and the names it had.
    pub(crate) fn machine_names(&self) -> machine::MachineFile {
        self.machine.read().expect("never poisoned").clone()
    }

    /// The names machines answer to, as this computer knows them: its own,
    /// and the names the machines on its hubs had (`world-network.md`, 机器的
    /// 名字).
    pub(crate) fn names(&self) -> fastctx_types::layout::Names {
        let own = self.machine.read().expect("never poisoned").clone();
        let mut renamed: Vec<Vec<MachineName>> = Vec::new();
        for link in self.links() {
            for member in link.view().members.iter().filter(|member| !member.was.is_empty()) {
                if own.is(&member.name) || renamed.iter().any(|names| names.contains(&member.name)) {
                    continue;
                }
                renamed.push(std::iter::once(member.name.clone()).chain(member.was.iter().cloned()).collect());
            }
        }
        fastctx_types::layout::Names { me: own.name, was: own.was, renamed }
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

/// This node took the World: the successor it said it was is not waiting
/// any more, unless another said so since.
fn no_longer_waiting(world: &WorldDir) {
    let named = std::fs::read(world.node_successor())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<fastctx_types::ProcessIdentity>(&bytes).ok());
    if named.is_some_and(|named| fastctx_platform::current_identity().is_ok_and(|me| me == named)) {
        let _ = std::fs::remove_file(world.node_successor());
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

/// Another node serves the World already: this one ends, having taken
/// nothing of it.
#[derive(Debug)]
pub struct AlreadyServed(pub PathBuf);

impl std::fmt::Display for AlreadyServed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "a node is already running for the World in {}", self.0.display())
    }
}

impl std::error::Error for AlreadyServed {}

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
    // A node that waits to take the World says so first: between the node
    // before it going and this one listening, nobody answers, and a client
    // that comes then waits for this one rather than giving up.
    let waits = supervision.is_some() || renewal.successor;
    if waits {
        ensure_private_dir(world.path()).with_context(|| format!("preparing {}", world.path().display()))?;
        let me = serde_json::to_vec(&fastctx_platform::current_identity()?).expect("identities serialize");
        fastctx_platform::publish(&world.node_successor(), &me, fastctx_platform::Permissions::Private)
            .context("saying this node waits to take the World")?;
    }
    let supervision = match supervision {
        Some(supervision) => {
            Some(tokio::task::block_in_place(|| supervision.prepare()).context("talking to the guardian")?)
        }
        None => None,
    };
    ensure_private_dir(world.path()).with_context(|| format!("preparing {}", world.path().display()))?;
    let _lock = match waits {
        // A successor prepares, then waits to be let in, then waits for the
        // node before it to let the World go.
        true => {
            let lock = world.node_lock();
            tokio::task::spawn_blocking(move || FileLock::acquire(&lock)).await??
        }
        // A Linux service manager starts its node again the moment one
        // ends, as when a node hands the World to the newer program it
        // renewed to: the one it starts waits a while for the World rather
        // than ending at once, refused, to be started again.
        false if cfg!(target_os = "linux") && std::env::var_os("INVOCATION_ID").is_some() => {
            let lock = world.node_lock();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                if let Some(lock) = FileLock::try_acquire(&lock)? {
                    break lock;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(AlreadyServed(world.path().to_path_buf()).into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
        false => FileLock::try_acquire(&world.node_lock())?.ok_or_else(|| AlreadyServed(world.path().to_path_buf()))?,
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
    let panics = log.clone();
    std::panic::set_hook(Box::new(move |info| panics.line(format_args!("panicked: {info}"))));
    let machine = machine::names(&world)?;
    log.line(format_args!("node {} starting as {}", env!("CARGO_PKG_VERSION"), machine.name));
    let lifetime = lifetime::inspect(supervision.is_some())
        .inspect_err(|error| log.line(format_args!("could not check the node's process containment: {error}")))
        .context("checking the node's process containment")?;
    if let Some(warning) = lifetime.warning() {
        log.line(&warning);
        eprintln!("fastctx: {warning}");
    }
    let fastctx = world_program::prepare(fastctx_root).context("preparing the stable World program")?;

    // An older World's records go into one directory per conversation before
    // anything reads them.
    recovery::prepare(&world, &log);
    let jobs_reconciled = recovery::reconcile(&world, &log);

    let (residents, problems) = residents::Residents::load(&world)?;
    residents.set_here(machine.name.clone());
    let unfinished = residents.unfinished();
    for (path, error) in &problems {
        log.line(format_args!("could not read {}: {error}", path.display()));
    }

    // The proxy chosen in Fastctx goes before any hub is dialled.
    if let Err(error) = proxy_setting::take_kept(&world) {
        log.line(format_args!("{error}; the system's proxy is used"));
    }
    let memberships = world::load(&world).context("reading this computer's memberships")?;
    let endpoint = Endpoint::for_world(world.path())?;
    let mut listener = Listener::bind(&endpoint)?;
    log.line(format_args!("listening at {endpoint}"));
    if waits {
        no_longer_waiting(&world);
    }
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
    let (codex, codex_heard) = codex::hosts::CodexHosts::new(
        launch::codex_home(&home, &environment),
        world.path().join("codex"),
        environment,
        Arc::new(move |line| codex_log.line(line)),
    );
    let claude = claude::Claude::new(&world)?;
    // Built cyclically so the goal services can hold a weak handle back to the
    // node they reach account records and context through.
    let node = Arc::new_cyclic(|weak: &std::sync::Weak<Node>| Node {
        root: fastctx_root.to_path_buf(),
        world,
        fastctx,
        machine: RwLock::new(machine),
        jobs: jobs::Jobs::new(),
        timers: Default::default(),
        turn_activity: Default::default(),
        goals: goals::connected(weak.clone()),
        sentinel_waits: Arc::new(sentinels::goal_seams::Waits(weak.clone())),
        sentinels: sentinels::connected(weak.clone()),
        mcp_keeping: std::sync::Mutex::new(()),
        job_watches: Default::default(),
        contexts: Default::default(),
        context_deliveries: Default::default(),
        scratch_deliveries: Default::default(),
        native_context: Default::default(),
        disk: Default::default(),
        places: Default::default(),
        disk_changed: Default::default(),
        disk_due: Default::default(),
        job_readers: Default::default(),
        residents,
        codex,
        logins: logins::Logins::default(),
        unclaimed: codex::Unclaimed::default(),
        codex_agents: Default::default(),
        claude,
        desk: desk::Desk::default(),
        watchers: watch::Watchers::default(),
        attached: Default::default(),
        idle_probes: Default::default(),
        retrying: Default::default(),
        composers: composer::Composers::default(),
        actions: Default::default(),
        file_searches: file_search::Searches::default(),
        line_sources: lines::Sources::default(),
        log,
        links: RwLock::new(std::collections::BTreeMap::new()),
        ways: tokio::sync::watch::channel(0).0,
        views: tokio::sync::watch::channel(0).0,
        shown: tokio::sync::watch::channel(0).0,
        memory_changes: tokio::sync::watch::channel(0).0,
        facts: facts::Facts::default(),
        forget_asks: forget::Asks::default(),
        stuck: Default::default(),
        usage: usage::Usage::default(),
        syncing: Default::default(),
        ledger_updates: Default::default(),
        writing: Default::default(),
        taking: Default::default(),
        stops: Default::default(),
        following: Default::default(),
        job_ends: gates::Gates::new(fastctx_platform::doing::Rank::JOB_END, "job end"),
        copying: gates::Gates::new(fastctx_platform::doing::Rank::COPYING, "copying"),
        learned: Default::default(),
        words: Default::default(),
        interrupts: Default::default(),
        dispatched: Default::default(),
        births: Default::default(),
        on_deliveries: Default::default(),
        received: Default::default(),
        control_letters: Default::default(),
        receipts_changed: Default::default(),
        outbox: Default::default(),
        on_stopping: Default::default(),
        on_delivering: Default::default(),
        notifying_jobs: Default::default(),
        effects_ready: std::sync::atomic::AtomicBool::new(false),
        effects_changed: tokio::sync::Notify::new(),
        // A watcher that falls behind tells its session to list again, so
        // the capacity only bounds how many changes are told one by one.
        tools: tokio::sync::broadcast::channel(64).0,
        handoff: Arc::new(handoff::Handoff::default()),
        features: features::Switches::default(),
        own_collab: own_collab::Keeping::default(),
        renewal,
        updates: Default::default(),
        supervised: supervision.is_some(),
        lifetime,
        started: jiff::Timestamp::now(),
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
    run_stops::resume(&node);
    codex::adopt(&node).await;
    codex::listen(node.clone(), codex_heard.events);
    logins::limits::listen(node.clone(), codex_heard.limits);
    logins::heard::listen(node.clone(), codex_heard.said);
    codex::replay(&node).await;
    let mut unfinished: std::collections::BTreeSet<_> = unfinished.into_iter().collect();
    unfinished.extend(node.residents.unfinished());
    // The sessions of a conversation a stopped node left deleted halfway
    // are passed over: what they report is no one's now.
    claude::replay(&node).await;
    claude::recover_startups(&node);
    claude::start(&node).context("writing the World's Claude Code plugin")?;

    // What a node that stopped left owed, now that letters can be sent: the
    // turns it stopped under end, what it was handing over is settled, and
    // the ends of turns and jobs not yet told are told.
    speech::restore(&node).await;
    unfinished.extend(node.residents.unfinished());
    turns::recover(&node, unfinished.into_iter().collect()).await;
    composer::recover(&node).await;
    speech::recover(&node).await;
    control_queue::recover(&node);
    node.effects_ready.store(true, std::sync::atomic::Ordering::Release);
    node.effects_changed.notify_waiters();
    stops::recover(&node);
    composer::resume(&node);
    lifecycle::start(&node);
    recovery::start(&node, jobs_reconciled);
    timers::start(&node);
    goals::start(&node);
    sentinels::start(&node);
    on_delivery::resume(&node);
    received::resume(&node);
    control_letters::resume(&node);
    job_receipts::start(&node);
    obligations::start(&node);
    retention::start(&node).context("starting command artifact retention")?;
    profile::session::start(&node);
    context::start(&node);
    updates::start(&node);
    afoot::start(&node);
    usage::watch(&node);
    forget::start(&node);
    let jobs_node = node.clone();
    tokio::spawn(async move {
        let mut again = again::Again::default();
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
            tokio::time::sleep(again.next()).await;
        }
    });

    // What an old fastctx left here is put away in the background; nothing
    // still in use is touched (`world-upgrade.md`).
    let cleaning = node.clone();
    tokio::task::spawn_blocking(move || legacy(&cleaning));

    // An apply cut short holds copies of the hosts' settings until it is
    // finished; they go as soon as the node is up (`world-run.md`, apply).
    // Then the World's tools in each login are brought up to what this
    // fastctx writes (`world-agent-logins.md`, 接入跟着版本走).
    let integrating = node.clone();
    tokio::spawn(async move {
        let settling = integrating.clone();
        let settled = tokio::task::spawn_blocking(move || {
            for problem in fastctx_install::apply::settle(&settling.root) {
                settling.log.line(format_args!("settling an interrupted apply: {problem}"));
            }
            login_identity::refresh(&settling);
        })
        .await;
        if let Err(error) = settled {
            integrating.log.line(format_args!("settling the World's tools in the logins: {error}"));
        }
        logins::refresh::all(&integrating).await;
        logins::watch::start(&integrating);
    });

    if let Some(supervision) = supervision {
        handoff::follow(node.clone(), supervision);
    }

    renewal::resume(&node);

    stuck::start(&node);
    let watchdog = watchdog::Watchdog::start(node.log.clone()).context("starting the control-loop watchdog")?;
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
