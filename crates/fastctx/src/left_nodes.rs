//! Ending what still holds a World once its service stopped every node it
//! keeps. A node nobody keeps, as a renewal's bridge left serving, would hold
//! the World from the service's node when the service starts again.

use std::path::Path;
use std::time::{Duration, Instant};

use fastctx_platform::FileLock;
use fastctx_types::ProcessIdentity;
use fastctx_types::layout::WorldDir;
use fastctx_types::renewal::Action;

/// How long a node left is given to hand the World over before it is ended.
const GOES_WITHIN: Duration = Duration::from_secs(15);
/// How long the World's lock is waited for once its holder ended.
const LETS_GO_WITHIN: Duration = Duration::from_secs(10);
/// How long the node left is waited for when asked something.
const ANSWER_WITHIN: Duration = Duration::from_secs(10);

/// Ends what still holds the World under `root`: the process ended, if any.
pub(crate) fn end(root: &Path) -> anyhow::Result<Option<ProcessIdentity>> {
    let world = WorldDir::under(root);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let holder = match tokio::time::timeout(ANSWER_WITHIN, fastctx_client::renewal::process(&world)).await {
            Ok(Ok(Some(holder))) => Some(holder),
            // Not listening, or not answering: the process the World's
            // holder record names, while the World's lock is held.
            _ => recorded_holder(&world)?,
        };
        let Some(holder) = holder else { return Ok(None) };
        if fastctx_platform::current_identity().is_ok_and(|me| me == holder) {
            return Ok(None);
        }
        let _ = tokio::time::timeout(ANSWER_WITHIN, fastctx_client::renewal::request(&world, Action::Yield)).await;
        let began = Instant::now();
        while fastctx_platform::is_alive(&holder)? && began.elapsed() < GOES_WITHIN {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if fastctx_platform::is_alive(&holder)? {
            fastctx_platform::terminate::terminate(&holder)?;
        }
        let began = Instant::now();
        while FileLock::try_acquire(&world.node_lock())?.is_none() {
            if began.elapsed() >= LETS_GO_WITHIN {
                anyhow::bail!("process {} ended, and the World's lock is still held", holder.pid);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok(Some(holder))
    })
}

/// The process the World's holder record names, while it lives and the
/// World's lock is held.
fn recorded_holder(world: &WorldDir) -> anyhow::Result<Option<ProcessIdentity>> {
    if FileLock::try_acquire(&world.node_lock())?.is_some() {
        return Ok(None);
    }
    let Ok(bytes) = std::fs::read(world.node_holder()) else { return Ok(None) };
    let Ok(holder) = serde_json::from_slice::<ProcessIdentity>(&bytes) else { return Ok(None) };
    Ok(fastctx_platform::is_alive(&holder)?.then_some(holder))
}
