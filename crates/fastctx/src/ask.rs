//! Asking the node, as a command: it speaks as whoever runs it
//! (`fastctx_client::command_peer`).

use fastctx_client::World;
use fastctx_types::layout::WorldDir;
use fastctx_types::{Address, Call, MachineName, Reply, TownName};

/// Sends one call to this machine's node, starting it if it is not
/// running. Returns the answer and the machine the node runs on.
pub fn call(call: Call) -> anyhow::Result<(Reply, MachineName)> {
    let world = World::here()?;
    Ok(tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(world.call(call))?)
}

/// The town a command means when it names none, for a resident of another
/// machine running it here: the one its `run` was started in. A resident of
/// this machine, or the user, is left to the node, which knows where each
/// of its residents lives now.
pub fn town_from_run() -> Option<TownName> {
    let resident: Address = std::env::var("FASTCTX_RESIDENT").ok()?.parse().ok()?;
    let here = std::fs::read(WorldDir::under(&fastctx_platform::fastctx_root().ok()?).machine_file()).ok()?;
    let here: serde_json::Value = serde_json::from_slice(&here).ok()?;
    if MachineName::try_from(here.get("name")?.as_str()?).ok()? == resident.machine {
        return None;
    }
    TownName::try_from(std::env::var("FASTCTX_TOWN").ok()?).ok()
}
