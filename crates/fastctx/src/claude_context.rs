//! The mod reads only its conversation's account layer through the local node.

use fastctx_types::{Call, Reply, ipc::Peer, layout::WorldDir, residents::Host};
use std::path::Path;

pub fn run(world: &Path, session: String) -> anyhow::Result<()> {
    let host =
        Host { agent: "claude".parse().expect("known agent"), client: "claude-code".into(), version: String::new() };
    let world = fastctx_client::World::new(WorldDir::at(world), Peer::Session { host });
    let (reply, _) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(world.call(Call::ClaudeContext { session }))?;
    let Reply::ClaudeContext { bodies } = reply else {
        anyhow::bail!("the node did not answer the Claude context bodies");
    };
    println!("{}", serde_json::to_string(&bodies)?);
    Ok(())
}
