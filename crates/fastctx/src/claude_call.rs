//! A mod invocation owns one helper process. Claude Code kills that process
//! when its stream closes, the call is cancelled, or the mod is unloaded.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context as _, bail};
use clap::Subcommand;
use fastctx_client::Client;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::{Call, ProcessIdentity, Reply};

#[derive(Subcommand)]
pub enum Command {
    Begin { world: PathBuf, tool_use_id: String },
    Finish { world: PathBuf, tool_use_id: String },
}

pub fn run(command: Command) -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async move {
        match command {
            Command::Begin { world, tool_use_id } => {
                let registration: serde_json::Value = serde_json::from_str(&input)?;
                let host: ProcessIdentity = serde_json::from_value(
                    registration.get("process").cloned().context("the Claude registration has no host process")?,
                )?;
                if !fastctx_platform::is_alive(&host)? {
                    bail!("the Claude host has exited");
                }
                let owner = fastctx_platform::current_identity()?;
                request(&world, Call::ClaudeCall { tool_use_id, registration, owner: owner.clone() }).await?;
                let mut stdout = std::io::stdout().lock();
                writeln!(stdout, "{}", serde_json::to_string(&owner)?)?;
                stdout.flush()?;
                drop(stdout);
                // Also end if the host dies without unloading its modules.
                tokio::task::spawn_blocking(move || fastctx_platform::host_process::wait_for_exit(&host)).await??;
            }
            Command::Finish { world, tool_use_id } => {
                let owner = serde_json::from_str(&input)?;
                request(&world, Call::ClaudeCallFinished { tool_use_id, owner }).await?;
            }
        }
        Ok(())
    })
}

async fn request(world: &std::path::Path, call: Call) -> anyhow::Result<()> {
    let world = WorldDir::at(world);
    let mut client = Client::connect_or_start(&world, Peer::Command { resident: None, session: None }).await?;
    match client.call(call).await? {
        Reply::Done => Ok(()),
        other => bail!("the node answered the Claude call claim with {other:?}"),
    }
}
