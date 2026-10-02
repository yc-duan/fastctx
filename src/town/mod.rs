//! The town: residents living in a project, and the sessions the World holds for them.
//!
//! Every resident is a session of some agent program on some machine. The World does four
//! things with the sessions it holds: starts them, delivers words to them (a new turn when
//! idle, folded into the running turn when busy, woken when stopped), lets anyone see what
//! they are doing, and stops them. Asking a colleague and a user talking to a resident from
//! another device are two uses of the same four operations (`specs/world-colleagues.md`).

pub(crate) mod admin;
pub(crate) mod claude;
pub(crate) mod cli;
pub(crate) mod codex;
pub(crate) mod core;
pub(crate) mod desk;
pub(crate) mod journal;
pub(crate) mod project;
pub(crate) mod records;
pub(crate) mod seen;
pub(crate) mod session;
pub(crate) mod talk;
pub(crate) mod talk_cli;
pub(crate) mod w;
pub(crate) mod who;

use std::path::PathBuf;
use std::sync::Arc;

/// Starts this machine's town: the Codex host that will hold its Codex residents and the town
/// core. The node daemon calls it once; the World's Claude plugin is rewritten at the same time
/// so it always matches the running binary.
pub(crate) fn start(
    machine: String,
    home: PathBuf,
    world_dir: PathBuf,
    codex_home: PathBuf,
    log: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<Arc<core::Town>, String> {
    let fastctx = std::env::current_exe().map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?;
    let workdir = world_dir.join("codex");
    std::fs::create_dir_all(&workdir)
        .map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(&workdir)))?;
    claude::write_plugin(&world_dir)?;
    let (codex, events) = codex::host::CodexHost::new(codex_home, workdir, Arc::clone(&log));
    Ok(core::Town::new(core::TownConfig { machine, world_dir, fastctx, home }, codex, events, log))
}
