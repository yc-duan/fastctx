//! Uninstall is independent of node startup and installation migration.

#[cfg(target_os = "macos")]
pub mod bundle;
mod claude_cache;
mod claude_cache_paths;
mod claude_session_cache;
mod claude_work_cache;
mod execute;
mod helper;
mod inventory;
mod logout;
#[cfg(target_os = "linux")]
pub mod package;
mod privileged;
mod runtime;
pub use helper::{read_request, run_outside};

pub use execute::{finish_application, run, run_system};
pub use fastctx_types::uninstall::{Event, Plan};
pub use inventory::{Action, Request, Step, preview};
pub use privileged::system_worker;

/// The managed machine whose owner keeps it, as this World's memberships
/// say (`world-network.md`, 受管机器).
pub fn managed(world: &fastctx_types::layout::WorldDir) -> anyhow::Result<Option<fastctx_types::uninstall::Managed>> {
    Ok(fastctx_net::link::state::managed_on(&world.net())?
        .map(|(hub, owner)| fastctx_types::uninstall::Managed { hub: hub.to_string(), owner: owner.to_string() }))
}

pub(crate) fn entries(path: &std::path::Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

pub(crate) fn read<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> anyhow::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
