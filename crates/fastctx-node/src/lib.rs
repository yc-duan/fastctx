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

mod account_features;
mod account_records;
pub mod activity;
mod activity_book;
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
mod blocking;
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
pub use composer::compaction::compact_document as compact_composer_document;
mod consent;
pub mod context;
mod control;
mod control_letters;
mod control_long;
mod control_posts;
mod control_queue;
pub mod conversation_ingest;
mod copy;
mod cut_history;
mod data;
mod delivery;
mod desk;
mod devices;
mod directory_changes;
mod ending;
#[cfg(test)]
mod future_sizes;
pub use ending::flush_log;
mod asset_preservation;
mod content_upkeep;
mod conversation_store;
mod facts;
mod features;
mod file_events;
mod file_read;
mod file_search;
mod firewall;
mod focus;
mod forget;
mod forks;
mod foundation;
mod gate_waits;
mod gates;
mod gather;
mod goals;
pub mod handoff;
mod held_history;
mod here;
mod history;
mod homes;
mod hub_arrival;
mod hub_moving;
mod inbox;
mod initializing;
mod inspect;
mod intake;
mod job_accounts;
mod job_files;
mod job_metadata;
mod job_reader;
mod job_receipts;
mod job_watch;
mod jobs;
mod labels;
mod layers;
mod learned;
mod leases;
mod ledger;
mod letters;
mod lifecycle;
mod lifetime;
mod limits;
mod lines;
mod local_subscriptions;
mod log;
mod login_identity;
mod logins;
mod look;
mod machine;
mod managed;
mod management;
mod mcp;
mod memory_room;
mod obligations;
mod on;
mod on_delivery;
mod on_steps;
mod on_stop;
mod outbox;
mod own_collab;
mod passes;
mod pages;
mod placement;
mod plan;
mod plan_file;
mod plan_reads;
mod profile;
mod program_collection;
mod proxy_setting;
mod ranked;
mod reach;
mod received;
mod record_access;
mod record_database;
mod record_gates;
mod record_migration;
mod record_ownership;
mod record_payloads;
mod record_value;
mod records;
mod recovery;
mod removed;
mod renewal;
mod renewal_legacy;
mod reopen;
pub mod replacement;
mod replicas;
mod residents;
mod retention;
mod run_facts;
mod run_receipts;
mod run_stops;
mod said;
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
mod skills;
mod speech;
mod startup;
mod stats;
mod step_command;
mod stop;
mod stops;
mod stuck;
mod submitted;
mod talk;
pub mod thumbnail;
mod timeline;
pub mod timers;
mod to_end;
mod town;
mod turn_evidence;
mod turn_slots;
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
mod workers;
pub use workers::ROLES as WORKER_ROLES;
mod world;
mod world_format;
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
    database: fastctx_storage::Database,
    /// Durable service work submitted with its triggering state change.
    durable_jobs: Arc<fastctx_jobs::Jobs>,
    workers: workers::Pools,
    /// The unified data foundation's engines on `database`, when it opened
    /// (`foundation`).
    foundation: Option<foundation::Foundation>,
    /// The content database: what this machine holds of conversations and
    /// other streams as items, read a page at a time (`timeline`).
    content: Option<fastctx_content::Content>,
    ingestion: std::sync::OnceLock<Arc<conversation_ingest::watcher::Service>>,
    book_database: fastctx_storage::Database,
    record_migration: record_migration::Service,
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
    plan_files: plan_file::PlanFiles,
    copies: placement::copies::Copies,
    context_bodies: context::memory::Bodies,
    replicas: replicas::Copies,
    /// What this machine keeps in memory of each history scope, when it
    /// has no network (`history::shelf`).
    history_shelves: history::shelf::Shelves,
    history_imports: history::import::Imports,
    /// What this machine keeps in memory of the ledgers it writes alone
    /// (`ledger::alone`).
    alone_ledgers: ledger::alone::Ledgers,
    /// What each hub was last told of each conversation's records
    /// (`record_ownership::Registered`).
    registered: record_ownership::Registered,
    /// The latest read of each conversation's collaboration asked for here
    /// (`colleagues::tree::Reads`).
    collaboration_reads: colleagues::tree::Reads,
    collaboration_followed: colleagues::live::Followed,
    cut_histories: cut_history::CutHistories,
    plan_reads: plan_reads::Reads,
    skill_copies: skills::copies::Copies,
    skill_index: skills::cache::Cache,
    locations: placement::location::Locations,
    /// What failing background work has told already.
    said: said::Said,
    job_files: job_files::Files,
    run_dispatches: leases::Registry<PathBuf, fastctx_platform::doing::SyncMutex<()>>,
    artifact_clocks: leases::Registry<PathBuf, fastctx_platform::doing::SyncMutex<()>>,
    /// Each conversation's place here, deleted alone (`data::places`).
    places: data::places::Places,

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
    /// The controls running outside their conversation's action gate
    /// (`control_long`).
    long_controls: control_long::Running,
    /// How many turns each agent login runs at once, and the turns waiting
    /// for room (`turn_slots`).
    turn_slots: turn_slots::Slots,
    /// What the machine keeps free in memory for its agents, and the turns
    /// waiting for it (`memory_room`).
    memory_room: memory_room::Room,
    /// How far the machine is toward its system's limits over the last day.
    limits: limits::Readings,
    /// Whom each job running here works for (`job_accounts`).
    job_accounts: job_accounts::JobAccounts,
    /// Where each account's jobs share the processors, where they do.
    cpu_shares: Option<fastctx_platform::cpu_share::Shares>,
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
    /// The World as the interfaces watching it are told it (`shown::Shown`).
    world_shown: shown::Shown,
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
    births: here::Births,
    /// The jobs `on` sent from here to other machines, by machine and id,
    /// as a letter telling one's end finds them (`on`).
    on_deliveries: on_delivery::Deliveries,
    received: received::Received,
    control_letters: control_letters::Receiving,
    outbox: outbox::Outbox,
    delivery_work: delivery::work::Work,
    on_stopping: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    on_delivering: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    notifying_jobs: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    startup: startup::Recovery,
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
    own_history: history::own_switch::Keeping,
    renewal: renewal::Startup,
    /// New versions of Fastctx here (`updates`).
    updates: updates::Updates,
    supervised: bool,
    lifetime: fastctx_types::node_lifetime::NodeLifetime,
    /// When this node started.
    started: jiff::Timestamp,
}

impl Node {
    pub(crate) async fn ready_for(&self, id: &fastctx_types::ResidentId) {
        self.startup.wait(id).await;
    }

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
        self.contexts.reading.changed(self);
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
    let named = fastctx_platform::disk::read(world.node_successor())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<fastctx_types::ProcessIdentity>(&bytes).ok());
    if named.is_some_and(|named| fastctx_platform::current_identity().is_ok_and(|me| me == named)) {
        let _ = fastctx_platform::disk::remove_file(world.node_successor());
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

/// The stack each thread of the node's runtime gets. Its calls run deep (one
/// that opens a conversation goes through the server, the speech, the host
/// and the login records), and an unoptimized build gives every temporary of
/// a function a slot of its own, so the default 2 MiB leaves a debug build
/// without room. Only the pages a thread touches are committed.
pub const THREAD_STACK: usize = 8 * 1024 * 1024;

pub async fn run_prepared(
    fastctx_root: &Path,
    supervision: Option<handoff::Supervision>,
    successor: Option<&str>,
    coordinator: Option<&fastctx_types::ProcessIdentity>,
    startup_gate: Option<FileLock>,
) -> anyhow::Result<()> {
    // The main thread runs the node's accepting loop: what it reads and
    // writes before it takes the World goes where that may block.
    {
        let (root, successor, coordinator) =
            (fastctx_root.to_path_buf(), successor.map(str::to_owned), coordinator.cloned());
        fastctx_platform::task::blocking(move || -> anyhow::Result<()> {
            anyhow::ensure!(
                !fastctx_platform::disk::exists(root.join("uninstalled.json"))?,
                "this World was uninstalled; reinstall it before starting a node"
            );
            if let Some(coordinator) = &coordinator {
                replacement::validate(
                    &root,
                    successor.as_deref().ok_or_else(|| anyhow!("prepared node has no successor digest"))?,
                    coordinator,
                )?;
            }
            Ok(())
        })
        .await?;
    }
    let world = WorldDir::under(fastctx_root);
    let mut renewal = renewal::Startup::prepare(fastctx_root, successor, coordinator.is_some()).await?;
    // A node that waits to take the World says so first: between the node
    // before it going and this one listening, nobody answers, and a client
    // that comes then waits for this one rather than giving up.
    let waits = supervision.is_some() || renewal.successor;
    if waits {
        let waiting = world.clone();
        fastctx_platform::task::blocking(move || -> anyhow::Result<()> {
            ensure_private_dir(waiting.path()).with_context(|| format!("preparing {}", waiting.path().display()))?;
            let me = serde_json::to_vec(&fastctx_platform::current_identity()?).expect("identities serialize");
            fastctx_platform::publish(&waiting.node_successor(), &me, fastctx_platform::Permissions::Private)
                .context("saying this node waits to take the World")
        })
        .await?;
    }
    let supervision = match supervision {
        Some(supervision) => Some(
            tokio::task::block_in_place(|| {
                let _blocking = fastctx_platform::roles::scoped(fastctx_platform::roles::Role::Blocking);
                supervision.prepare()
            })
            .context("talking to the guardian")?,
        ),
        None => None,
    };
    {
        let preparing = world.clone();
        fastctx_platform::task::blocking(move || {
            ensure_private_dir(preparing.path()).with_context(|| format!("preparing {}", preparing.path().display()))
        })
        .await?;
    }
    let _lock = match waits {
        // A successor prepares, then waits to be let in, then waits for the
        // node before it to let the World go.
        true => {
            let lock = world.node_lock();
            fastctx_platform::task::spawn_blocking(move || FileLock::acquire(&lock)).await??
        }
        // A Linux service manager starts its node again the moment one
        // ends, as when a node hands the World to the newer program it
        // renewed to: the one it starts waits a while for the World rather
        // than ending at once, refused, to be started again.
        false if cfg!(target_os = "linux") && std::env::var_os("INVOCATION_ID").is_some() => {
            let lock = world.node_lock();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                let trying = lock.clone();
                if let Some(lock) = fastctx_platform::task::blocking(move || FileLock::try_acquire(&trying)).await? {
                    break lock;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(AlreadyServed(world.path().to_path_buf()).into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
        false => {
            let lock = world.node_lock();
            fastctx_platform::task::blocking(move || FileLock::try_acquire(&lock))
                .await?
                .ok_or_else(|| AlreadyServed(world.path().to_path_buf()))?
        }
    };
    if renewal.successor && {
        let root = fastctx_root.to_path_buf();
        fastctx_platform::task::blocking(move || fastctx_install::renewal::read(&root))
            .await?
            .is_some_and(|plan| plan.candidate.build != renewal.build)
    } {
        return Ok(());
    }
    renewal.acquired();
    let supervised = supervision.is_some();
    let root = fastctx_root.to_path_buf();
    let (node, mut listener, memberships, codex_heard, jobs_reconciled, _lock) =
        fastctx_platform::task::spawn_blocking(move || {
            // Ownership stays with preparation even if its waiter is cancelled.
            let ownership = _lock;
            let fastctx_root = root;
            let log = Arc::new(log::Log::open(&world.node_log())?);
            // The node usually has no terminal: a panic in any of its tasks is
            // written to its log, or it would leave no trace.
            let panics = log.clone();
            std::panic::set_hook(Box::new(move |info| {
                panics.line(format_args!("panicked: {info}"));
                panics.flush(std::time::Duration::from_millis(500));
            }));
            // What went wrong, written down while it lasts, and how long tasks wait
            // for a thread (`specs/world-engine.md`, 量与告警).
            fastctx_platform::scene::set_place(world.path().join("scenes"));
            let tail = log.clone();
            fastctx_platform::scene::set_tail(move || tail.recent());
            fastctx_platform::task::watch("node");
            // Before anything moves into the World; a newer World is refused here.
            if let Err(error) = world_format::claim(&world) {
                log.line(format_args!("{error}"));
                return Err(anyhow::Error::from(error).context("opening the World"));
            }
            let machine = machine::names(&world)?;
            log.line(format_args!("node {} starting as {}", env!("CARGO_PKG_VERSION"), machine.name));
            let lifetime = lifetime::inspect(supervised)
                .inspect_err(|error| log.line(format_args!("could not check the node's process containment: {error}")))
                .context("checking the node's process containment")?;
            if let Some(warning) = lifetime.warning() {
                log.line(&warning);
                eprintln!("fastctx: {warning}");
            }
            let fastctx = world_program::prepare(&fastctx_root).context("preparing the stable World program")?;

            // An older World's records go into one directory per conversation before
            // anything reads them.
            recovery::prepare(&world, &log);
            let jobs_reconciled = recovery::reconcile(&world, &log);

            let (residents, problems) = residents::Residents::load(&world)?;
            residents.set_here(machine.name.clone());
            let startup = startup::Recovery::new(&residents);
            for (path, error) in &problems {
                log.line(format_args!("could not read {}: {error}", path.display()));
            }

            // The proxy chosen in Fastctx goes before any hub is dialled.
            if let Err(error) = proxy_setting::take_kept(&world) {
                log.line(format_args!("{error}; the system's proxy is used"));
            }
            let memberships = world::load(&world).context("reading this computer's memberships")?;
            let endpoint = Endpoint::for_world(world.path())?;
            let listener = Listener::bind(&endpoint)?;
            log.line(format_args!("listening at {endpoint}"));
            // The processors shared evenly among the accounts whose jobs run here,
            // where the service was given its cgroup (`fastctx_platform::cpu_share`).
            let cpu_shares = match fastctx_platform::cpu_share::Shares::take_up(world.path()) {
                Ok(shares) => shares,
                Err(error) => {
                    log.line(format_args!("sharing the processors among the accounts' jobs: {error}"));
                    None
                }
            };
            if waits {
                no_longer_waiting(&world);
            }
            drop(startup_gate);

            // The app-server runs with the user's own environment, as every program
            // the node starts does, never with whatever started the node. The one
            // exception is which Codex: a `CODEX_HOME` the node itself was given
            // names it, for the node as for the app-server, which reads the same
            // variable.
            let mut environment =
                fastctx_platform::world_process_environment().context("reading the user's environment")?;
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
            let claude = claude::Claude::new();
            let memory_room = memory_room::Room::new(&world);
            let database = record_database::open(&world);
            if let Some(error) = database.fault() {
                log.line(format_args!("node record storage unavailable: {error}"));
            }
            if let Some(report) = database.salvaged() {
                log.line(format_args!(
                    "node record storage was damaged and set aside; what could be read was salvaged:
{report}"
                ));
            }
            if database.fault().is_none() {
                record_database::start_upgrade(&database, log.clone()).context("pinning the World upgrade snapshot")?;
            }
            let foundation = match database.fault() {
                Some(_) => None,
                None => match foundation::Foundation::open(&world, &database, log.clone()) {
                    Ok(foundation) => Some(foundation),
                    Err(error) => {
                        log.line(format_args!("the data foundation is unavailable: {error:#}"));
                        None
                    }
                },
            };
            let content = match fastctx_content::Content::open(world.path()) {
                Ok(content) => Some(content),
                Err(error) => {
                    log.line(format_args!("the content database is unavailable: {error}"));
                    None
                }
            };
            let record_migration =
                record_database::register(&world, &database).context("opening the node record database")?;
            record_migration.report_to(log.clone());
            if let Err(error) = residents.seal(&database) {
                log.line(format_args!("reading the conversations deleted before: {error}"));
            }
            let book_database = activity_book::open(&world);
            if let Some(error) = book_database.fault() {
                log.line(format_args!("activity book storage unavailable: {error}"));
            }
            if let Some(report) = book_database.salvaged() {
                log.line(format_args!(
                    "activity book storage was damaged and set aside; what could be read was salvaged:
{report}"
                ));
            }
            // Built cyclically so the goal services can hold a weak handle back to the
            // node they reach account records and context through.
            let node = Arc::new_cyclic(|weak: &std::sync::Weak<Node>| Node {
                root: fastctx_root.to_path_buf(),
                world,
                durable_jobs: fastctx_jobs::Jobs::new(database.clone()),
                workers: Default::default(),
                database,
                foundation,
                content,
                ingestion: Default::default(),
                book_database,
                record_migration,
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
                plan_files: Default::default(),
                copies: Default::default(),
                context_bodies: Default::default(),
                replicas: Default::default(),
                history_shelves: Default::default(),
                history_imports: Default::default(),
                alone_ledgers: Default::default(),
                registered: Default::default(),
                collaboration_reads: Default::default(),
                collaboration_followed: Default::default(),
                cut_histories: Default::default(),
                plan_reads: Default::default(),
                skill_copies: Default::default(),
                skill_index: Default::default(),
                locations: Default::default(),
                said: Default::default(),
                contexts: Default::default(),
                context_deliveries: Default::default(),
                scratch_deliveries: Default::default(),
                native_context: Default::default(),
                job_files: Default::default(),
                run_dispatches: Default::default(),
                artifact_clocks: Default::default(),
                places: Default::default(),
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
                long_controls: Default::default(),
                turn_slots: Default::default(),
                memory_room,
                limits: Default::default(),
                job_accounts: Default::default(),
                cpu_shares,
                file_searches: file_search::Searches::default(),
                line_sources: lines::Sources::default(),
                log,
                links: RwLock::new(std::collections::BTreeMap::new()),
                ways: tokio::sync::watch::channel(0).0,
                views: tokio::sync::watch::channel(0).0,
                shown: tokio::sync::watch::channel(0).0,
                world_shown: Default::default(),
                memory_changes: tokio::sync::watch::channel(0).0,
                facts: facts::Facts::default(),
                forget_asks: forget::Asks::default(),
                stuck: Default::default(),
                usage: usage::Usage::default(),
                syncing: Default::default(),
                ledger_updates: Default::default(),
                taking: Default::default(),
                stops: Default::default(),
                following: Default::default(),
                job_ends: gates::Gates::new(fastctx_platform::doing::Rank::JOB_END, "job end"),
                copying: gates::Gates::new(fastctx_platform::doing::Rank::COPYING, "copying"),
                learned: Default::default(),
                words: Default::default(),
                interrupts: Default::default(),
                births: Default::default(),
                on_deliveries: Default::default(),
                received: Default::default(),
                control_letters: Default::default(),
                outbox: Default::default(),
                delivery_work: Default::default(),
                on_stopping: Default::default(),
                on_delivering: Default::default(),
                notifying_jobs: Default::default(),
                startup,
                effects_ready: std::sync::atomic::AtomicBool::new(false),
                effects_changed: tokio::sync::Notify::new(),
                // A watcher that falls behind tells its session to list again, so
                // the capacity only bounds how many changes are told one by one.
                tools: tokio::sync::broadcast::channel(64).0,
                handoff: Arc::new(handoff::Handoff::default()),
                features: features::Switches::default(),
                own_collab: own_collab::Keeping::default(),
                own_history: history::own_switch::Keeping::default(),
                renewal,
                updates: Default::default(),
                supervised,
                lifetime,
                started: jiff::Timestamp::now(),
            });
            Ok::<_, anyhow::Error>((node, listener, memberships, codex_heard, jobs_reconciled, ownership))
        })
        .await??;
    let preparing = node.clone();
    fastctx_platform::task::spawn_blocking(move || {
        let node = preparing;
        logins::workbench::know_defaults(&node);
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
            node.log
                .line(format_args!("on the hub {} ({}) as {}", membership.hub_name, membership.id, membership.name));
            world::start(&node, membership).context("starting a link to a hub")?;
        }
        replicas::cleanup::start(&node);
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    codex::listen(node.clone(), codex_heard.events);
    logins::limits::listen(node.clone(), codex_heard.limits);
    logins::heard::listen(node.clone(), codex_heard.said);
    let preparing = node.clone();
    fastctx_platform::task::spawn_blocking(move || {
        let node = preparing;
        claude::start(&node).context("writing the World's Claude Code plugin")?;
        composer::prepare_recovery(&node);
        if node.database.fault().is_none() {
            retention::start(&node).context("starting command artifact retention")?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    let recovering = node.clone();
    fastctx_platform::task::spawn(async move {
        let node = recovering;
        // The queue starts whatever becomes of recovery: controls, answers
        // and waking a host wait on it, and what runs them waits for its
        // conversation to be ready itself. What is started with recovered
        // work below names its own kinds then.
        timeline::register(&node);
        management::register(&node);
        goals::submission::register(&node);
        sentinels::submission::register(&node);
        logins::submitted::register(&node);
        content_upkeep::register(&node);
        outbox::register(&node);
        control_queue::register(&node);
        composer::compaction::register(&node);
        lifecycle::retry::register(&node);
        let (durable_jobs, log) = (node.durable_jobs.clone(), node.log.clone());
        fastctx_platform::task::spawn(async move {
            let (mut again, mut said) = (crate::again::Again::default(), String::new());
            while let Err(error) = durable_jobs.start().await {
                let error = error.to_string();
                if error != said {
                    log.line(format_args!("starting durable jobs; trying again: {error}"));
                    said = error;
                }
                tokio::time::sleep(again.next().max(std::time::Duration::from_secs(1))).await;
            }
        });
        if let Err(error) = node.record_migration.wait_started().await {
            node.log.line(format_args!("record recovery remains unavailable: {error}"));
            node.effects_ready.store(true, std::sync::atomic::Ordering::Release);
            node.effects_changed.notify_waiters();
            return;
        }
        startup::recover(&node).await;
        node.effects_ready.store(true, std::sync::atomic::Ordering::Release);
        node.effects_changed.notify_waiters();
        if node.database.fault().is_some() {
            node.log.line("record recovery is unavailable; durable work remains pending until storage is repaired");
            return;
        }
        let log = node.log.clone();
        if let Err(error) = fastctx_platform::task::spawn_blocking(move || {
            stops::recover(&node);
            delivery::work::recover(&node);
            composer::resume(&node);
            composer::keep(&node);
            lifecycle::start(&node);
            recovery::start(&node, jobs_reconciled);
            goals::start(&node);
            sentinels::start(&node);
            skills::follow(&node);
            on_delivery::resume(&node);
            received::resume(&node);
            control_letters::resume(&node);
            job_receipts::start(&node);
            obligations::start(&node);
            placement::copies::start(&node);
            skills::copies::start(&node);
            plan_file::start(&node);
            profile::session::start(&node);
            context::start(&node);
            updates::start(&node);
            afoot::start(&node);
            usage::watch(&node);
            forget::start(&node);
        })
        .await
        {
            log.line(format_args!("starting recovered work: {error}"));
        }
    });
    // The foundation's tasks start with bulk migration, after the upgrade's
    // backup has released the database.
    let founding = node.clone();
    fastctx_platform::task::spawn(async move {
        if founding.record_migration.wait_started().await.is_err() {
            return;
        }
        if let Some(foundation) = founding.foundation.as_ref() {
            foundation.run(founding.log.clone()).await;
        }
    });
    let jobs_node = node.clone();
    fastctx_platform::task::spawn(async move {
        if jobs_node.record_migration.wait_started().await.is_err() {
            return;
        }
        if jobs_node.database.fault().is_some() {
            return;
        }
        let mut again = again::Again::default();
        loop {
            match inbox::settle_jobs(&jobs_node).await {
                Ok(problems) if problems.is_empty() => break,
                Ok(problems) => {
                    // Records still moving in are not yet, not a problem.
                    for (path, error) in
                        problems.iter().filter(|(_, error)| error.kind() != std::io::ErrorKind::WouldBlock)
                    {
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
    fastctx_platform::task::spawn_blocking(move || legacy(&cleaning));

    // An apply cut short holds copies of the hosts' settings until it is
    // finished; they go as soon as the node is up (`world-run.md`, apply).
    // Then the World's tools in each login are brought up to what this
    // fastctx writes (`world-agent-logins.md`, 接入跟着版本走).
    let integrating = node.clone();
    fastctx_platform::task::spawn(async move {
        let settling = integrating.clone();
        let settled = fastctx_platform::task::spawn_blocking(move || {
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

    ending::start(node.log.clone()).context("starting node stop diagnostics")?;
    stuck::start(&node);
    reopen::start(&node);
    memory_room::start(&node);
    limits::start(&node);
    let watchdog = watchdog::Watchdog::start().context("starting the control-loop watchdog")?;
    // Accepting fails over and over while no descriptor is left: it waits a
    // little longer each time, and says so once, until one is accepted.
    let mut refused = 0u32;
    loop {
        let accepted = tokio::select! {
            // A ready watchdog request precedes a flood of new clients;
            // its scheduling must not depend on random select fairness.
            biased;
            () = watchdog.requested() => {
                if node.links().iter().any(|link| link.failed()) {
                    crate::ending::with_reason(1, "a link task ended unexpectedly; exiting for the service to restart it".into());
                }
                continue;
            },
            accepted = record_migration::after_open(&node, listener.accept()) => accepted,
        };
        match accepted {
            Ok(connection) => {
                refused = 0;
                watchdog.progress();
                fastctx_platform::task::spawn(server::serve(node.clone(), connection));
            }
            // One client failing to connect does not stop the others.
            Err(error) => {
                if refused == 0 {
                    node.log.line(format_args!("accepting a connection: {error}"));
                }
                refused = refused.saturating_add(1);
                let pause = std::time::Duration::from_millis(10u64 << refused.min(7));
                tokio::select! {
                    biased;
                    () = watchdog.requested() => {}
                    () = tokio::time::sleep(pause) => {}
                }
            }
        }
    }
}

#[cfg(feature = "storage-drill")]
#[doc(hidden)]
pub use storage_drill::{migration as record_migration_drill, run as record_drill, transfer as record_transfer_drill};
#[cfg(feature = "storage-drill")]
mod storage_drill;

pub use conversation_store::transfer::status::ConversationImport;

/// Lists durable import progress and rejected offers without modifying the node.
pub fn conversation_imports(root: &std::path::Path) -> std::io::Result<Vec<ConversationImport>> {
    world_format::check(&WorldDir::under(root))?;
    conversation_store::transfer::status::list(root)
}

/// A conversation's records written beside it for another machine to take.
pub struct ExportedConversation {
    pub bundle: std::path::PathBuf,
    /// The export's name between machines: the one taking it keeps it, and
    /// this machine lets the conversation go by it.
    pub fingerprint: String,
}

/// Exports every conversation-owned table into its directory while the node is
/// stopped. Copy that directory with its host records before starting the target.
pub fn export_conversation(root: &std::path::Path, conversation: &str) -> std::io::Result<ExportedConversation> {
    world_format::check(&WorldDir::under(root))?;
    let (bundle, fingerprint) = conversation_store::transfer::exporting::export(root, conversation)?;
    Ok(ExportedConversation { bundle, fingerprint })
}

/// Lets a conversation go from this stopped node once another machine took
/// its export whole, named by that export's fingerprint; says where its
/// directory was set aside.
pub fn release_conversation(
    root: &std::path::Path,
    conversation: &str,
    fingerprint: &str,
) -> std::io::Result<std::path::PathBuf> {
    world_format::check(&WorldDir::under(root))?;
    conversation_store::transfer::release::release(root, conversation, fingerprint)
}
