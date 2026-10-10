//! Uninstall is independent of node startup and installation migration.

mod inventory;
mod runtime;
mod execute;
mod logout;
mod helper;
mod privileged;
#[cfg(target_os = "linux")]
pub mod package;
pub use helper::{run_outside, read_request};

pub use inventory::{preview, Request, Action, Step};
pub use execute::{run, run_system, finish_application};
pub use privileged::system_worker;
pub use fastctx_types::uninstall::{Event, Plan};

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
