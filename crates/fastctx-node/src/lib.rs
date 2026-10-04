//! The node: the one long-lived process on each machine, and the only writer
//! of the World's state there.
//!
//! It holds the World's lock for as long as it runs, so a machine has one
//! node per World; it listens on the World's local endpoint, where every
//! client (commands, the MCP adapter of each agent session, the TUI, the
//! supervisors of running jobs) reaches it; it holds the machine's link to
//! the hub when the machine has joined a World with others; and on start
//! it records what happened while it was away, such as jobs whose
//! supervisor did not survive a restart of the machine.

mod claude;
mod codex;
mod colleagues;
mod copy;
mod inbox;
mod jobs;
mod log;
mod look;
mod machine;
mod residents;
mod server;
mod speech;
mod talk;
mod turns;
mod watch;
mod who;
mod world;
mod write;

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context as _, anyhow};
use fastctx_hosts::codex::{CodexHost, launch};
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
    /// This machine's name; it changes only when the machine joins a World.
    machine: RwLock<MachineName>,
    jobs: jobs::Jobs,
    residents: residents::Residents,
    /// This machine's Codex app-server, holding the Codex residents the
    /// World started.
    codex: Arc<CodexHost>,
    unclaimed: codex::Unclaimed,
    /// Claude sessions, as the World's plugin reports them.
    claude: claude::Claude,
    watchers: watch::Watchers,
    log: Arc<log::Log>,
    /// Present while the machine is a member of a World with others.
    link: RwLock<Option<Arc<Link>>>,
}

impl Node {
    pub(crate) fn machine(&self) -> MachineName {
        self.machine.read().expect("never poisoned").clone()
    }

    pub(crate) fn set_machine(&self, name: MachineName) {
        *self.machine.write().expect("never poisoned") = name;
    }

    pub(crate) fn link(&self) -> Option<Arc<Link>> {
        self.link.read().expect("never poisoned").clone()
    }
}

/// Runs the node of the World kept under `fastctx_root` until the process
/// is ended.
pub async fn run(fastctx_root: &Path) -> anyhow::Result<()> {
    let world = WorldDir::under(fastctx_root);
    ensure_private_dir(world.path()).with_context(|| format!("preparing {}", world.path().display()))?;
    let _lock = FileLock::try_acquire(&world.node_lock())?
        .ok_or_else(|| anyhow!("a node is already running for the World in {}", world.path().display()))?;
    let log = Arc::new(log::Log::open(&world.node_log())?);
    // The node usually has no terminal: a panic in any of its tasks is
    // written to its log, or it would leave no trace.
    let panics = log::Log::open(&world.node_log())?;
    std::panic::set_hook(Box::new(move |info| panics.line(format_args!("panicked: {info}"))));
    let machine = machine::name(&world)?;
    log.line(format_args!("node {} starting as {machine}", env!("CARGO_PKG_VERSION")));

    let reconciled = jobs::reconcile(&world)?;
    for path in &reconciled.interrupted {
        log.line(format_args!("marked interrupted: {}", path.display()));
    }
    for (path, error) in &reconciled.problems {
        log.line(format_args!("could not check {}: {error}", path.display()));
    }

    let (residents, problems) = residents::Residents::load(&world)?;
    for (path, error) in &problems {
        log.line(format_args!("could not read {}: {error}", path.display()));
    }

    let membership = world::load(&world).context("reading this machine's membership")?;
    let endpoint = Endpoint::for_world(world.path())?;
    let mut listener = Listener::bind(&endpoint)?;
    log.line(format_args!("listening at {endpoint}"));

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
    let (codex, codex_events) = CodexHost::new(
        launch::codex_home(&home, &environment),
        world.path().join("codex"),
        environment,
        Arc::new(move |line| codex_log.line(line)),
    );
    let node = Arc::new(Node {
        root: fastctx_root.to_path_buf(),
        world,
        machine: RwLock::new(machine),
        jobs: jobs::Jobs::new(),
        residents,
        codex,
        unclaimed: codex::Unclaimed::default(),
        claude: claude::Claude::default(),
        watchers: watch::Watchers::default(),
        log,
        link: RwLock::new(None),
    });
    if let Some((membership, key)) = membership {
        node.log.line(format_args!("a member of the World {} as {}", membership.world, membership.name));
        world::start(&node, membership, key).context("starting the link to the hub")?;
    }
    codex::adopt(&node);
    codex::listen(node.clone(), codex_events);
    claude::start(&node).context("writing the World's Claude Code plugin")?;

    // What a node that stopped left owed, now that letters can be sent.
    turns::recover(&node).await;
    for (path, error) in inbox::settle(&node).await? {
        node.log.line(format_args!("could not tell what {} owes: {error}", path.display()));
    }

    loop {
        match listener.accept().await {
            Ok(connection) => {
                tokio::spawn(server::serve(node.clone(), connection));
            }
            // One client failing to connect does not stop the others.
            Err(error) => node.log.line(format_args!("accepting a connection: {error}")),
        }
    }
}
