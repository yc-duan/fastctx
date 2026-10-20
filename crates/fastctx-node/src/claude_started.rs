//! Records native startup without relying on the JavaScript mod being loaded.

use std::io::Read as _;
use std::path::Path;

use fastctx_hosts::claude::startup_hook::{self, Started};
use fastctx_types::ResidentId;
use fastctx_types::layout::WorldDir;
use serde::Deserialize;

pub fn run(world: &Path, conversation: &ResidentId) -> std::io::Result<()> {
    #[derive(Deserialize)]
    struct Input {
        session_id: String,
    }
    // SessionStart carries metadata only. Bound stdin independently of the host.
    let mut input = Vec::new();
    std::io::stdin().take(1024 * 1024).read_to_end(&mut input)?;
    let input: Input = serde_json::from_slice(&input)?;
    let directory = WorldDir::at(world).conversation(conversation);
    if !directory.path().is_dir() {
        return Err(std::io::Error::other("the conversation's directory no longer exists"));
    }
    let started = Started {
        session: input.session_id,
        at: jiff::Timestamp::now(),
        process: fastctx_platform::host_process::parent_identity()?,
    };
    fastctx_platform::publish(
        &startup_hook::receipt(&directory),
        &serde_json::to_vec(&started)?,
        fastctx_platform::Permissions::Private,
    )?;
    Ok(())
}
