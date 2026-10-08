//! `fastctx attach`: opens a resident in its host's own interface in this
//! terminal, at the machine it lives on: the same session the World shows,
//! taken up in the official program (`world-gui.md`, 它是什么). It is the
//! user's own act, so it does not work in a command a resident runs.

use anyhow::bail;
use clap::Args;
use fastctx_client::World;
use fastctx_types::{Address, ResidentId};

/// Open a resident in its agent's own interface, in this terminal.
#[derive(Args)]
pub struct AttachArgs {
    /// The resident, such as codex-7f3a, living on this machine.
    resident: String,
}

pub fn run(args: AttachArgs) -> anyhow::Result<()> {
    if let Ok(resident) = std::env::var("FASTCTX_RESIDENT") {
        bail!("fastctx attach is for the user, not for {resident}");
    }
    let resident = match args.resident.parse::<Address>() {
        Ok(address) => address.resident,
        Err(_) => ResidentId::try_from(args.resident.as_str())?,
    };
    let world = World::here()?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let command = runtime.block_on(world.attach(resident))?;
    drop(runtime);
    let status = fastctx_client::host::run(&command)?;
    std::process::exit(status.code().unwrap_or(1));
}
