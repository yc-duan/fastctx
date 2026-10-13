//! The mod reserves its exchange before its first filesystem write.

use std::path::Path;

use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::Host;
use fastctx_types::{Call, Reply};

pub fn run(
    world: &Path,
    session: String,
    previous: Option<String>,
    cwd: Option<String>,
    conversation: Option<String>,
) -> anyhow::Result<()> {
    let process = Some(fastctx_platform::host_process::parent_identity()?);
    let host =
        Host { agent: "claude".parse().expect("a known agent"), client: "claude-code".into(), version: String::new() };
    let world = fastctx_client::World::new(WorldDir::at(world), Peer::Session { host });
    let (reply, _) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(world.call(Call::ClaudeSessionDirectory { session, previous, cwd, process, conversation }))?;
    let Reply::ClaudeSessionDirectory { path } = reply else {
        anyhow::bail!("the node did not reserve the Claude session directory");
    };
    println!("{}", serde_json::to_string(&path)?);
    Ok(())
}
