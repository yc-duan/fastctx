//! How long this computer's node keeps a command's output and scratch once
//! its end was delivered (`world-run.md`, 输出): a number of days in
//! `<FASTCTX_HOME>/world/retention.json`, which the node creates with its
//! default and watches for replacements.

use std::io;
use std::path::PathBuf;

use fastctx_platform::Permissions;
use fastctx_platform::atomic::publish;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Retention {
    command_artifact_days: u32,
}

fn path() -> Result<PathBuf, String> {
    Ok(fastctx_platform::fastctx_root().map_err(|error| error.to_string())?.join("world").join("retention.json"))
}

/// The days kept, as the node's file says; none before the node first ran
/// here and wrote it.
#[fastctx_gui_command::command]
pub(crate) async fn retention_read() -> Result<Option<u32>, String> {
    match std::fs::read(path()?) {
        Ok(bytes) => serde_json::from_slice::<Retention>(&bytes)
            .map(|retention| Some(retention.command_artifact_days))
            .map_err(|error| format!("reading the retention settings: {error}")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("reading the retention settings: {error}")),
    }
}

/// Replaces the days kept; the node takes the new number as the file
/// changes.
#[fastctx_gui_command::command]
pub(crate) async fn retention_write(days: u32) -> Result<(), String> {
    let bytes = serde_json::to_vec(&Retention { command_artifact_days: days }).map_err(|error| error.to_string())?;
    publish(&path()?, &bytes, Permissions::Private).map_err(|error| format!("writing the retention settings: {error}"))
}
