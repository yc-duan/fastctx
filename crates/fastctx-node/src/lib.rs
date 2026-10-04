//! The node: the one long-lived process on each machine, and the only writer
//! of the World's state there.
//!
//! It holds the World's lock for as long as it runs, so a machine has one
//! node per World; it listens on the World's local endpoint, where every
//! client (commands, the MCP adapter of each agent session, the TUI, the
//! supervisors of running jobs) reaches it; and on start it records what
//! happened while it was away, such as jobs whose supervisor did not
//! survive a restart of the machine.

mod inbox;
mod jobs;
mod log;
mod machine;
mod residents;
mod server;
mod who;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use fastctx_platform::ipc::{Endpoint, Listener};
use fastctx_platform::{FileLock, ensure_private_dir};
use fastctx_types::MachineName;
use fastctx_types::layout::WorldDir;

/// What every connection shares.
pub(crate) struct Node {
    world: WorldDir,
    machine: MachineName,
    jobs: jobs::Jobs,
    residents: residents::Residents,
    log: log::Log,
}

/// Runs the node of the World kept under `fastctx_root` until the process
/// is ended.
pub async fn run(fastctx_root: &Path) -> anyhow::Result<()> {
    let world = WorldDir::under(fastctx_root);
    ensure_private_dir(world.path()).with_context(|| format!("preparing {}", world.path().display()))?;
    let _lock = FileLock::try_acquire(&world.node_lock())?
        .ok_or_else(|| anyhow!("a node is already running for the World in {}", world.path().display()))?;
    let log = log::Log::open(&world.node_log())?;
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

    for (path, error) in inbox::settle(&world)? {
        log.line(format_args!("could not tell what {} owes: {error}", path.display()));
    }

    let (residents, problems) = residents::Residents::load(&world)?;
    for (path, error) in &problems {
        log.line(format_args!("could not read {}: {error}", path.display()));
    }

    let endpoint = Endpoint::for_world(world.path())?;
    let mut listener = Listener::bind(&endpoint)?;
    log.line(format_args!("listening at {endpoint}"));
    let node = Arc::new(Node { world, machine, jobs: jobs::Jobs::new(), residents, log });
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
