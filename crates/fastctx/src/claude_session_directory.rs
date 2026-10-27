//! The mod reserves its exchange before its first filesystem write.

use std::path::{Path, PathBuf};

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
    // The login the session runs with, found as Claude Code finds it, and
    // whether its daemon runs it: a background session the World did not
    // start is the owner's only under a login that serves no one else
    // (`fastctx_node`'s adoption). A path that is not Unicode says nothing.
    let home = match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => fastctx_platform::user_home()?.join(".claude"),
    }
    .into_os_string()
    .into_string()
    .ok();
    // Claude Code's daemon runs background sessions, the World's among them,
    // and names each one's job folder to what it starts (2.1.303 and 2.1.310
    // keep its session kind from a plugin's commands, not the folder).
    let background = Some(
        std::env::var_os("CLAUDE_JOB_DIR").is_some_and(|dir| !dir.is_empty())
            || matches!(std::env::var("CLAUDE_CODE_SESSION_KIND").as_deref(), Ok("bg" | "daemon" | "daemon-worker")),
    );
    let host =
        Host { agent: "claude".parse().expect("a known agent"), client: "claude-code".into(), version: String::new() };
    let world = fastctx_client::World::new(WorldDir::at(world), Peer::Session { host });
    let (reply, _) = tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(
        world.call(Call::ClaudeSessionDirectory { session, previous, cwd, process, conversation, home, background }),
    )?;
    let Reply::ClaudeSessionDirectory { path } = reply else {
        anyhow::bail!("the node did not reserve the Claude session directory");
    };
    println!("{}", serde_json::to_string(&path)?);
    Ok(())
}
